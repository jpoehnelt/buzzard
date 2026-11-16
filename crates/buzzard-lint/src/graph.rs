//! Directed graph of file-to-file references inside a workspace.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};

use buzzard_rsrc::{LvPath, PathKind};
use petgraph::algo::tarjan_scc;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::Direction;

use crate::workspace::{normalize, Workspace};

/// Edges point from a file to the files it uses (caller -> callee).
#[derive(Debug, Default)]
pub struct DependencyGraph {
    graph: DiGraph<PathBuf, ()>,
    index: HashMap<PathBuf, NodeIndex>,
}

impl DependencyGraph {
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

        for (path, state) in workspace.files() {
            let Some(meta) = state.metadata() else {
                continue;
            };
            let from = index[path];
            let mut targets = BTreeSet::new();
            for link in &meta.links {
                let exact = resolve(path, &link.path).and_then(|p| index.get(&p).copied());
                // A stale path (moved checkout, absolute path from another
                // machine) still identifies its target when the file name is
                // unique in the workspace.
                let target = exact.or_else(|| match by_name.get(link.name()?)?.as_slice() {
                    [only] => Some(*only),
                    _ => None,
                });
                targets.extend(target);
            }
            for to in targets {
                graph.add_edge(from, to, ());
            }
        }
        Self { graph, index }
    }

    fn neighbors(&self, path: &Path, direction: Direction) -> Vec<&Path> {
        let Some(&node) = self.index.get(&normalize(path)) else {
            return Vec::new();
        };
        let mut out: Vec<&Path> = self
            .graph
            .neighbors_directed(node, direction)
            .map(|n| self.graph[n].as_path())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Files that reference `path`.
    pub fn callers(&self, path: &Path) -> Vec<&Path> {
        self.neighbors(path, Direction::Incoming)
    }

    /// Files that `path` references.
    pub fn callees(&self, path: &Path) -> Vec<&Path> {
        self.neighbors(path, Direction::Outgoing)
    }

    /// Every group of mutually dependent files, with one concrete cycle
    /// through the group's first member. Groups and members are sorted.
    pub fn cycles(&self) -> Vec<Cycle> {
        let mut out: Vec<Cycle> = tarjan_scc(&self.graph)
            .into_iter()
            .filter(|scc| scc.len() > 1 || self.graph.contains_edge(scc[0], scc[0]))
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
                .graph
                .neighbors(n)
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

    pub fn file_count(&self) -> usize {
        self.graph.node_count()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cycle {
    /// Every file in the strongly connected group, sorted.
    pub members: Vec<PathBuf>,
    /// One concrete cycle: each file references the next, the last
    /// references the first.
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
                out.push("/");
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
            components: vec!["".into(), "".into(), "lib".into(), "b.vi".into()],
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
