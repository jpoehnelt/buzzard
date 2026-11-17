//! `.llb` archives as part of a workspace.
//!
//! A member of an archive is addressed the way LabVIEW addresses it in a
//! link: by the path of the archive followed by the member's name, as in
//! `lib/util.llb/Helper.vi`. Such a path names no file on disk, so
//! everything that reads a file by its path goes through [`read`], which
//! takes the bytes of a member out of its archive.
//!
//! A member name is data from the archive, never a path. A virtual path is
//! made by putting the name behind the archive's path as one component
//! ([`member_path`]), and only for a name that is one component
//! ([`is_addressable`]): a name such as `../x.vi` is reported and not
//! opened, so that no member can stand for another file.

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf, MAIN_SEPARATOR_STR};
use std::sync::{Arc, Mutex, PoisonError};

use buzzard_rsrc::{FileKind, Llb, LlbMember, LLB_EXTENSION};
use rayon::prelude::*;
use serde::Serialize;

use crate::graph::{code_page_readings, fold};
use crate::inspector::Inspection;
use crate::workspace::is_rsrc_file;

/// True if the path has the extension of an LLB archive.
pub fn is_archive(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(LLB_EXTENSION))
}

/// Whether a member name can be the last component of a virtual path: it
/// is not empty, not `.` or `..`, and holds no separator and no NUL.
pub fn is_addressable(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

/// The virtual path of the member `name` of the archive at `archive`.
///
/// The name is appended as text. `Path::join` would let a name that is an
/// absolute path or starts with a drive replace the archive's path.
pub fn member_path(archive: &Path, name: &str) -> PathBuf {
    let mut path = OsString::from(archive.as_os_str());
    path.push(MAIN_SEPARATOR_STR);
    path.push(name);
    PathBuf::from(path)
}

/// The archive and member name of a virtual path: `Some` when the parent
/// of `path` is an `.llb` file on disk. A file inside a directory that is
/// merely called `x.llb` is an ordinary file.
pub fn split(path: &Path) -> Option<(PathBuf, String)> {
    let archive = path.parent()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    (is_archive(archive) && archive.is_file()).then(|| (archive.to_path_buf(), name))
}

/// Whether `path` names something that can be analyzed: a file or
/// directory on disk, or a member of an archive on disk. A member path
/// whose archive cannot be read counts: reading it then gives the reason.
pub fn exists(path: &Path) -> bool {
    path.exists()
        || split(path)
            .is_some_and(|(archive, name)| !matches!(read_member(&archive, &name), Ok(None)))
}

/// The bytes of the binary file at `path`, which may be a member path.
///
/// # Errors
///
/// Returns the reason as text when the file, or the archive of a member,
/// cannot be read, or the archive has no such member.
pub fn read(path: &Path) -> Result<Vec<u8>, String> {
    let Some((archive, name)) = split(path) else {
        return buzzard_rsrc::read_file(path).map_err(|e| e.to_string());
    };
    read_member(&archive, &name)?
        .ok_or_else(|| format!("the archive has no member `{}`", printable(&name)))
}

/// The bytes of the member `name` of the archive file at `archive`;
/// `None` when the archive has no member of that name (see
/// [`Llb::find`]).
///
/// # Errors
///
/// Returns the reason as text when the archive cannot be read or its
/// directory cannot be parsed, or the member cannot be read.
pub fn read_member(archive: &Path, name: &str) -> Result<Option<Vec<u8>>, String> {
    let bytes = buzzard_rsrc::read_file(archive).map_err(|e| e.to_string())?;
    let llb = Llb::parse(&bytes).map_err(|e| e.to_string())?;
    llb.find(name)
        .map(|member| llb.read(member).map_err(|e| e.to_string()))
        .transpose()
}

/// A name from an archive on one line.
pub(crate) fn printable(name: &str) -> String {
    name.chars().flat_map(char::escape_debug).collect()
}

/// What is kept of an opened archive: its directory and what could not be
/// opened as a file of the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveFile {
    /// The directory entries, in the order of the archive.
    pub members: Vec<LlbMember>,
    /// Content resources that no directory entry names.
    pub unlisted: usize,
    /// One entry for each member that is not opened although it is a VI
    /// or control, with the reason: its name is not a file name, or an
    /// earlier member has the same name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
}

impl ArchiveFile {
    /// The names of the members that are files of the workspace: the VIs
    /// and controls whose name is addressable, each name once, in
    /// directory order.
    pub fn opened(&self) -> Vec<&str> {
        let mut seen = BTreeSet::new();
        self.members
            .iter()
            .filter(|member| is_opened(member) && is_addressable(&member.name))
            .map(|member| member.name.as_str())
            .filter(|name| seen.insert(*name))
            .collect()
    }
}

/// Whether a member is analyzed: the directory says it is a VI or a
/// control, or its name has the extension of one. An archive inside an
/// archive is listed and never opened.
fn is_opened(member: &LlbMember) -> bool {
    match member.kind {
        FileKind::Vi | FileKind::Control => true,
        FileKind::Library => false,
        FileKind::Other => is_rsrc_file(Path::new(&member.name)),
    }
}

/// An archive read from disk: its directory, and what was made of each
/// opened member, under its virtual path.
#[derive(Debug)]
pub struct Opened<T> {
    /// The directory and the members that were not opened.
    pub file: ArchiveFile,
    /// The opened members in directory order.
    pub members: Vec<(PathBuf, T)>,
}

