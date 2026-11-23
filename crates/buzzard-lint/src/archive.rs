//! `.llb` archives as part of a workspace.
//!
//! A member of an archive is addressed the way LabVIEW addresses it in a
//! link, by the path of the archive followed by the member's name, as in
//! `lib/util.llb/Helper.vi`. Such a path names no file on disk, so
//! everything that reads a file by its path goes through [`read`], which
//! takes the bytes of a member out of its archive.
//!
//! A member name is data from the archive, never a path. [`member_path`]
//! makes a virtual path by appending the name to the archive's path as one
//! component. It is used only for a name that is one component and prints
//! on one line ([`is_addressable`]). A name such as `../x.vi` is reported
//! and not opened, so no member can stand for another file or forge a line
//! of a report.
//!
//! The members that become files of a workspace are the VIs, the controls,
//! and the project files (`.lvlib`, `.lvclass`, `.lvproj`), which are XML
//! text as they are on disk. The type in the directory cannot hide a
//! member. A member whose name has the extension of a VI is opened
//! whatever type its entry states, and the parser decides what it is.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf, MAIN_SEPARATOR_STR};
use std::sync::{Arc, Mutex, PoisonError};

use buzzard_rsrc::{FileKind, Llb, LlbMember, RsrcFile, Storage, LLB_EXTENSION};
use rayon::prelude::*;
use serde::Serialize;

use crate::graph::{code_page_readings, fold};
use crate::project::is_project_file;
use crate::workspace::is_rsrc_file;

/// True if the path has the extension of an LLB archive.
pub fn is_archive(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(LLB_EXTENSION))
}

/// The most bytes that the compressed members of one archive may inflate
/// to together when the archive is opened as part of a workspace. The
/// members past that limit are reported and not read. A member that is
/// stored uncompressed takes no more than its bytes of the archive and
/// does not count.
pub const MAX_INFLATED_BYTES: u64 = 1024 * 1024 * 1024;

/// Whether a member name can be the last component of a virtual path.
/// Such a name is not empty, is not `.` or `..`, and holds no separator
/// and no ASCII control character (a NUL, a line break that would split a
/// line of a report, or the start of a terminal escape sequence). The
/// bytes 0x80 to 0x9f of a name in a legacy code page are letters, not
/// controls.
///
/// On Windows a name with a colon is refused as well. `C:x.vi` is a path
/// on another drive there, and joining it to a path replaces the path.
pub fn is_addressable(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && !names_a_drive(name)
        && !name.chars().any(|c| c.is_ascii_control())
}

