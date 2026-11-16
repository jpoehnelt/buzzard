//! Protocol-independent analysis state behind the language server.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use buzzard_lint::workspace::{is_analyzable, normalize};
use buzzard_lint::{lint, summary, DependencyGraph, Diagnostic, FileState, LintConfig, Workspace};
use buzzard_rsrc::{FileKind, ViMetadata};
use walkdir::WalkDir;

/// Project-level text files that are listed as symbols but not parsed.
const PROJECT_EXTENSIONS: [&str; 3] = ["lvproj", "lvlib", "lvclass"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Vi,
    Control,
    Class,
    Library,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub path: PathBuf,
}

/// Diagnostics to publish for one file; an empty list clears them.
pub type Update = (PathBuf, Vec<Diagnostic>);

#[derive(Debug, Default)]
pub struct Engine {
    roots: Vec<PathBuf>,
    config: LintConfig,
    workspace: Workspace,
    graph: DependencyGraph,
    project_files: BTreeSet<PathBuf>,
    published: BTreeMap<PathBuf, Vec<Diagnostic>>,
}

impl Engine {
    pub fn new(roots: Vec<PathBuf>, config: LintConfig) -> Self {
        Self {
            roots,
            config,
            ..Self::default()
        }
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Scan every root from scratch and return the diagnostics to publish.
    pub fn scan(&mut self) -> Vec<Update> {
        self.workspace = Workspace::scan(&self.roots);
        self.project_files = self
            .roots
            .iter()
            .flat_map(|root| WalkDir::new(root).into_iter().flatten())
            .filter(|e| e.file_type().is_file() && is_project_file(e.path()))
            .map(|e| normalize(e.path()))
            .collect();
        self.relint()
    }

    /// True if a change to this path can affect analysis results.
    pub fn is_relevant(path: &Path) -> bool {
        is_analyzable(path) || is_project_file(path)
    }

    /// Record freshly loaded states for changed files (`None` = deleted) and
    /// return only the diagnostics that differ from what was last published.
    pub fn apply(&mut self, changes: Vec<(PathBuf, Option<FileState>)>) -> Vec<Update> {
        for (path, state) in changes {
            let path = normalize(&path);
            if is_project_file(&path) {
                if path.is_file() {
                    self.project_files.insert(path);
                } else {
                    self.project_files.remove(&path);
                }
            } else if let Some(state) = state {
                self.workspace.insert(&path, state);
            } else {
                self.workspace.remove(&path);
            }
        }
        self.relint()
    }

    fn relint(&mut self) -> Vec<Update> {
        self.graph = DependencyGraph::build(&self.workspace);
        let mut current: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();
        for diagnostic in lint(&self.workspace, &self.graph, &self.config) {
            current
                .entry(diagnostic.path.clone())
                .or_default()
                .push(diagnostic);
        }
        let mut updates = Vec::new();
        for (path, diagnostics) in &current {
            if self.published.get(path) != Some(diagnostics) {
                updates.push((path.clone(), diagnostics.clone()));
            }
        }
        for path in self.published.keys() {
            if !current.contains_key(path) {
                updates.push((path.clone(), Vec::new()));
            }
        }
        updates.sort();
        self.published = current;
        updates
    }

    pub fn metadata(&self, path: &Path) -> Option<&ViMetadata> {
        self.workspace.get(path)?.metadata()
    }

    pub fn callers(&self, path: &Path) -> Vec<PathBuf> {
        self.graph
            .callers(path)
            .into_iter()
            .map(Path::to_path_buf)
            .collect()
    }

    pub fn callees(&self, path: &Path) -> Vec<PathBuf> {
        self.graph
            .callees(path)
            .into_iter()
            .map(Path::to_path_buf)
            .collect()
    }

    /// Markdown shown when hovering a file.
    pub fn hover(&self, path: &Path) -> Option<String> {
        let title = path.file_name()?.to_string_lossy();
        match self.workspace.get(path)? {
            FileState::Parsed(meta) => {
                let usage = (
                    self.graph.callers(path).len(),
                    self.graph.callees(path).len(),
                );
                Some(summary::to_markdown(&title, meta, Some(usage)))
            }
            FileState::Failed(reason) => {
                Some(format!("### {title}\n\nCannot analyze file: {reason}\n"))
            }
        }
    }

    /// Fuzzy symbol search, best match first, ties broken by name then path.
    pub fn symbols(&self, query: &str, limit: usize) -> Vec<Symbol> {
        let parsed = self.workspace.files().iter().map(|(path, state)| {
            let kind = match state.metadata().map(|m| m.kind) {
                Some(FileKind::Control) => SymbolKind::Control,
                _ => SymbolKind::Vi,
            };
            (path, kind)
        });
        let project = self.project_files.iter().map(|path| {
            let kind = match path.extension().and_then(|e| e.to_str()) {
                Some(e) if e.eq_ignore_ascii_case("lvclass") => SymbolKind::Class,
                Some(e) if e.eq_ignore_ascii_case("lvlib") => SymbolKind::Library,
                _ => SymbolKind::Project,
            };
            (path, kind)
        });
        let mut scored: Vec<(i32, Symbol)> = parsed
            .chain(project)
            .filter_map(|(path, kind)| {
                let name = path.file_name()?.to_string_lossy().into_owned();
                let score = crate::fuzzy::score(query, &name)?;
                Some((
                    score,
                    Symbol {
                        name,
                        kind,
                        path: path.clone(),
                    },
                ))
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.name.cmp(&b.1.name))
                .then_with(|| a.1.path.cmp(&b.1.path))
        });
        scored
            .into_iter()
            .take(limit)
            .map(|(_, symbol)| symbol)
            .collect()
    }
}

fn is_project_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        PROJECT_EXTENSIONS
            .iter()
            .any(|known| e.eq_ignore_ascii_case(known))
    })
}