/// Read the archive at `path` once, and parse and analyze its members in
/// parallel: `analyze` is given what parsing each member gave, on the
/// thread that parsed it.
///
/// # Errors
///
/// Returns the reason as text when the file cannot be read or its
/// directory cannot be parsed. A member that cannot be read or parsed is
/// not an error of the archive: `analyze` is given the reason.
pub fn open<T: Send>(
    path: &Path,
    analyze: impl Fn(Result<Inspection, String>) -> T + Sync,
) -> Result<Opened<T>, String> {
    let bytes = buzzard_rsrc::read_file(path).map_err(|e| e.to_string())?;
    let llb = Llb::parse(&bytes).map_err(|e| e.to_string())?;
    let (wanted, problems) = wanted(&llb);
    let members = wanted
        .par_iter()
        .map(|member| {
            let inspection = llb
                .read(member)
                .map_err(|e| e.to_string())
                .and_then(|bytes| Inspection::from_bytes(&bytes));
            (member_path(path, &member.name), analyze(inspection))
        })
        .collect();
    Ok(Opened {
        file: ArchiveFile {
            members: llb.members().to_vec(),
            unlisted: llb.unlisted(),
            problems,
        },
        members,
    })
}

/// Read only the directory of the archive at `path`.
///
/// # Errors
///
/// Returns the reason as text when the file cannot be read or its
/// directory cannot be parsed.
pub fn list(path: &Path) -> Result<ArchiveFile, String> {
    let bytes = buzzard_rsrc::read_file(path).map_err(|e| e.to_string())?;
    let llb = Llb::parse(&bytes).map_err(|e| e.to_string())?;
    let (_, problems) = wanted(&llb);
    Ok(ArchiveFile {
        members: llb.members().to_vec(),
        unlisted: llb.unlisted(),
        problems,
    })
}

/// The members to open, and why each of the others that is a VI or a
/// control is not.
fn wanted<'l>(llb: &'l Llb<'_>) -> (Vec<&'l LlbMember>, Vec<String>) {
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    let mut wanted = Vec::new();
    for member in llb.members().iter().filter(|member| is_opened(member)) {
        let name = printable(&member.name);
        if !is_addressable(&member.name) {
            problems.push(format!("member `{name}`: its name is not a file name"));
        } else if seen.insert(member.name.as_str()) {
            wanted.push(member);
        } else {
            problems.push(format!(
                "a second member named `{name}`: links name the first"
            ));
        }
    }
    (wanted, problems)
}

/// The member names of archives on disk, read once each. Used to decide
/// whether a link into an archive that is not part of the workspace has a
/// target.
#[derive(Debug, Default)]
pub struct Archives {
    /// By archive path: the member names without their letter case, or
    /// `None` for an archive that cannot be read.
    cache: Mutex<HashMap<PathBuf, Option<Arc<BTreeSet<String>>>>>,
}

impl Archives {
    fn names(&self, archive: &Path) -> Option<Arc<BTreeSet<String>>> {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache
            .entry(archive.to_path_buf())
            .or_insert_with(|| {
                let bytes = buzzard_rsrc::read_file(archive).ok()?;
                let llb = Llb::parse(&bytes).ok()?;
                // A name is stored in the code page of the saving machine,
                // and a link may spell it in UTF-8: every reading counts.
                let mut names = BTreeSet::new();
                for member in llb.members() {
                    names.insert(fold(&member.name));
                    let stored = [member.name.clone()];
                    for reading in code_page_readings(&stored) {
                        names.extend(reading.iter().map(|name| fold(name)));
                    }
                }
                Some(Arc::new(names))
            })
            .clone()
    }

    /// Whether the archive file at `archive` has a member called by the
    /// components `rest`, compared without letter case as the editor
    /// does. `None` when the archive cannot be read: nothing is known
    /// then.
    pub fn contains(&self, archive: &Path, rest: &[&str]) -> Option<bool> {
        let names = self.names(archive)?;
        Some(match rest {
            [name] => names.contains(&fold(name)),
            // An archive has no directories.
            _ => false,
        })
    }

    /// Whether there is something at `path`: a file or directory, or a
    /// member of an archive. `None` when `path` leads into an archive
    /// that cannot be read. A path that leads into any other file (the
    /// private data control inside an `.lvclass` file) is `into_file`:
    /// that file is not opened to look for the item.
    pub fn present(&self, path: &Path, into_file: bool) -> Option<bool> {
        if path.exists() {
            return Some(true);
        }
        let Some(file) = path.ancestors().skip(1).find(|above| above.is_file()) else {
            return Some(false);
        };
        if !is_archive(file) {
            return Some(into_file);
        }
        let rest: Vec<String> = path
            .strip_prefix(file)
            .ok()?
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
        self.contains(file, &rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_member_path_is_the_archive_path_and_one_more_component() {
        let archive = Path::new("lib/util.llb");
        assert_eq!(
            member_path(archive, "Helper.vi"),
            Path::new("lib/util.llb/Helper.vi")
        );
        // A name is never joined as a path: an absolute name or one with a
        // drive would replace the archive's path.
        for name in ["/etc/passwd", "C:evil.vi", "..", "../../x.vi"] {
            assert!(member_path(archive, name).starts_with("lib/util.llb"));
        }
    }

    #[test]
    fn only_a_single_component_is_addressable() {
        for name in ["a.vi", "odd name?.vi", "a:b.vi", "...", "\u{83}\u{65}.vi"] {
            assert!(is_addressable(name), "{name}");
        }
        for name in [
            "", ".", "..", "../x.vi", "a/b.vi", "a\\b.vi", "/x.vi", "a\0.vi",
        ] {
            assert!(!is_addressable(name), "{name:?}");
        }
    }

    #[test]
    fn recognizes_the_extension() {
        assert!(is_archive(Path::new("x/error.llb")));
        assert!(is_archive(Path::new("x/ERROR.LLB")));
        assert!(!is_archive(Path::new("x/error.lvlibp")));
        assert!(!is_archive(Path::new("x/llb")));
    }
}
