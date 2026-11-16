//! Directed graph of file-to-file references inside a workspace.
//!
//! Edges come in three kinds (see [`EdgeKind`]). "Dependency" below means a
//! `Uses` or `Inherits` edge; `Contains` edges describe ownership and never
//! take part in cycle detection, because a member routinely refers back to
//! the library or class that owns it.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Component, Path, PathBuf};

use buzzard_rsrc::{LvPath, PathKind};
use encoding_rs::Encoding;
use petgraph::algo::tarjan_scc;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{EdgeFiltered, EdgeRef};
use petgraph::Direction;
use serde::Serialize;

use crate::project::ParentClass;
use crate::workspace::{identity_from, normalize, Workspace};

/// What an edge from `a` to `b` means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// `a` references `b` in its link info (caller -> callee).
    Uses,
    /// `a` is a project, library or class that lists `b` as a member.
    Contains,
    /// `a` is a class whose parent class is `b`.
    Inherits,
}

impl EdgeKind {
    fn is_dependency(self) -> bool {
        matches!(self, EdgeKind::Uses | EdgeKind::Inherits)
    }
}

/// Whether two paths that differ only in letter case name the same file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CaseSensitivity {
    /// Only an exact match counts (Linux and most other Unix systems).
    #[default]
    Sensitive,
    /// When no exact match exists, a match that differs only in case counts
    /// (Windows and macOS), provided it is the only one.
    Insensitive,
}

impl CaseSensitivity {
    /// The convention of the operating system this program was built for.
    pub fn host() -> Self {
        if cfg!(any(windows, target_os = "macos")) {
            CaseSensitivity::Insensitive
        } else {
            CaseSensitivity::Sensitive
        }
    }
}

/// Code pages a file name may have been saved in. Only `PTH2` paths are
/// UTF-8; older forms hold the bytes of the saving machine's code page.
static CODE_PAGES: [&Encoding; 8] = [
    encoding_rs::WINDOWS_1252,
    encoding_rs::SHIFT_JIS,
    encoding_rs::GBK,
    encoding_rs::BIG5,
    encoding_rs::EUC_KR,
    encoding_rs::WINDOWS_1251,
    encoding_rs::WINDOWS_1250,
    encoding_rs::MACINTOSH,
];

/// Text without its letter case, for comparing names of file systems that
/// ignore case.
pub(crate) fn fold(text: &str) -> String {
    text.to_lowercase()
}

