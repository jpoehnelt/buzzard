//! The set of analyzed files and their parse results.

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};

use buzzard_rsrc::{ViMetadata, RSRC_EXTENSIONS};
use rayon::prelude::*;
use walkdir::WalkDir;

use crate::archive::{self, ArchiveFile};
use crate::dataflow::{self, Finding};
use crate::inspector::Inspection;
use crate::project::{is_project_file, ProjectFile};
use crate::verify::{self, Issue};

/// What is kept in memory for one binary file: its metadata and the
/// results of the dataflow and state machine checks. The diagram itself
/// is not retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    /// Version, flags, links and other facts decoded from the file.
    pub metadata: ViMetadata,
    /// Sorted findings of the dataflow checks; empty when the file has no
    /// block diagram that could be decoded.
    pub dataflow: Vec<Finding>,
    /// What the verification of the diagram's state machines found.
    pub state_machines: Vec<Issue>,
}

impl From<Inspection> for Analysis {
    fn from(inspection: Inspection) -> Self {
        let diagram = inspection.diagram.as_ref();
        Self {
            dataflow: diagram.map(dataflow::analyze).unwrap_or_default(),
            state_machines: diagram
                .map(|diagram| verify::verify(diagram).issues())
                .unwrap_or_default(),
            metadata: inspection.metadata,
        }
    }
}

/// What is known about one file of the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    /// A parsed binary resource file, such as a `.vi` or `.ctl`.
    Parsed(Box<Analysis>),
    /// A parsed `.lvproj`, `.lvlib` or `.lvclass` file.
    Project(Box<ProjectFile>),
    /// An `.llb` archive whose directory was read. Its members that are
    /// VIs or controls are files of the workspace themselves, each under
    /// the path of the archive followed by its name.
    Archive(Box<ArchiveFile>),
    /// The file could not be read or parsed; the message says why.
    Failed(String),
}

impl FileState {
    /// Read one file from disk. Project-level XML files and archives are
    /// recognized by extension; everything else is read as a binary
    /// resource file. `path` may be the path of a member of an archive.
    ///
    /// For an archive this gives the state of the archive alone; see
    /// [`load_all`](Self::load_all).
    pub fn load(path: &Path) -> Self {
        Self::load_all(path).swap_remove(0).1
    }

    /// Read one file from disk, and with an archive each member that is a
    /// VI or a control. The first entry is the file itself; the members
    /// follow in the order of the archive, under their virtual paths. The
    /// archive is read once and its members are parsed in parallel.
    pub fn load_all(path: &Path) -> Vec<(PathBuf, FileState)> {
        let own = |state| vec![(path.to_path_buf(), state)];
        if is_project_file(path) {
            return own(match ProjectFile::from_path(path) {
                Ok(project) => FileState::Project(Box::new(project)),
                Err(reason) => FileState::Failed(reason),
            });
        }
        if !archive::is_archive(path) || archive::split(path).is_some() {
            return own(Self::from_inspection(Inspection::from_path(path)));
        }
        match archive::open(path, Self::from_inspection) {
            Ok(opened) => {
                let mut out = own(FileState::Archive(Box::new(opened.file)));
                out.extend(opened.members);
                out
            }
            Err(reason) => own(FileState::Failed(reason)),
        }
    }

    /// Turn a backend's answer for a binary file into a file state.
    pub fn from_inspection(inspection: Result<Inspection, String>) -> Self {
        match inspection {
            Ok(inspection) => FileState::Parsed(Box::new(inspection.into())),
            Err(reason) => FileState::Failed(reason),
        }
    }

    /// The metadata of a parsed binary file; `None` for anything else.
    pub fn metadata(&self) -> Option<&ViMetadata> {
        match self {
            FileState::Parsed(analysis) => Some(&analysis.metadata),
            FileState::Project(_) | FileState::Archive(_) | FileState::Failed(_) => None,
        }
    }

    /// The parsed project, library or class; `None` for anything else.
    pub fn project(&self) -> Option<&ProjectFile> {
        match self {
            FileState::Project(project) => Some(project),
            FileState::Parsed(_) | FileState::Archive(_) | FileState::Failed(_) => None,
        }
    }

