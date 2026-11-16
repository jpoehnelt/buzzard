//! Protocol-independent analysis state behind the language server.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use buzzard_lint::workspace::{is_analyzable, normalize};
use buzzard_lint::{
    lint, summary, DependencyGraph, Diagnostic, FileState, LintConfig, ProjectFile, ProjectKind,
    Workspace,
};
use buzzard_rsrc::{FileKind, ViMetadata};

/// What kind of file a workspace symbol stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// A VI, or any other binary file that is not a control.
    Vi,
    /// A control (`.ctl`).
    Control,
    /// A `.lvclass` class.
    Class,
    /// A `.lvlib` library.
    Library,
    /// A `.lvproj` project.
    Project,
}

/// One result of a workspace symbol search: a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// File name, which is what the query is matched against.
    pub name: String,
    /// What kind of file it is.
    pub kind: SymbolKind,
    /// Path of the file, as the workspace stores it.
    pub path: PathBuf,
}

/// Diagnostics to publish for one file; an empty list clears them.
pub type Update = (PathBuf, Vec<Diagnostic>);

/// The analyzed workspace, its dependency graph, and the diagnostics last
/// published for each file.
#[derive(Debug, Default)]
pub struct Engine {
    roots: Vec<PathBuf>,
    config: LintConfig,
    workspace: Workspace,
    graph: DependencyGraph,
    published: BTreeMap<PathBuf, Vec<Diagnostic>>,
}

impl Engine {
    /// An engine for `roots` that has not scanned anything yet; see
    /// [`Engine::scan`].
    pub fn new(roots: Vec<PathBuf>, config: LintConfig) -> Self {
        Self {
            roots,
            config,
            ..Self::default()
        }
    }

    /// The roots given to [`Engine::new`].
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Scan every root from scratch and return the diagnostics to publish.
    pub fn scan(&mut self) -> Vec<Update> {
        self.workspace = Workspace::scan(&self.roots);
        self.relint()
    }

    /// True if a change to this path can affect analysis results.
    pub fn is_relevant(path: &Path) -> bool {
        is_analyzable(path)
    }

    /// Record freshly loaded states for changed files (`None` = deleted) and
    /// return only the diagnostics that differ from what was last published.
    /// Project-level XML files are handled like any other file: pass the
    /// state from [`FileState::load`].
    pub fn apply(&mut self, changes: Vec<(PathBuf, Option<FileState>)>) -> Vec<Update> {
        for (path, state) in changes {
            let path = normalize(&path);
            if let Some(state) = state {
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

    /// The metadata of the binary file at `path`, if it parsed.
    pub fn metadata(&self, path: &Path) -> Option<&ViMetadata> {
        self.workspace.get(path)?.metadata()
    }

    /// The parsed project, library or class at `path`.
    pub fn project(&self, path: &Path) -> Option<&ProjectFile> {
        self.workspace.get(path)?.project()
    }

    /// Workspace files listed as members of the project file at `path`.
    pub fn members(&self, path: &Path) -> Vec<PathBuf> {
        to_owned(self.graph.members(path))
    }

    /// Projects, libraries and classes that list `path` as a member.
    pub fn owners(&self, path: &Path) -> Vec<PathBuf> {
        to_owned(self.graph.owners(path))
    }

    /// The parent of the class at `path`, when it is in the workspace.
    pub fn parent_class(&self, path: &Path) -> Option<PathBuf> {
        self.graph.parent_class(path).map(Path::to_path_buf)
    }

    /// Classes in the workspace that inherit from the class at `path`.
    pub fn child_classes(&self, path: &Path) -> Vec<PathBuf> {
        to_owned(self.graph.child_classes(path))
    }

    /// Files that use the file at `path`.
    pub fn callers(&self, path: &Path) -> Vec<PathBuf> {
        to_owned(self.graph.callers(path))
    }

    /// Files that the file at `path` uses.
    pub fn callees(&self, path: &Path) -> Vec<PathBuf> {
        to_owned(self.graph.callees(path))
    }

    /// Markdown shown when hovering a file.
    pub fn hover(&self, path: &Path) -> Option<String> {
        let title = path.file_name()?.to_string_lossy();
        match self.workspace.get(path)? {
            FileState::Parsed(analysis) => {
                let meta = &analysis.metadata;
                let usage = (
                    self.graph.callers(path).len(),
                    self.graph.callees(path).len(),
                );
                Some(summary::to_markdown(&title, meta, Some(usage)))
            }
            FileState::Project(project) => {
                let usage = summary::ProjectUsage {
                    members_in_workspace: self.graph.members(path).len(),
                    owners: self.graph.owners(path).len(),
                    child_classes: self.graph.child_classes(path).len(),
                };
                Some(summary::project_to_markdown(&title, project, Some(usage)))
            }
            FileState::Failed(reason) => {
                Some(format!("### {title}\n\nCannot analyze file: {reason}\n"))
            }
        }
    }

    /// Fuzzy symbol search, best match first, ties broken by name then path.
    pub fn symbols(&self, query: &str, limit: usize) -> Vec<Symbol> {
        let files = self.workspace.files().iter().map(|(path, state)| {
            // A project file that failed to parse is still a project file.
            let project = state
                .project()
                .map(|p| p.kind)
                .or_else(|| ProjectKind::from_path(path));
            let kind = match (project, state.metadata().map(|m| m.kind)) {
                (Some(ProjectKind::Class), _) => SymbolKind::Class,
                (Some(ProjectKind::Library), _) => SymbolKind::Library,
                (Some(ProjectKind::Project), _) => SymbolKind::Project,
                (None, Some(FileKind::Control)) => SymbolKind::Control,
                (None, _) => SymbolKind::Vi,
            };
            (path, kind)
        });
        let mut scored: Vec<(i32, Symbol)> = files
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

fn to_owned(paths: Vec<&Path>) -> Vec<PathBuf> {
    paths.into_iter().map(Path::to_path_buf).collect()
}