/// The single node in `nodes`, if there is exactly one.
fn only(nodes: Option<&Vec<NodeIndex>>) -> Option<NodeIndex> {
    match nodes?.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// Finds workspace files by path or by file name.
#[derive(Debug, Default)]
struct Lookup {
    /// Directory against which relative paths get their identity.
    base: Option<PathBuf>,
    by_path: HashMap<PathBuf, NodeIndex>,
    by_name: HashMap<String, Vec<NodeIndex>>,
    /// Case-folded variants of the two maps above, kept only when case
    /// does not distinguish files.
    folded: Option<Folded>,
}

#[derive(Debug, Default)]
struct Folded {
    by_path: HashMap<String, Vec<NodeIndex>>,
    by_name: HashMap<String, Vec<NodeIndex>>,
}

impl Lookup {
    fn new(case: CaseSensitivity) -> Self {
        Self {
            base: std::env::current_dir().ok(),
            folded: (case == CaseSensitivity::Insensitive).then(Folded::default),
            ..Self::default()
        }
    }

    fn add(&mut self, path: &Path, node: NodeIndex) {
        let id = identity_from(self.base.as_deref(), path);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        if let Some(folded) = &mut self.folded {
            let key = fold(&id.to_string_lossy());
            folded.by_path.entry(key).or_default().push(node);
            if let Some(name) = &name {
                folded.by_name.entry(fold(name)).or_default().push(node);
            }
        }
        if let Some(name) = name {
            self.by_name.entry(name).or_default().push(node);
        }
        self.by_path.insert(id, node);
    }

    /// The file at `path`, however the path is spelled.
    fn path(&self, path: &Path) -> Option<NodeIndex> {
        let id = identity_from(self.base.as_deref(), path);
        if let Some(&node) = self.by_path.get(&id) {
            return Some(node);
        }
        let folded = self.folded.as_ref()?;
        only(folded.by_path.get(&fold(&id.to_string_lossy())))
    }

    /// The file called `name`, when exactly one file has that name.
    fn name(&self, name: &str) -> Option<NodeIndex> {
        if let Some(nodes) = self.by_name.get(name) {
            return only(Some(nodes));
        }
        let folded = self.folded.as_ref()?;
        only(folded.by_name.get(&fold(name)))
    }

    /// The workspace file a stored link refers to, as seen from the file
    /// `from` at `from_path`.
    fn link(
        &self,
        from: NodeIndex,
        from_path: &Path,
        link: &LvPath,
        name: Option<&str>,
    ) -> Option<NodeIndex> {
        if let Some(found) = self.link_as_stored(from, from_path, link, name) {
            return Some(found);
        }
        // The text may be the bytes of a legacy code page shown one byte
        // per character. Accept a reading only when every code page that
        // yields a match agrees on the file.
        let mut exact = BTreeSet::new();
        let mut named = BTreeSet::new();
        for (path, name) in recode(link, name) {
            exact.extend(resolve(from_path, &path).and_then(|p| self.path(&p)));
            if can_be_stale(&path) {
                let name = name.as_deref().or_else(|| path.file_name());
                named.extend(name.and_then(|n| self.name(n)).filter(|&to| to != from));
            }
        }
        let candidates = if exact.is_empty() { named } else { exact };
        match candidates.len() {
            1 => candidates.into_iter().next(),
            _ => None,
        }
    }

    /// The workspace file that is the parent of the class `from`.
    ///
    /// Unlike a call to an installed VI, a parent class stored by its
    /// installed location is still looked up by name: class hierarchies
    /// are developed in one workspace and used from another, and class
    /// names are distinctive. No class is its own parent, whatever its
    /// name suggests.
    fn parent(&self, from: NodeIndex, from_path: &Path, parent: &ParentClass) -> Option<NodeIndex> {
        self.link(from, from_path, &parent.link, Some(&parent.name))
            .or_else(|| self.name(&parent.name).filter(|&to| to != from))
    }

    fn link_as_stored(
        &self,
        from: NodeIndex,
        from_path: &Path,
        link: &LvPath,
        name: Option<&str>,
    ) -> Option<NodeIndex> {
        if let Some(exact) = resolve(from_path, link).and_then(|p| self.path(&p)) {
            return Some(exact);
        }
        // A stale path (moved checkout, absolute path from another machine)
        // still identifies its target when the file name is unique. A
        // symbolic path such as `<vilib>/...` is never stale: it names a
        // file of the installation, not a workspace file that happens to
        // share its name. Nothing references itself through a stale path.
        if !can_be_stale(link) {
            return None;
        }
        self.name(name.or_else(|| link.file_name())?)
            .filter(|&to| to != from)
    }
}

/// True for a link whose stored location may simply be out of date: a
/// relative path, or one pinned to a machine's file system layout.
fn can_be_stale(link: &LvPath) -> bool {
    !link.is_empty() && (link.kind == PathKind::Relative || link.is_machine_absolute())
}

/// The bytes behind text that was decoded one byte per character, if the
/// text has that shape and is not plain ASCII.
fn code_page_bytes(text: &str) -> Option<Vec<u8>> {
    let bytes: Vec<u8> = text
        .chars()
        .map(|c| u8::try_from(u32::from(c)).ok())
        .collect::<Option<_>>()?;
    (!bytes.is_ascii()).then_some(bytes)
}

/// `text` read as `encoding`, or `None` if its bytes are not valid in it.
/// Text that cannot be code page bytes is returned unchanged.
fn recode_text(text: &str, encoding: &'static Encoding) -> Option<String> {
    let Some(bytes) = code_page_bytes(text) else {
        return Some(text.to_owned());
    };
    encoding
        .decode_without_bom_handling_and_without_replacement(&bytes)
        .map(Cow::into_owned)
}

/// Every distinct reading of a link under the candidate code pages. Empty
/// unless some part of the link looks like undecoded code page bytes.
fn recode(link: &LvPath, name: Option<&str>) -> Vec<(LvPath, Option<String>)> {
    let suspect = |text: &str| code_page_bytes(text).is_some();
    if !link.components.iter().any(|c| suspect(c)) && !name.is_some_and(suspect) {
        return Vec::new();
    }
    let mut out: Vec<(LvPath, Option<String>)> = Vec::new();
    for encoding in CODE_PAGES {
        let components: Option<Vec<String>> = link
            .components
            .iter()
            .map(|c| recode_text(c, encoding))
            .collect();
        let name = match name {
            Some(name) => recode_text(name, encoding).map(Some),
            None => Some(None),
        };
        let (Some(components), Some(name)) = (components, name) else {
            continue;
        };
        let reading = (
            LvPath {
                kind: link.kind,
                components,
            },
            name,
        );
        if !out.contains(&reading) {
            out.push(reading);
        }
    }
    out
}

/// Every place on disk a link stored in the file `from` may name: the
/// location as stored, then its readings under the legacy code pages the
/// graph tries. Empty for a link that does not resolve (see [`resolve`]).
pub fn candidate_paths(from: &Path, link: &LvPath, name: Option<&str>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = resolve(from, link).into_iter().collect();
    for (reading, _) in recode(link, name) {
        if let Some(path) = resolve(from, &reading) {
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

/// The readings of path components under the legacy code pages the graph
/// tries, without the components as stored. Empty unless a component
/// looks like undecoded code page bytes.
pub(crate) fn code_page_readings(components: &[String]) -> Vec<Vec<String>> {
    let link = LvPath {
        kind: PathKind::Absolute,
        components: components.to_vec(),
    };
    recode(&link, None)
        .into_iter()
        .map(|(reading, _)| reading.components)
        .filter(|reading| reading != components)
        .collect()
}

/// Every file of a workspace and the edges between them.
#[derive(Debug, Default)]
pub struct DependencyGraph {
    /// Nodes carry the path as given, which is what callers see.
    graph: DiGraph<PathBuf, EdgeKind>,
    lookup: Lookup,
}

impl DependencyGraph {
    /// Build the graph with the case convention of this operating system.
    pub fn build(workspace: &Workspace) -> Self {
        Self::build_with(workspace, CaseSensitivity::host())
    }

    /// Build the graph with an explicit case convention.
    pub fn build_with(workspace: &Workspace, case: CaseSensitivity) -> Self {
        let mut graph = DiGraph::new();
        let mut lookup = Lookup::new(case);
        let mut nodes = Vec::with_capacity(workspace.files().len());
        for path in workspace.files().keys() {
            let node = graph.add_node(path.clone());
            lookup.add(path, node);
            nodes.push(node);
        }

        for ((path, state), &from) in workspace.files().iter().zip(&nodes) {
            let mut targets: BTreeSet<(NodeIndex, EdgeKind)> = BTreeSet::new();
            if let Some(project) = state.project() {
                for member in project.members.iter().filter(|m| !m.dependency) {
                    let target = member.path.as_ref().and_then(|p| lookup.path(p));
                    targets.extend(target.map(|to| (to, EdgeKind::Contains)));
                }
                if let Some(parent) = &project.parent_class {
                    let target = lookup.parent(from, path, parent);
                    targets.extend(target.map(|to| (to, EdgeKind::Inherits)));
                }
            }
            let links = state.metadata().map_or(&[][..], |meta| &meta.links);
            for link in links {
                let target = lookup.link(from, path, &link.path, link.name());
                targets.extend(target.map(|to| (to, EdgeKind::Uses)));
            }
            for (to, kind) in targets {
                graph.add_edge(from, to, kind);
            }
        }
        Self { graph, lookup }
    }

    /// The workspace file that a link stored in the file `from` refers to,
    /// decided the way the `Uses` edges are: by the stored location, or by
    /// the file name when the location is stale and the name is unique.
    /// `None` when `from` is not in the workspace or the link names no
    /// workspace file.
    pub fn link_target(&self, from: &Path, link: &LvPath, name: Option<&str>) -> Option<&Path> {
        let node = self.lookup.path(from)?;
        let to = self.lookup.link(node, &self.graph[node], link, name)?;
        Some(self.graph[to].as_path())
    }

    /// The workspace file that is the parent of the class at `from`,
    /// decided the way the `Inherits` edges are.
    pub fn parent_target(&self, from: &Path, parent: &ParentClass) -> Option<&Path> {
        let node = self.lookup.path(from)?;
        let to = self.lookup.parent(node, &self.graph[node], parent)?;
        Some(self.graph[to].as_path())
    }

    fn neighbors(&self, path: &Path, direction: Direction, kind: EdgeKind) -> Vec<&Path> {
        let Some(node) = self.lookup.path(path) else {
            return Vec::new();
        };
        let mut out: Vec<&Path> = self
            .graph
            .edges_directed(node, direction)
            .filter(|e| *e.weight() == kind)
            .map(|e| match direction {
                Direction::Outgoing => e.target(),
                Direction::Incoming => e.source(),
            })
            .map(|n| self.graph[n].as_path())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Files that reference `path` (`Uses` edges only).
    pub fn callers(&self, path: &Path) -> Vec<&Path> {
        self.neighbors(path, Direction::Incoming, EdgeKind::Uses)
    }

    /// Files that `path` references (`Uses` edges only).
    pub fn callees(&self, path: &Path) -> Vec<&Path> {
        self.neighbors(path, Direction::Outgoing, EdgeKind::Uses)
    }

    /// Workspace files that the project, library or class at `path` lists
    /// as members. Items under a project's `Dependencies` node are not
    /// members.
    pub fn members(&self, path: &Path) -> Vec<&Path> {
        self.neighbors(path, Direction::Outgoing, EdgeKind::Contains)
    }

    /// Projects, libraries and classes that list `path` as a member.
    pub fn owners(&self, path: &Path) -> Vec<&Path> {
        self.neighbors(path, Direction::Incoming, EdgeKind::Contains)
    }

    /// The parent of the class at `path`, when it is in the workspace.
    pub fn parent_class(&self, path: &Path) -> Option<&Path> {
        self.neighbors(path, Direction::Outgoing, EdgeKind::Inherits)
            .into_iter()
            .next()
    }

    /// Classes in the workspace whose parent is the class at `path`.
    pub fn child_classes(&self, path: &Path) -> Vec<&Path> {
        self.neighbors(path, Direction::Incoming, EdgeKind::Inherits)
    }

    /// Successors along dependency edges, in no particular order.
    fn dependencies(&self, node: NodeIndex) -> impl Iterator<Item = NodeIndex> + '_ {
        self.graph
            .edges(node)
            .filter(|e| e.weight().is_dependency())
            .map(|e| e.target())
    }

    /// Every group of mutually dependent files, with one concrete cycle
    /// through the group's first member. Groups and members are sorted.
    #[expect(
        clippy::missing_panics_doc,
        reason = "the only `expect` is on a strongly connected component, which is never empty"
    )]
    pub fn cycles(&self) -> Vec<Cycle> {
        let dependencies = EdgeFiltered::from_fn(&self.graph, |e| e.weight().is_dependency());
        let mut out: Vec<Cycle> = tarjan_scc(&dependencies)
            .into_iter()
            .filter(|scc| scc.len() > 1 || self.dependencies(scc[0]).any(|n| n == scc[0]))
            .map(|scc| {
                let members: BTreeMap<&Path, NodeIndex> =
                    scc.iter().map(|&n| (self.graph[n].as_path(), n)).collect();
                let start = *members.values().next().expect("an SCC is never empty");
                let allowed: BTreeSet<NodeIndex> = scc.iter().copied().collect();
                Cycle {
                    members: members.keys().map(|p| p.to_path_buf()).collect(),
                    example: self
                        .shortest_cycle(start, &allowed)
                        .into_iter()
                        .map(|n| self.graph[n].clone())
                        .collect(),
                }
            })
            .collect();
        out.sort();
        out
    }

    /// Shortest path from `start` back to itself within `allowed`, choosing
    /// the smallest path name at every tie so the result is stable.
    fn shortest_cycle(&self, start: NodeIndex, allowed: &BTreeSet<NodeIndex>) -> Vec<NodeIndex> {
        let successors = |n: NodeIndex| {
            let mut next: Vec<NodeIndex> = self
                .dependencies(n)
                .filter(|m| allowed.contains(m))
                .collect();
            next.sort_by(|a, b| self.graph[*a].cmp(&self.graph[*b]));
            next.dedup();
            next
        };
        let mut parent: HashMap<NodeIndex, NodeIndex> = HashMap::new();
        let mut queue = VecDeque::from([start]);
        while let Some(node) = queue.pop_front() {
            for next in successors(node) {
                if next == start {
                    let mut cycle = vec![node];
                    let mut at = node;
                    while at != start {
                        at = parent[&at];
                        cycle.push(at);
                    }
                    cycle.reverse();
                    return cycle;
                }
                if let std::collections::hash_map::Entry::Vacant(slot) = parent.entry(next) {
                    slot.insert(node);
                    queue.push_back(next);
                }
            }
        }
        vec![start]
    }

    /// Number of files in the graph, which is every file in the workspace.
    pub fn file_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Number of edges of every kind.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Every edge as `(from, to, kind)`, sorted.
    pub fn edges(&self) -> Vec<(&Path, &Path, EdgeKind)> {
        let mut out: Vec<(&Path, &Path, EdgeKind)> = self
            .graph
            .edge_references()
            .map(|e| {
                (
                    self.graph[e.source()].as_path(),
                    self.graph[e.target()].as_path(),
                    *e.weight(),
                )
            })
            .collect();
        out.sort();
        out
    }
}

/// A circular dependency, as found by [`DependencyGraph::cycles`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cycle {
    /// Every file in the strongly connected group, sorted.
    pub members: Vec<PathBuf>,
    /// One concrete cycle: each file uses or inherits from the next, the
    /// last from the first.
    pub example: Vec<PathBuf>,
}

/// Turn a stored link path into a file system path, as seen from the file
/// that contains the link. Symbolic paths such as `<vilib>` do not resolve.
pub fn resolve(from: &Path, link: &LvPath) -> Option<PathBuf> {
    if link.is_empty() || link.symbolic_root().is_some() {
        return None;
    }
    let mut out = match link.kind {
        // Relative links are relative to the referencing file itself, so
        // the first empty ("parent") component steps to its directory.
        PathKind::Relative => normalize(from),
        PathKind::Absolute => PathBuf::new(),
        PathKind::Unc | PathKind::NotAPath => return None,
    };
    for (i, component) in link.components.iter().enumerate() {
        if component.is_empty() {
            // Only a named component can be stepped out of. `pop` would
            // also remove a trailing `..`, which cancels a step up instead
            // of adding one, and it would remove the root.
            let named = matches!(out.components().next_back(), Some(Component::Normal(_)));
            if named {
                out.pop();
            } else if !out.has_root() {
                out.push("..");
            }
        } else if i == 0 && link.kind == PathKind::Absolute {
            if component.len() == 1 && cfg!(windows) {
                out.push(format!("{component}:\\"));
            } else {
                out = PathBuf::from("/");
                out.push(component);
            }
        } else {
            out.push(component);
        }
    }
    Some(normalize(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_to_the_referencing_file() {
        let from = Path::new("proj/sub/main.vi");
        assert_eq!(
            resolve(from, &LvPath::sibling("a.vi")).unwrap(),
            Path::new("proj/sub/a.vi")
        );
        let up = LvPath {
            kind: PathKind::Relative,
            components: vec![String::new(), String::new(), "lib".into(), "b.vi".into()],
        };
        assert_eq!(resolve(from, &up).unwrap(), Path::new("proj/lib/b.vi"));
    }

    #[test]
    fn symbolic_and_empty_paths_do_not_resolve() {
        let from = Path::new("main.vi");
        assert_eq!(resolve(from, &LvPath::absolute(&["<vilib>", "a.vi"])), None);
        assert_eq!(resolve(from, &LvPath::absolute(&[])), None);
    }

    #[test]
    fn parent_steps_never_cancel_the_parent_steps_of_the_referencing_path() {
        // ../main.vi linking to ../../lib/x.vi is three levels up from the
        // starting directory, not `lib/x.vi` inside it.
        let up = LvPath {
            kind: PathKind::Relative,
            components: vec![String::new(), String::new(), "lib".into(), "x.vi".into()],
        };
        assert_eq!(
            resolve(Path::new("../main.vi"), &up).unwrap(),
            Path::new("../../lib/x.vi")
        );
        assert_eq!(
            resolve(Path::new("../../main.vi"), &up).unwrap(),
            Path::new("../../../lib/x.vi")
        );
        assert_eq!(
            resolve(Path::new("main.vi"), &up).unwrap(),
            Path::new("../lib/x.vi")
        );
        // A referencing path that is not normalized behaves like its
        // normalized form.
        assert_eq!(
            resolve(Path::new("a/../../main.vi"), &up).unwrap(),
            Path::new("../../lib/x.vi")
        );
    }

    #[cfg(unix)]
    #[test]
    fn parent_steps_stop_at_the_root() {
        let up = LvPath {
            kind: PathKind::Relative,
            components: vec![String::new(), String::new(), String::new(), "x.vi".into()],
        };
        assert_eq!(
            resolve(Path::new("/a/main.vi"), &up).unwrap(),
            Path::new("/x.vi")
        );
    }

    #[cfg(windows)]
    #[test]
    fn resolves_windows_drive_letter() {
        let link = LvPath::absolute(&["C", "Users", "me", "a.vi"]);
        assert_eq!(
            resolve(Path::new("main.vi"), &link).unwrap(),
            Path::new(r"C:\Users\me\a.vi")
        );
        // The result is a full path: it has a drive and a root.
        assert!(resolve(Path::new("main.vi"), &link).unwrap().is_absolute());
        // A lower-case drive letter is kept as stored.
        let lower = LvPath::absolute(&["d", "a.vi"]);
        assert_eq!(
            resolve(Path::new(r"C:\proj\main.vi"), &lower).unwrap(),
            Path::new(r"d:\a.vi")
        );
    }

    #[cfg(windows)]
    #[test]
    fn resolves_windows_relative_and_parent_steps() {
        let from = Path::new(r"C:\proj\sub\main.vi");
        assert_eq!(
            resolve(from, &LvPath::sibling("a.vi")).unwrap(),
            Path::new(r"C:\proj\sub\a.vi")
        );
        let up = LvPath {
            kind: PathKind::Relative,
            components: vec![String::new(), String::new(), "lib".into(), "b.vi".into()],
        };
        assert_eq!(resolve(from, &up).unwrap(), Path::new(r"C:\proj\lib\b.vi"));
        // Stepping up never leaves the drive.
        let far = LvPath {
            kind: PathKind::Relative,
            components: vec![String::new(); 6],
        };
        assert_eq!(resolve(from, &far).unwrap(), Path::new(r"C:\"));
    }

    #[test]
    fn reads_code_page_bytes_back() {
        // "テスト.vi" in Shift-JIS, one byte per character.
        let stored: String = [0x83u8, 0x65, 0x83, 0x58, 0x83, 0x67]
            .iter()
            .map(|&b| char::from(b))
            .chain(".vi".chars())
            .collect();
        assert_eq!(
            recode_text(&stored, encoding_rs::SHIFT_JIS).as_deref(),
            Some("テスト.vi")
        );
        // Plain ASCII and text beyond U+00FF are not code page bytes.
        assert_eq!(code_page_bytes("plain.vi"), None);
        assert_eq!(code_page_bytes("テスト.vi"), None);
        assert!(
            recode(&LvPath::sibling("plain.vi"), Some("plain.vi")).is_empty(),
            "expected nothing, got {:?}",
            recode(&LvPath::sibling("plain.vi"), Some("plain.vi"))
        );
        let readings = recode(&LvPath::sibling(&stored), Some(&stored));
        assert!(readings
            .iter()
            .any(|(path, name)| path.file_name() == Some("テスト.vi")
                && name.as_deref() == Some("テスト.vi")));
        // Readings are distinct.
        for (i, reading) in readings.iter().enumerate() {
            assert!(!readings[..i].contains(reading));
        }
    }

    #[cfg(unix)]
    #[test]
    fn resolves_unix_absolute() {
        let link = LvPath::absolute(&["home", "u", "a.vi"]);
        assert_eq!(
            resolve(Path::new("main.vi"), &link).unwrap(),
            Path::new("/home/u/a.vi")
        );
    }
}