    /// The directory of an opened archive; `None` for anything else.
    pub fn archive(&self) -> Option<&ArchiveFile> {
        match self {
            FileState::Archive(archive) => Some(archive),
            FileState::Parsed(_) | FileState::Project(_) | FileState::Failed(_) => None,
        }
    }
}

/// The analyzed files, keyed by their path as it was given.
///
/// A file has one identity however its path is spelled (see [`identity`]):
/// `a.vi`, `./a.vi` and `/cwd/a.vi` are one entry, stored under the first
/// spelling that was seen, which is also the one diagnostics show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Workspace {
    files: BTreeMap<PathBuf, FileState>,
    /// Identity of every file, mapped to its key in `files`.
    keys: HashMap<PathBuf, PathBuf>,
    /// Identities of the scan roots, sorted and deduplicated.
    roots: Vec<PathBuf>,
}

impl Workspace {
    /// Discover and parse every analyzable file under `roots` in parallel.
    pub fn scan(roots: &[PathBuf]) -> Self {
        let base = std::env::current_dir().ok();
        let paths = discover_from(base.as_deref(), roots);
        let loaded: Vec<Vec<(PathBuf, FileState)>> = paths
            .into_par_iter()
            .map(|path| FileState::load_all(&path))
            .collect();
        // A member that was also given as a root of its own is one file.
        let mut files = BTreeMap::new();
        let mut keys = HashMap::new();
        for (path, state) in loaded.into_iter().flatten() {
            let path = normalize(&path);
            let key = keys
                .entry(identity_from(base.as_deref(), &path))
                .or_insert(path);
            files.insert(key.clone(), state);
        }
        let mut roots: Vec<PathBuf> = roots
            .iter()
            .map(|root| identity_from(base.as_deref(), root))
            .collect();
        roots.sort();
        roots.dedup();
        Self { files, keys, roots }
    }

    /// Every file and its state, in path order.
    pub fn files(&self) -> &BTreeMap<PathBuf, FileState> {
        &self.files
    }

    /// The key under which `path` is stored, whatever its spelling.
    fn key(&self, path: &Path) -> Option<PathBuf> {
        let path = normalize(path);
        if self.files.contains_key(&path) {
            return Some(path);
        }
        self.keys.get(&identity(&path)).cloned()
    }

    /// The state of one file, whatever the spelling of its path.
    pub fn get(&self, path: &Path) -> Option<&FileState> {
        self.files.get(&self.key(path)?)
    }

    /// Add or replace a file. A file that is already present under another
    /// spelling of the same path keeps its existing key.
    ///
    /// Replacing an archive drops the members it had: the caller inserts
    /// the members of the new state (see [`FileState::load_all`]).
    pub fn insert(&mut self, path: &Path, state: FileState) {
        let key = self.key(path).unwrap_or_else(|| {
            let key = normalize(path);
            self.keys.insert(identity(&key), key.clone());
            key
        });
        self.drop_members(&key);
        self.files.insert(key, state);
    }

    /// Drop a file, returning its last state. The members of an archive
    /// go with it.
    pub fn remove(&mut self, path: &Path) -> Option<FileState> {
        let key = self.key(path)?;
        self.drop_members(&key);
        self.keys.remove(&identity(&key));
        self.files.remove(&key)
    }

    /// Drop the member files of the archive stored under `key`, if it is
    /// one.
    fn drop_members(&mut self, key: &Path) {
        let Some(FileState::Archive(archive)) = self.files.get(key) else {
            return;
        };
        let members: Vec<PathBuf> = archive
            .opened()
            .into_iter()
            .map(|name| archive::member_path(key, name))
            .collect();
        for member in members {
            self.keys.remove(&identity(&member));
            self.files.remove(&member);
        }
    }

    /// Re-read one file from disk, dropping it if it no longer exists. An
    /// archive is re-read with its members.
    pub fn refresh(&mut self, path: &Path) {
        if path.is_file() || archive::split(path).is_some() {
            for (path, state) in FileState::load_all(path) {
                self.insert(&path, state);
            }
        } else {
            self.remove(path);
        }
    }