/// Whether `name` holds a colon on a system where a colon ends a drive.
fn names_a_drive(name: &str) -> bool {
    cfg!(windows) && name.contains(':')
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

/// The archive and member name of a virtual path. `Some` when the parent
/// of `path` is an `.llb` file on disk. A file inside a directory called
/// `x.llb` is an ordinary file, and so is a path whose last component
/// could not be the name of an opened member ([`is_addressable`]).
pub fn split(path: &Path) -> Option<(PathBuf, String)> {
    let archive = path.parent()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    (is_addressable(&name) && is_archive(archive) && archive.is_file())
        .then(|| (archive.to_path_buf(), name))
}

/// A member path with the member's name as its archive spells it.
/// [`Llb::find`] finds a member without regard to letter case, so every
/// spelling of the name gives the same path. `None` when `path` is not a
/// member path, when the archive cannot be read, or when the archive has
/// no such member.
pub fn canonical(path: &Path) -> Option<PathBuf> {
    let (archive, name) = split(path)?;
    let bytes = buzzard_rsrc::read_file(&archive).ok()?;
    let llb = Llb::parse(&bytes).ok()?;
    let member = llb.find(&name)?;
    is_addressable(&member.name).then(|| member_path(&archive, &member.name))
}

/// Whether `path` names something that can be analyzed: a file or
/// directory on disk, or a member of an archive on disk. A member path
/// whose archive cannot be read counts as existing, and reading it gives
/// the reason.
pub fn exists(path: &Path) -> bool {
    path.exists()
        || split(path)
            .is_some_and(|(archive, name)| !matches!(read_member(&archive, &name), Ok(None)))
}

/// The bytes of the file at `path`, which may be a member path.
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

/// The bytes of the member `name` of the archive file at `archive`.
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

/// A name from an archive, printed on one line. ASCII control characters
/// are escaped, and everything else is as the archive spells it.
pub(crate) fn printable(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// What the workspace keeps of an opened archive: its directory and the
/// members that could not be opened as files of the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveFile {
    /// The directory entries, in the order of the archive.
    pub members: Vec<LlbMember>,
    /// Content resources that no directory entry names.
    pub unlisted: usize,
    /// One entry for each member that is not opened although it is a VI,
    /// a control, or a project file, with the reason: its name is not a
    /// file name, or an earlier member has the same name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
}

impl ArchiveFile {
    /// The names of the members that are files of the workspace: the
    /// VIs, controls, and project files whose name is addressable, each
    /// name once, in directory order.
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

/// Whether a member is analyzed. It is when its type in the directory is
/// VI or control, or when its name has the extension of a VI, a control,
/// or a project file. The name counts whatever the type in the directory
/// is, so one changed type code cannot hide a VI. An archive inside an
/// archive (a member called `x.llb`) is listed and never opened.
fn is_opened(member: &LlbMember) -> bool {
    let name = Path::new(&member.name);
    matches!(member.kind, FileKind::Vi | FileKind::Control)
        || is_rsrc_file(name)
        || is_project_file(name)
}

/// An archive read from disk: its directory, and the result of the
/// analysis of each opened member, under its virtual path.
#[derive(Debug)]
pub struct Opened<T> {
    /// The directory and the members that were not opened.
    pub file: ArchiveFile,
    /// The opened members in directory order.
    pub members: Vec<(PathBuf, T)>,
}

/// Whether the bytes are an archive without a directory of members, which
/// is the zipped program tree that an application builder writes. Such a
/// file is a resource file like any other, and is not opened as an archive.
fn has_no_members(bytes: &[u8]) -> bool {
    RsrcFile::parse(bytes).is_ok_and(|file| buzzard_rsrc::llb::is_program_tree(&file))
}

/// Read the archive at `path` once, and read its members in parallel.
/// `analyze` gets the virtual path and the bytes of each member, or the
/// reason the member could not be read, on the thread that read it.
///
/// `Ok(None)` for an archive that has no directory of members because it
/// was built into an application. The caller reads it as a plain resource
/// file.
///
/// Compressed members are read in directory order until they would
/// inflate to more than [`MAX_INFLATED_BYTES`] together. For each of the
/// others, `analyze` gets that as the reason.
///
/// # Errors
///
/// Returns the reason as text when the file cannot be read or its
/// directory cannot be parsed. A member that cannot be read is not an
/// error of the archive. `analyze` gets the reason.
pub fn open<T: Send>(
    path: &Path,
    analyze: impl Fn(&Path, Result<Vec<u8>, String>) -> T + Sync,
) -> Result<Option<Opened<T>>, String> {
    open_within(path, MAX_INFLATED_BYTES, analyze)
}

fn open_within<T: Send>(
    path: &Path,
    budget: u64,
    analyze: impl Fn(&Path, Result<Vec<u8>, String>) -> T + Sync,
) -> Result<Option<Opened<T>>, String> {
    let bytes = buzzard_rsrc::read_file(path).map_err(|e| e.to_string())?;
    if has_no_members(&bytes) {
        return Ok(None);
    }
    let llb = Llb::parse(&bytes).map_err(|e| e.to_string())?;
    let (wanted, problems) = wanted(&llb);
    // The directory decides which members fit the budget before anything
    // is read, so the result does not depend on the order the threads run
    // in.
    let mut inflated = 0u64;
    let within: Vec<bool> = wanted
        .iter()
        .map(|member| {
            if member.storage == Storage::Stored {
                return true;
            }
            inflated = inflated.saturating_add(member.size);
            inflated <= budget
        })
        .collect();
    let members = wanted
        .par_iter()
        .zip(within)
        .map(|(member, within)| {
            let virtual_path = member_path(path, &member.name);
            let bytes = if within {
                llb.read(member).map_err(|e| e.to_string())
            } else {
                Err(format!(
                    "the compressed members of the archive hold more than {budget} bytes \
                     together; this one is not read"
                ))
            };
            let analyzed = analyze(&virtual_path, bytes);
            (virtual_path, analyzed)
        })
        .collect();
    Ok(Some(Opened {
        file: ArchiveFile {
            members: llb.members().to_vec(),
            unlisted: llb.unlisted(),
            problems,
        },
        members,
    }))
}

/// Read only the directory of the archive at `path`. `Ok(None)` for an
/// archive without one (see [`open`]).
///
/// # Errors
///
/// Returns the reason as text when the file cannot be read or its
/// directory cannot be parsed.
pub fn list(path: &Path) -> Result<Option<ArchiveFile>, String> {
    let bytes = buzzard_rsrc::read_file(path).map_err(|e| e.to_string())?;
    if has_no_members(&bytes) {
        return Ok(None);
    }
    let llb = Llb::parse(&bytes).map_err(|e| e.to_string())?;
    let (_, problems) = wanted(&llb);
    Ok(Some(ArchiveFile {
        members: llb.members().to_vec(),
        unlisted: llb.unlisted(),
        problems,
    }))
}

/// The members to open, and why each of the others that is a VI, a
/// control, or a project file is not.
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

/// The member names of archives on disk, read once each. `deps` and `BZ006`
/// use them to decide whether a link into an archive that is not part of the
/// workspace has a target.
#[derive(Debug, Default)]
pub struct Archives {
    /// By archive path, the case-folded member names, each with whether
    /// the member is itself an archive. `None` for an archive that cannot
    /// be read.
    cache: Mutex<HashMap<PathBuf, Option<Arc<Names>>>>,
}

type Names = BTreeMap<String, bool>;

impl Archives {
    fn names(&self, archive: &Path) -> Option<Arc<Names>> {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache
            .entry(archive.to_path_buf())
            .or_insert_with(|| {
                let bytes = buzzard_rsrc::read_file(archive).ok()?;
                let llb = Llb::parse(&bytes).ok()?;
                // A name is stored in the code page of the saving machine,
                // and a link may spell it in UTF-8, so every reading counts.
                let mut names = Names::new();
                for member in llb.members() {
                    let nested =
                        member.kind == FileKind::Library || is_archive(Path::new(&member.name));
                    let mut add = |name: &str| {
                        *names.entry(fold(name)).or_default() |= nested;
                    };
                    add(&member.name);
                    let stored = [member.name.clone()];
                    for reading in code_page_readings(&stored) {
                        for name in &reading {
                            add(name);
                        }
                    }
                }
                Some(Arc::new(names))
            })
            .clone()
    }

    /// Whether the archive file at `archive` has the member that the
    /// components `rest` name, compared without regard to letter case as
    /// the editor compares them. `None` when the answer is unknown,
    /// because the archive cannot be read or the components lead into a
    /// member that is itself an archive, which is never opened.
    pub fn contains(&self, archive: &Path, rest: &[&str]) -> Option<bool> {
        let names = self.names(archive)?;
        match rest {
            [name] => Some(names.contains_key(&fold(name))),
            [first, ..] => match names.get(&fold(first)) {
                Some(true) => None,
                // An archive has no directories.
                Some(false) | None => Some(false),
            },
            [] => Some(false),
        }
    }

    /// Whether there is something at `path`: a file or directory, or a
    /// member of an archive. `None` when `path` leads into an archive
    /// that cannot be read. For a path that leads into any other file
    /// (such as the private data control inside an `.lvclass` file), the
    /// answer is `into_file`, because that file is not opened to look for
    /// the item.
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
        // A name is never joined as a path, because an absolute name or
        // one with a drive would replace the archive's path.
        for name in ["/etc/passwd", "C:evil.vi", "..", "../../x.vi"] {
            assert!(member_path(archive, name).starts_with("lib/util.llb"));
        }
    }

    #[test]
    fn only_a_single_component_is_addressable() {
        for name in ["a.vi", "odd name?.vi", "...", "\u{83}\u{65}.vi"] {
            assert!(is_addressable(name), "{name}");
        }
        // A colon starts a drive on Windows only.
        assert_eq!(is_addressable("a:b.vi"), !cfg!(windows));
        assert_eq!(is_addressable("C:drive.vi"), !cfg!(windows));
        for name in [
            "",
            ".",
            "..",
            "../x.vi",
            "a/b.vi",
            "a\\b.vi",
            "/x.vi",
            "a\0.vi",
            "a\nb.vi",
            "\u{1b}[31mred.vi",
            "a\u{7f}b.vi",
        ] {
            assert!(!is_addressable(name), "{name:?}");
        }
    }

    fn temp_archive(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "buzzard-archive-unit-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.llb");
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// What `open_within` passes to `analyze`, by member name: the size of
    /// what was read, or the reason nothing was.
    fn sizes(path: &Path, budget: u64) -> Vec<(String, Result<usize, String>)> {
        let opened = open_within(path, budget, |_, bytes| bytes.map(|b| b.len()))
            .unwrap()
            .unwrap();
        let names = opened.members.into_iter().map(|(path, read)| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, read)
        });
        names.collect()
    }

    #[test]
    fn members_are_handed_over_in_directory_order() {
        use buzzard_rsrc::builder::LlbBuilder;
        let names = ["z.vi", "a.vi", "M.vi", "b.ctl", "0.vi"];
        let mut builder = LlbBuilder::default();
        for (size, name) in names.iter().enumerate() {
            builder = builder.member(name, &vec![1; size + 1]);
        }
        let path = temp_archive("order", &builder.build());
        let read = sizes(&path, MAX_INFLATED_BYTES);
        let expected: Vec<(String, Result<usize, String>)> = names
            .iter()
            .enumerate()
            .map(|(size, name)| ((*name).to_owned(), Ok(size + 1)))
            .collect();
        assert_eq!(read, expected);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_compressed_members_of_an_archive_share_one_budget() {
        use buzzard_rsrc::builder::LlbBuilder;
        let data = vec![0u8; 1000];
        let path = temp_archive(
            "budget",
            &LlbBuilder::default()
                .compressed("1.vi", &data)
                .member("stored.vi", &data)
                .legacy("2.vi", &data)
                .compressed("3.vi", &data)
                .member("also stored.vi", &data)
                .compressed("4.vi", &data[..1])
                .build(),
        );
        // With a budget of 3001 everything fits. With 2000 the third
        // compressed member does not fit, and neither does a compressed
        // member after it, however small. Stored members take no more
        // than their bytes of the file and are always read.
        let all = sizes(&path, 3001);
        assert!(all.iter().all(|(_, read)| read.is_ok()), "{all:?}");
        for _ in 0..3 {
            let read = sizes(&path, 2000);
            let refused: Vec<&str> = read
                .iter()
                .filter(|(_, read)| read.is_err())
                .map(|(name, _)| name.as_str())
                .collect();
            assert_eq!(refused, ["3.vi", "4.vi"]);
            assert_eq!(
                read[3].1.as_ref().unwrap_err(),
                "the compressed members of the archive hold more than 2000 bytes together; \
                 this one is not read"
            );
            assert_eq!(read[1].1, Ok(1000));
            assert_eq!(read[4].1, Ok(1000));
        }
        assert_eq!(sizes(&path, 0).iter().filter(|(_, r)| r.is_ok()).count(), 2);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn an_archive_without_a_directory_is_not_opened_as_one() {
        use buzzard_rsrc::builder::RsrcBuilder;
        use buzzard_rsrc::header::file_type;
        let built = RsrcBuilder::new(file_type::LIBRARY)
            .resource(b"LVzp", 0, None, vec![0; 16])
            .build();
        let path = temp_archive("built", &built);
        assert_eq!(list(&path), Ok(None));
        assert!(open(&path, |_, _| ()).unwrap().is_none());
        // An archive with neither a directory nor a program tree is an
        // error, and so is a VI.
        let empty = RsrcBuilder::new(file_type::LIBRARY)
            .resource(b"PALM", 0, None, vec![0; 16])
            .build();
        std::fs::write(&path, empty).unwrap();
        assert!(list(&path).unwrap_err().contains("no directory"));
        let vi = RsrcBuilder::default()
            .resource(b"LVzp", 0, None, vec![0; 16])
            .build();
        std::fs::write(&path, vi).unwrap();
        assert!(list(&path).unwrap_err().contains("not an LLB archive"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_name_is_printed_on_one_line_and_otherwise_as_it_is() {
        assert_eq!(printable("a\\b `c` |d|.vi"), "a\\b `c` |d|.vi");
        assert_eq!(
            printable("a\nb\r\t\0\u{1b}[0m\u{7f}"),
            "a\\nb\\r\\t\\u{0}\\u{1b}[0m\\u{7f}"
        );
        // Bytes of a legacy code page above 0x7f are letters.
        assert_eq!(printable("\u{83}\u{65}\u{e9}"), "\u{83}\u{65}\u{e9}");
    }

    #[test]
    fn recognizes_the_extension() {
        assert!(is_archive(Path::new("x/error.llb")));
        assert!(is_archive(Path::new("x/ERROR.LLB")));
        assert!(!is_archive(Path::new("x/error.lvlibp")));
        assert!(!is_archive(Path::new("x/llb")));
    }
}
