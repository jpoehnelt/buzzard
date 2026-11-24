//! The analysis state of the language server, independent of the protocol.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use buzzard_lint::archive::ArchiveFile;
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
    /// A `.lvlib` library, or an `.llb` archive.
    Library,
    /// A `.lvproj` project.
    Project,
}

/// One result of a workspace symbol search. Every result is a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// File name, which is what the query is matched against.
    pub name: String,
    /// What kind of file it is.
    pub kind: SymbolKind,
    /// Path of the file, as the workspace stores it. For a member of an
    /// `.llb` archive this is the path of the archive followed by the
    /// member's name, which is not a file an editor can open. See
    /// [`Snapshot::document`].
    pub path: PathBuf,
}

/// Diagnostics to publish for one file. An empty list clears them.
///
/// The path is always a file on disk. The findings about the members of
/// an `.llb` archive are published for the archive, each with the
/// member's name at the start of its message
/// ([`Diagnostic::on_disk`]). An editor cannot open a member, so it would
/// show the diagnostics of a member nowhere.
pub type Update = (PathBuf, Vec<Diagnostic>);

/// An immutable view of the analyzed workspace. Readers (hover, references,
/// symbols) hold one of these while the next one is being computed, so a
/// long scan never blocks a request.
#[derive(Debug, Default)]
pub struct Snapshot {
    workspace: Workspace,
    graph: DependencyGraph,
}

/// The analyzed workspace, its dependency graph, and the diagnostics last
/// published for each file.
#[derive(Debug, Default)]
pub struct Engine {
    roots: Vec<PathBuf>,
    config: LintConfig,
    snapshot: Arc<Snapshot>,
    published: BTreeMap<PathBuf, Vec<Diagnostic>>,
}

impl std::ops::Deref for Engine {
    type Target = Snapshot;

    fn deref(&self) -> &Snapshot {
        &self.snapshot
    }
}

impl Engine {
    /// An engine for `roots` that has not scanned anything yet. See
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

    /// The current state, cheap to clone and safe to read from any thread.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot)
    }

    /// Scan every root from scratch and return the diagnostics to publish.
    pub fn scan(&mut self) -> Vec<Update> {
        self.install(Workspace::scan(&self.roots))
    }

    /// True if a change to this path can affect analysis results.
    pub fn is_relevant(path: &Path) -> bool {
        is_analyzable(path)
    }

    /// Record freshly loaded states for changed files and return only the
    /// diagnostics that differ from what was last published. A state of
    /// `None` means that the file was deleted. For a project-level XML file,
    /// pass the state from [`FileState::load`], as for any other file. For
    /// an `.llb` archive, pass every entry of [`FileState::load_all`], the
    /// archive first, because storing an archive drops the members it had
    /// before. Deleting an archive drops them too.
    ///
    /// The next state is built beside the current one and swapped in at the
    /// end. Snapshots handed out earlier are not affected.
    pub fn apply(&mut self, changes: Vec<(PathBuf, Option<FileState>)>) -> Vec<Update> {
        let mut workspace = self.snapshot.workspace.clone();
        for (path, state) in changes {
            let path = normalize(&path);
            if let Some(state) = state {
                workspace.insert(&path, state);
            } else {
                workspace.remove(&path);
            }
        }
        self.install(workspace)
    }

    fn install(&mut self, workspace: Workspace) -> Vec<Update> {
        let graph = DependencyGraph::build(&workspace);
        let mut current: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();
        for diagnostic in lint(&workspace, &graph, &self.config) {
            let (diagnostic, _) = diagnostic.on_disk(&workspace);
            current
                .entry(diagnostic.path.clone())
                .or_default()
                .push(diagnostic);
        }
        for diagnostics in current.values_mut() {
            diagnostics.sort();
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
        self.snapshot = Arc::new(Snapshot { workspace, graph });
        updates
    }
}

impl Snapshot {
    /// True if `path` is one of the analyzed files.
    pub fn contains(&self, path: &Path) -> bool {
        self.workspace.get(path).is_some()
    }

    /// Every analyzed file, sorted.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.workspace.files().keys().map(PathBuf::as_path)
    }

    /// Analyzed files strictly below the directory `dir`, sorted.
    pub fn paths_under(&self, dir: &Path) -> Vec<PathBuf> {
        let dir = normalize(dir);
        // Paths order by component, so everything below `dir` sits in one
        // run right after it. The rest of the workspace is not searched.
        let after = (Bound::Excluded(dir.as_path()), Bound::Unbounded);
        self.workspace
            .files()
            .range::<Path, _>(after)
            .map(|(path, _)| path)
            .take_while(|path| path.starts_with(&dir))
            .cloned()
            .collect()
    }

    /// The file an editor can open for `path`: the archive for a member
    /// of an `.llb` archive, and `path` itself for anything else.
    pub fn document(&self, path: &Path) -> PathBuf {
        self.workspace.file_on_disk(path)
    }

    /// True if `path` is a member of an `.llb` archive of the workspace.
    pub fn is_member(&self, path: &Path) -> bool {
        self.workspace.archive_of(path).is_some()
    }

    /// The directory of the `.llb` archive at `path`.
    pub fn archive(&self, path: &Path) -> Option<&ArchiveFile> {
        self.workspace.get(path)?.archive()
    }

    /// The metadata of the binary file at `path`, if it parsed.
    pub fn metadata(&self, path: &Path) -> Option<&ViMetadata> {
        self.workspace.get(path)?.metadata()
    }

    /// The parsed project, library, or class at `path`.
    pub fn project(&self, path: &Path) -> Option<&ProjectFile> {
        self.workspace.get(path)?.project()
    }

    /// Workspace files listed as members of the project file at `path`.
    pub fn members(&self, path: &Path) -> Vec<PathBuf> {
        to_owned(self.graph.members(path))
    }

    /// Projects, libraries, and classes that list `path` as a member.
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
            FileState::Archive(archive) => Some(summary::archive_to_markdown(&title, archive)),
            FileState::Failed(reason) => {
                // A file name is data and is printed on one line, as the
                // other summaries print it.
                Some(format!(
                    "### {}\n\nCannot analyze file: {reason}\n",
                    buzzard_lint::printable::text(&title)
                ))
            }
        }
    }

    /// A fuzzy search for symbols. The best match comes first, and ties are
    /// ordered by name, then path.
    pub fn symbols(&self, query: &str, limit: usize) -> Vec<Symbol> {
        let files = self.workspace.files().iter().map(|(path, state)| {
            // A project file that failed to parse is still a project file.
            let project = state
                .project()
                .map(|p| p.kind)
                .or_else(|| ProjectKind::from_path(path));
            let kind = match (project, state.metadata().map(|m| m.kind)) {
                _ if state.archive().is_some() => SymbolKind::Library,
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
