//! Directed graph of file-to-file references inside a workspace.
//!
//! Edges come in three kinds (see [`EdgeKind`]). "Dependency" below means a
//! `Uses` or `Inherits` edge; `Contains` edges describe ownership and never
//! take part in cycle detection, because a member routinely refers back to
//! the library or class that owns it.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};

use buzzard_rsrc::{LvPath, PathKind};
use petgraph::algo::tarjan_scc;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{EdgeFiltered, EdgeRef};
use petgraph::Direction;
use serde::Serialize;

use crate::workspace::{normalize, Workspace};

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

/// Every file of a workspace and the edges between them.
#[derive(Debug, Default)]
pub struct DependencyGraph {
    graph: DiGraph<PathBuf, EdgeKind>,
    index: HashMap<PathBuf, NodeIndex>,
}

impl DependencyGraph {
    /// Build the graph for a workspace. A link or parent class whose stored
    /// path matches no file falls back to a file name that is unique in the
    /// workspace.
    pub fn build(workspace: &Workspace) -> Self {
        let mut graph = DiGraph::new();
        let mut index = HashMap::new();
        let mut by_name: HashMap<&str, Vec<NodeIndex>> = HashMap::new();
        for path in workspace.files().keys() {
            let node = graph.add_node(path.clone());
            index.insert(path.clone(), node);
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                by_name.entry(name).or_default().push(node);
            }
        }

        // A stale path (moved checkout, absolute path from another machine,
        // installation path of a library developed in this workspace) still
        // identifies its target when the file name is unique.
        let unique = |name: &str| match by_name.get(name)?.as_slice() {
            [only] => Some(*only),
            _ => None,
        };

        for (path, state) in workspace.files() {
            let from = index[path];
            let mut targets: BTreeSet<(NodeIndex, EdgeKind)> = BTreeSet::new();
            if let Some(project) = state.project() {
                for member in project.members.iter().filter(|m| !m.dependency) {
                    let target = member.path.as_ref().and_then(|p| index.get(p));
                    targets.extend(target.map(|&to| (to, EdgeKind::Contains)));
                }
                if let Some(parent) = &project.parent_class {
                    let exact = parent.path.as_ref().and_then(|p| index.get(p).copied());
                    // No class is its own parent, whatever its name suggests.
                    let target = exact.or_else(|| unique(&parent.name).filter(|&to| to != from));
                    targets.extend(target.map(|to| (to, EdgeKind::Inherits)));
                }
            }
            let links = state.metadata().map_or(&[][..], |meta| &meta.links);
            for link in links {
                let exact = resolve(path, &link.path).and_then(|p| index.get(&p).copied());
                let target = exact.or_else(|| unique(link.name()?));
                targets.extend(target.map(|to| (to, EdgeKind::Uses)));
            }
            for (to, kind) in targets {
                graph.add_edge(from, to, kind);
            }
        }
        Self { graph, index }
    }

    fn neighbors(&self, path: &Path, direction: Direction, kind: EdgeKind) -> Vec<&Path> {
        let Some(&node) = self.index.get(&normalize(path)) else {
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
        PathKind::Relative => from.to_path_buf(),
        PathKind::Absolute => PathBuf::new(),
        PathKind::Unc | PathKind::NotAPath => return None,
    };
    for (i, component) in link.components.iter().enumerate() {
        if component.is_empty() {
            if !out.pop() {
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
