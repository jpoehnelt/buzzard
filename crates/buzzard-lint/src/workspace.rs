//! The set of analyzed files and their parse results.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use buzzard_rsrc::{ViMetadata, RSRC_EXTENSIONS};
use rayon::prelude::*;
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    Parsed(Box<ViMetadata>),
    /// The file could not be read or parsed; the message says why.
    Failed(String),
}

impl FileState {
    pub fn load(path: &Path) -> Self {
        match ViMetadata::from_path(path) {
            Ok(meta) => FileState::Parsed(Box::new(meta)),
            Err(err) => FileState::Failed(err.to_string()),
        }
    }

    pub fn metadata(&self) -> Option<&ViMetadata> {
        match self {
            FileState::Parsed(meta) => Some(meta),
            FileState::Failed(_) => None,
        }
    }
}

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

    pub fn files(&self) -> &BTreeMap<PathBuf, FileState> {
        &self.files
    }

    pub fn get(&self, path: &Path) -> Option<&FileState> {
        self.files.get(&normalize(path))
    }

    pub fn insert(&mut self, path: &Path, state: FileState) {
        self.files.insert(normalize(path), state);
    }

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

/// True if the path has an extension this tool can parse.
pub fn is_analyzable(path: &Path) -> bool {
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
            if entry.file_type().is_file() && is_analyzable(entry.path()) {
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
        assert!(!is_analyzable(Path::new("x/Proj.lvproj")));
        assert!(!is_analyzable(Path::new("x/vi")));
    }
}