    /// The archive a file of the workspace is a member of: the key of the
    /// archive and the member's name. `None` for a file on disk, and for
    /// a member whose archive is not in the workspace.
    pub fn archive_of(&self, path: &Path) -> Option<(&Path, String)> {
        let key = self.key(path)?;
        let parent = self.key(key.parent()?)?;
        let (archive, state) = self.files.get_key_value(&parent)?;
        state.archive()?;
        let name = key.file_name()?.to_string_lossy().into_owned();
        Some((archive.as_path(), name))
    }

    /// The file on disk that holds `path`: the archive for a member of an
    /// archive of the workspace, and `path` itself for anything else.
    pub fn file_on_disk<'a>(&'a self, path: &'a Path) -> &'a Path {
        self.archive_of(path).map_or(path, |(archive, _)| archive)
    }

    /// `path` as seen from the scan root that contains it, which does not
    /// depend on where the workspace is checked out or on how the root was
    /// spelled. With nested roots the outermost one counts. A root that is
    /// the file itself leaves its file name; a path under no root is
    /// returned as given.
    pub fn root_relative(&self, path: &Path) -> PathBuf {
        if self.roots.is_empty() {
            return path.to_path_buf();
        }
        let id = identity(path);
        let relative = self
            .roots
            .iter()
            .filter_map(|root| id.strip_prefix(root).ok())
            .max_by_key(|rest| rest.as_os_str().len());
        match relative {
            Some(rest) if rest.as_os_str().is_empty() => {
                id.file_name().map_or_else(|| id.clone(), PathBuf::from)
            }
            Some(rest) => rest.to_path_buf(),
            None => path.to_path_buf(),
        }
    }
}

/// True if the path has an extension this tool can parse: a binary resource
/// file, a project-level XML file or an archive.
pub fn is_analyzable(path: &Path) -> bool {
    is_project_file(path) || is_rsrc_file(path) || archive::is_archive(path)
}

pub(crate) fn is_rsrc_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        RSRC_EXTENSIONS
            .iter()
            .any(|known| e.eq_ignore_ascii_case(known))
    })
}

/// All analyzable files under `roots`, normalized, sorted and deduplicated.
/// A root that is itself a file is included whatever its extension, and so
/// is a root that is the path of a member of an archive on disk. Archives
/// are listed as the files they are; their members are not.
///
/// A file reached through several spellings of its path (overlapping
/// roots such as `.` and the absolute current directory) is listed once,
/// under the spelling of the root that was given first.
pub fn discover(roots: &[PathBuf]) -> Vec<PathBuf> {
    discover_from(std::env::current_dir().ok().as_deref(), roots)
}

fn discover_from(base: Option<&Path>, roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut found: Vec<(PathBuf, usize, PathBuf)> = Vec::new();
    let mut add = |order: usize, path: &Path| {
        let path = normalize(path);
        found.push((identity_from(base, &path), order, path));
    };
    for (order, root) in roots.iter().enumerate() {
        if root.is_file() || archive::split(root).is_some() {
            add(order, root);
            continue;
        }
        let walker = WalkDir::new(root).follow_links(false).into_iter();
        let entries = walker.filter_entry(|e| {
            // Skip hidden directories such as `.git`, but never the root itself.
            e.depth() == 0 || !e.file_name().to_str().is_some_and(|n| n.starts_with('.'))
        });
        for entry in entries.flatten() {
            #[expect(
                clippy::filetype_is_file,
                reason = "only regular files are wanted: links are not followed, and reading a FIFO could block"
            )]
            let regular = entry.file_type().is_file();
            if regular && is_analyzable(entry.path()) {
                add(order, entry.path());
            }
        }
    }
    found.sort();
    found.dedup_by(|later, first| later.0 == first.0);
    let mut out: Vec<PathBuf> = found.into_iter().map(|(_, _, path)| path).collect();
    out.sort();
    out
}

/// The identity of a file: its lexically normalized absolute path. Two
/// spellings of one path have the same identity. Symbolic links are not
/// followed, so no file system access is needed.
pub fn identity(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return normalize(path);
    }
    identity_from(std::env::current_dir().ok().as_deref(), path)
}

