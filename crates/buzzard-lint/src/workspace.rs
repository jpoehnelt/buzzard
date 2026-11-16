//! The set of analyzed files and their parse results.

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};

use buzzard_rsrc::{ViMetadata, RSRC_EXTENSIONS};
use rayon::prelude::*;
use walkdir::WalkDir;

use crate::dataflow::{self, Finding};
use crate::inspector::Inspection;
use crate::project::{is_project_file, ProjectFile};

/// What is kept in memory for one binary file: its metadata and the
/// results of the dataflow checks. The diagram itself is not retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    /// Version, flags, links and other facts decoded from the file.
    pub metadata: ViMetadata,
    /// Sorted findings of the dataflow checks; empty when the file has no
    /// block diagram that could be decoded.
    pub dataflow: Vec<Finding>,
}

impl From<Inspection> for Analysis {
    fn from(inspection: Inspection) -> Self {
        let dataflow = inspection
            .diagram
            .as_ref()
            .map(dataflow::analyze)
            .unwrap_or_default();
        Self {
            metadata: inspection.metadata,
            dataflow,
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
    /// The file could not be read or parsed; the message says why.
    Failed(String),
}

impl FileState {
    /// Read one file from disk. Project-level XML files are recognized by
    /// extension; everything else is read as a binary resource file.
    pub fn load(path: &Path) -> Self {
        if is_project_file(path) {
            return match ProjectFile::from_path(path) {
                Ok(project) => FileState::Project(Box::new(project)),
                Err(reason) => FileState::Failed(reason),
            };
        }
        Self::from_inspection(Inspection::from_path(path))
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
            FileState::Project(_) | FileState::Failed(_) => None,
        }
    }

    /// The parsed project, library or class; `None` for anything else.
    pub fn project(&self) -> Option<&ProjectFile> {
        match self {
            FileState::Project(project) => Some(project),
            FileState::Parsed(_) | FileState::Failed(_) => None,
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
        let keys = paths
            .iter()
            .map(|path| (identity_from(base.as_deref(), path), path.clone()))
            .collect();
        let files = paths
            .into_par_iter()
            .map(|path| {
                let state = FileState::load(&path);
                (path, state)
            })
            .collect();
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
    pub fn insert(&mut self, path: &Path, state: FileState) {
        let key = self.key(path).unwrap_or_else(|| {
            let key = normalize(path);
            self.keys.insert(identity(&key), key.clone());
            key
        });
        self.files.insert(key, state);
    }

    /// Drop a file, returning its last state.
    pub fn remove(&mut self, path: &Path) -> Option<FileState> {
        let key = self.key(path)?;
        self.keys.remove(&identity(&key));
        self.files.remove(&key)
    }

    /// Re-read one file from disk, dropping it if it no longer exists.
    pub fn refresh(&mut self, path: &Path) {
        if path.is_file() {
            self.insert(path, FileState::load(path));
        } else {
            self.remove(path);
        }
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
/// file or a project-level XML file.
pub fn is_analyzable(path: &Path) -> bool {
    is_project_file(path) || is_rsrc_file(path)
}

fn is_rsrc_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        RSRC_EXTENSIONS
            .iter()
            .any(|known| e.eq_ignore_ascii_case(known))
    })
}

/// All analyzable files under `roots`, normalized, sorted and deduplicated.
/// A root that is itself a file is included whatever its extension.
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
        if root.is_file() {
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
        assert!(!is_analyzable(Path::new("x/notes.xml")));
        assert!(!is_analyzable(Path::new("x/vi")));
    }
}
