//! The set of analyzed files and their parse results.

use std::collections::BTreeMap;
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

/// Every analyzed file and its state, keyed by normalized path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Workspace {
    files: BTreeMap<PathBuf, FileState>,
}

impl Workspace {
    /// Discover and parse every analyzable file under `roots` in parallel.
    pub fn scan(roots: &[PathBuf]) -> Self {
        let files = discover(roots)
            .into_par_iter()
            .map(|path| {
                let state = FileState::load(&path);
                (path, state)
            })
            .collect();
        Self { files }
    }

    /// Every file and its state, in path order.
    pub fn files(&self) -> &BTreeMap<PathBuf, FileState> {
        &self.files
    }

    /// The state of one file. `path` is normalized first, like every key.
    pub fn get(&self, path: &Path) -> Option<&FileState> {
        self.files.get(&normalize(path))
    }

    /// Add a file or replace its state.
    pub fn insert(&mut self, path: &Path, state: FileState) {
        self.files.insert(normalize(path), state);
    }

    /// Drop a file, returning its last state.
    pub fn remove(&mut self, path: &Path) -> Option<FileState> {
        self.files.remove(&normalize(path))
    }

    /// Re-read one file from disk, dropping it if it no longer exists.
    pub fn refresh(&mut self, path: &Path) {
        if path.is_file() {
            self.insert(path, FileState::load(path));
        } else {
            self.remove(path);
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
pub fn discover(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        if root.is_file() {
            out.push(normalize(root));
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
                out.push(normalize(entry.path()));
            }
        }
    }
    out.sort();
    out.dedup();
    out
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