/// [`identity`] against a known base directory, for callers that resolve
/// many paths. Without a base, relative paths stay relative, which is still
/// consistent within one process.
pub(crate) fn identity_from(base: Option<&Path>, path: &Path) -> PathBuf {
    match base {
        Some(base) if !path.is_absolute() => normalize(&base.join(path)),
        _ => normalize(path),
    }
}

/// Lexically normalize a path: drop `.` and resolve `..` where possible.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = matches!(out.components().next_back(), Some(Component::Normal(_)));
                if can_pop {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_lexically() {
        assert_eq!(normalize(Path::new("./a/./b/../c.vi")), Path::new("a/c.vi"));
        assert_eq!(normalize(Path::new("../a.vi")), Path::new("../a.vi"));
        assert_eq!(normalize(Path::new("a/../../b.vi")), Path::new("../b.vi"));
        assert_eq!(normalize(Path::new("/a/../../b.vi")), Path::new("/b.vi"));
    }

    #[test]
    fn identity_ignores_the_spelling() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(identity(Path::new("a/./b/../c.vi")), cwd.join("a/c.vi"));
        assert_eq!(identity(&cwd.join("a/c.vi")), cwd.join("a/c.vi"));
        let base = Path::new("base");
        assert_eq!(
            identity_from(Some(base), Path::new("../x.vi")),
            Path::new("x.vi")
        );
        assert_eq!(identity_from(None, Path::new("./x.vi")), Path::new("x.vi"));
    }

    #[test]
    fn one_file_under_two_spellings_is_one_entry() {
        let cwd = std::env::current_dir().unwrap();
        let mut workspace = Workspace::default();
        workspace.insert(Path::new("v/a.vi"), FileState::Failed("first".into()));
        workspace.insert(&cwd.join("v/./a.vi"), FileState::Failed("second".into()));
        // The first spelling stays the key; the state is the latest one.
        let keys: Vec<&PathBuf> = workspace.files().keys().collect();
        assert_eq!(keys, [Path::new("v/a.vi")]);
        let second = FileState::Failed("second".into());
        assert_eq!(workspace.get(&cwd.join("v/a.vi")), Some(&second));
        assert_eq!(workspace.get(Path::new("./v/a.vi")), Some(&second));
        assert_eq!(workspace.remove(&cwd.join("v/a.vi")), Some(second));
        assert!(workspace.files().is_empty() && workspace.keys.is_empty());
    }

    #[test]
    fn root_relative_strips_the_outermost_root() {
        let cwd = std::env::current_dir().unwrap();
        let mut workspace = Workspace {
            roots: vec![cwd.join("proj"), cwd.join("proj/sub"), cwd.join("lone.vi")],
            ..Workspace::default()
        };
        let rel = |w: &Workspace, p: &str| w.root_relative(Path::new(p));
        assert_eq!(rel(&workspace, "proj/sub/a.vi"), Path::new("sub/a.vi"));
        assert_eq!(
            workspace.root_relative(&cwd.join("proj/sub/a.vi")),
            Path::new("sub/a.vi")
        );
        assert_eq!(rel(&workspace, "lone.vi"), Path::new("lone.vi"));
        assert_eq!(rel(&workspace, "else/b.vi"), Path::new("else/b.vi"));
        workspace.roots.clear();
        assert_eq!(rel(&workspace, "proj/sub/a.vi"), Path::new("proj/sub/a.vi"));
    }

    #[test]
    fn recognizes_extensions() {
        assert!(is_analyzable(Path::new("x/Main.VI")));
        assert!(is_analyzable(Path::new("x/Type.ctl")));
        assert!(is_analyzable(Path::new("x/Proj.lvproj")));
        assert!(is_analyzable(Path::new("x/Lib.LVLIB")));
        assert!(is_analyzable(Path::new("x/Motor.lvclass")));
        assert!(is_analyzable(Path::new("x/error.llb")));
        assert!(!is_analyzable(Path::new("x/notes.xml")));
        assert!(!is_analyzable(Path::new("x/vi")));
    }
}
