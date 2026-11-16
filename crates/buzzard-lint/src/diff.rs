//! Semantic diff of two revisions of one file: settings, dependencies, and
//! the nodes and wires of the block diagram.
//!
//! The result is text-first, sorted and deterministic, so it can be posted
//! as a review comment or used as a `git diff` driver.
//!
//! # Matching nodes between revisions
//!
//! Every heap object carries a unique id ([`Node::uid`], `Terminal::uid`,
//! `Wire::uid`). Whether those ids survive a save decides how hard matching
//! is, so it was measured on 1,340 pairs of consecutive git revisions of
//! real files (385 from the JKI State Machine repository, 955 from
//! grpc-labview; 768 of the pairs also cross an editor version upgrade):
//!
//! * 95.9% of the nodes of a new revision (23,088 of 24,078) have the id
//!   and class of a node in the old revision. For terminals the figure is
//!   95.5% (161,123 of 168,707) and for wires 94.6% (47,642 of 50,378).
//!   The rate is the same with and without a version upgrade in between.
//! * In 1,054 of the 1,340 pairs every node id is kept.
//! * Of 23,093 node ids present in both revisions, 5 name an object of a
//!   different class in the new revision: ids of deleted objects are
//!   occasionally reused.
//! * In 13 of 958 pairs whose nodes are otherwise the same, some ids were
//!   renumbered.
//!
//! Ids are therefore stable in practice but not guaranteed, and the matcher
//! works in two passes:
//!
//! 1. **By id.** Nodes with the same id and the same class are the same
//!    node, wherever they sit in the diagram. A different class means the
//!    id was reused, so that is no match.
//! 2. **By signature**, for what the first pass left over. The signature
//!    is the class, the linked name, the built-in function id, the multiset
//!    of terminal data types, and the nesting path (the classes of the
//!    enclosing structures and the frame index within each). Nodes with
//!    equal signatures are paired in order of position.
//!
//! Anything still unmatched is reported as added or removed. The second
//! pass is deliberately strict: with ids this stable, most leftovers are
//! real additions and removals, and a looser match would invent "changed"
//! nodes out of unrelated ones.
//!
//! Terminals of matched nodes are matched the same way (by id, then by
//! data type and data object class in list order). A wire is compared as
//! the set of connections it makes, source terminal to sink terminal, so
//! adding one branch to an existing wire is one added connection. A
//! connection that moved to another terminal of the same two nodes is
//! reported as one changed wire, not as a removal and an addition.
//!
//! A matched node whose bounds differ is reported as *moved* (which
//! includes resized), separately from real changes, because rearranging a
//! diagram is the most common noise in a review. [`DiffOptions`] can drop
//! those entries. Diagram boundaries are never reported as moved: their
//! bounds only mirror the structure around them.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write;

use buzzard_rsrc::{
    BlockDiagram, DataType, Diagram, LinkRef, Node, Rect, Reentrancy, ViFlags, ViMetadata,
};
use serde::Serialize;

use crate::dataflow::{describe, function_name};
use crate::render::label;
use crate::Inspection;

/// What [`diff`] leaves out of the report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffOptions {
    /// Leave out nodes that only changed position or size.
    pub ignore_moves: bool,
}

/// How a node of the new revision was paired with one of the old.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    /// Same object id and same class.
    Uid,
    /// Same class, linked name, function id, terminal types and nesting
    /// path, for a node whose id did not match.
    Signature,
}

/// How many entries each list of a [`ViDiff`] holds.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Summary {
    /// Settings that differ.
    pub settings_changed: usize,
    /// Dependencies only in the new revision.
    pub dependencies_added: usize,
    /// Dependencies only in the old revision.
    pub dependencies_removed: usize,
    /// Nodes only in the new revision.
    pub nodes_added: usize,
    /// Nodes only in the old revision.
    pub nodes_removed: usize,
    /// Matched nodes that changed in more than position or size.
    pub nodes_changed: usize,
    /// Matched nodes reported as moved or resized.
    pub nodes_moved: usize,
    /// Moved nodes left out because of [`DiffOptions::ignore_moves`].
    pub moves_ignored: usize,
    /// Connections only in the new revision.
    pub wires_added: usize,
    /// Connections only in the old revision.
    pub wires_removed: usize,
    /// Connections that moved to another terminal of the same nodes.
    pub wires_changed: usize,
}

/// One setting of the file that differs, e.g. `automatic error handling`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingChange {
    /// Name of the setting, e.g. `reentrancy`.
    pub setting: String,
    /// Value in the old revision, e.g. `on`, `off` or `unknown`.
    pub old: String,
    /// Value in the new revision.
    pub new: String,
}

/// A linked file. Two links are the same dependency when their paths and
/// their names are equal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Dependency {
    /// Where the linked file was when the link was saved.
    pub path: String,
    /// Qualified name of the linked item, parts joined with `:`.
    pub name: String,
}

/// Dependencies that only one revision has, sorted by path, then name.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct DependencyChanges {
    /// Only in the new revision.
    pub added: Vec<Dependency>,
    /// Only in the old revision.
    pub removed: Vec<Dependency>,
}

/// A node as it is named in the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeRef {
    /// Object id in the revision the node is from.
    pub uid: i32,
    /// Object class, e.g. `prim` or `iUse`.
    pub class: String,
    /// What the node is, e.g. ``subVI `Close.vi` `` or `for loop`.
    pub description: String,
    /// Enclosing structures, outermost first; empty at the top level.
    pub location: Vec<String>,
}

/// A node of both revisions that changed in more than position or size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeChange {
    /// The node in the new revision.
    pub node: NodeRef,
    /// Object id of the node in the old revision.
    pub old_uid: i32,
    /// How the two were paired.
    pub matched_by: MatchKind,
    /// One phrase per difference, e.g. `frames 2 -> 3`.
    pub details: Vec<String>,
}

/// A node of both revisions whose position or size changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeMove {
    /// The node in the new revision.
    pub node: NodeRef,
    /// Object id of the node in the old revision.
    pub old_uid: i32,
    /// How the two were paired.
    pub matched_by: MatchKind,
    /// Bounds in the old revision.
    pub from: Option<Rect>,
    /// Bounds in the new revision.
    pub to: Option<Rect>,
}

/// Block diagram nodes that differ, each list sorted by [`NodeRef::text`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct NodeChanges {
    /// Nodes only in the new revision.
    pub added: Vec<NodeRef>,
    /// Nodes only in the old revision.
    pub removed: Vec<NodeRef>,
    /// Matched nodes that changed in more than position or size.
    pub changed: Vec<NodeChange>,
    /// Matched nodes that moved or were resized; empty with
    /// [`DiffOptions::ignore_moves`].
    pub moved: Vec<NodeMove>,
}

/// One end of a connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WireEnd {
    /// Object id of the terminal.
    pub terminal: i32,
    /// The node that owns the terminal, described with its id, e.g.
    /// ``subVI `Close.vi` #40``. `None` when no node in the diagram owns
    /// the terminal.
    pub node: Option<String>,
}

/// One connection of a wire: from its source to one of its sinks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WireRef {
    /// `None` when the type of the data is not known.
    pub data_type: Option<String>,
    /// The source end.
    pub from: WireEnd,
    /// The sink end.
    pub to: WireEnd,
    /// Enclosing structures of the wire, outermost first.
    pub location: Vec<String>,
}

/// A connection between the same two nodes that now uses another terminal
/// of one or both of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WireChange {
    /// The connection in the new revision.
    pub wire: WireRef,
    /// Source terminal id in the old revision.
    pub old_from_terminal: i32,
    /// Sink terminal id in the old revision.
    pub old_to_terminal: i32,
}

/// Wire connections that differ, each list sorted by [`WireRef::text`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct WireChanges {
    /// Connections only in the new revision.
    pub added: Vec<WireRef>,
    /// Connections only in the old revision.
    pub removed: Vec<WireRef>,
    /// Connections that moved to another terminal of the same nodes.
    pub changed: Vec<WireChange>,
}

/// Everything that differs between two revisions of a file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct ViDiff {
    /// Whether nothing differs. Moves left out by
    /// [`DiffOptions::ignore_moves`] do not count.
    pub identical: bool,
    /// How many entries each list holds.
    pub summary: Summary,
    /// Things the reader must know to interpret the rest, e.g. that one
    /// side does not exist.
    pub notes: Vec<String>,
    /// Settings that differ, always in the same order.
    pub settings: Vec<SettingChange>,
    /// Links added and removed.
    pub dependencies: DependencyChanges,
    /// Block diagram nodes added, removed, changed and moved.
    pub nodes: NodeChanges,
    /// Block diagram connections added, removed and re-attached.
    pub wires: WireChanges,
}

/// Compare two revisions of a file. `None` stands for a revision that does
/// not exist: everything in the other one is then added or removed.
pub fn diff(old: Option<&Inspection>, new: Option<&Inspection>, options: DiffOptions) -> ViDiff {
    let mut out = ViDiff::default();
    match (old, new) {
        (None, None) => {}
        (None, Some(_)) => out
            .notes
            .push("old file is missing: everything is reported as added".to_owned()),
        (Some(_), None) => out
            .notes
            .push("new file is missing: everything is reported as removed".to_owned()),
        (Some(old), Some(new)) => out.settings = settings(&old.metadata, &new.metadata),
    }
    out.dependencies = dependencies(
        old.map_or(&[], |i| &i.metadata.links),
        new.map_or(&[], |i| &i.metadata.links),
    );

    let old_diagram = old.and_then(|i| i.diagram.as_ref());
    let new_diagram = new.and_then(|i| i.diagram.as_ref());
    if let (Some(_), Some(_)) = (old, new) {
        match (old_diagram, new_diagram) {
            (None, Some(_)) => out.notes.push(
                "old file has no readable block diagram: every node and wire is reported as added"
                    .to_owned(),
            ),
            (Some(_), None) => out.notes.push(
                "new file has no readable block diagram: every node and wire is reported as removed"
                    .to_owned(),
            ),
            _ => {}
        }
    }
    let moves_ignored = diagrams(old_diagram, new_diagram, options, &mut out);

    out.summary = Summary {
        settings_changed: out.settings.len(),
        dependencies_added: out.dependencies.added.len(),
        dependencies_removed: out.dependencies.removed.len(),
        nodes_added: out.nodes.added.len(),
        nodes_removed: out.nodes.removed.len(),
        nodes_changed: out.nodes.changed.len(),
        nodes_moved: out.nodes.moved.len(),
        moves_ignored,
        wires_added: out.wires.added.len(),
        wires_removed: out.wires.removed.len(),
        wires_changed: out.wires.changed.len(),
    };
    out.identical = Summary {
        moves_ignored: 0,
        ..out.summary.clone()
    } == Summary::default();
    out
}

// ---------------------------------------------------------------- settings

fn on_off(value: Option<bool>) -> String {
    match value {
        Some(true) => "on",
        Some(false) => "off",
        None => "unknown",
    }
    .to_owned()
}

fn or_none(value: Option<String>) -> String {
    value.unwrap_or_else(|| "none".to_owned())
}

fn settings(old: &ViMetadata, new: &ViMetadata) -> Vec<SettingChange> {
    type Flag = fn(&ViFlags) -> Option<bool>;
    let mut out = Vec::new();
    let mut push = |setting: &str, old: String, new: String| {
        if old != new {
            out.push(SettingChange {
                setting: setting.to_owned(),
                old: clean(&old),
                new: clean(&new),
            });
        }
    };
    push(
        "file type",
        old.file_type.to_string(),
        new.file_type.to_string(),
    );
    push(
        "saved name",
        or_none(old.saved_name.as_ref().map(|n| format!("`{n}`"))),
        or_none(new.saved_name.as_ref().map(|n| format!("`{n}`"))),
    );
    push(
        "saved version",
        or_none(old.version.map(|v| v.to_string())),
        or_none(new.version.map(|v| v.to_string())),
    );
    push(
        "owning library",
        or_none((!old.libraries.is_empty()).then(|| format!("`{}`", old.libraries.join(" > ")))),
        or_none((!new.libraries.is_empty()).then(|| format!("`{}`", new.libraries.join(" > ")))),
    );

    let flag = |meta: &ViMetadata, get: Flag| on_off(meta.flags.as_ref().and_then(get));
    let flags: [(&str, Flag); 10] = [
        ("compiled code separated", |f| f.separate_compiled_code),
        ("automatic error handling", |f| f.auto_error_handling),
        ("debugging", |f| f.debuggable),
        ("breakpoints", |f| f.has_breakpoints),
        ("suspend when called", |f| f.suspend_when_called),
        ("run when opened", |f| f.run_on_open),
        ("clear indicators when called", |f| f.clear_indicators),
        ("saved for previous version", |f| f.saved_for_previous),
        ("inlining", |f| f.inline),
        ("locked", |f| f.locked),
    ];
    for (name, get) in flags {
        push(name, flag(old, get), flag(new, get));
    }
    let count = |meta: &ViMetadata| {
        meta.flags
            .as_ref()
            .and_then(|f| f.breakpoint_count)
            .map_or_else(|| "unknown".to_owned(), |n| n.to_string())
    };
    push("breakpoint count", count(old), count(new));
    let reentrancy = |meta: &ViMetadata| {
        match meta.flags.as_ref().and_then(|f| f.reentrancy) {
            Some(Reentrancy::NonReentrant) => "non-reentrant",
            Some(Reentrancy::SharedClones) => "shared clones",
            Some(Reentrancy::PreallocatedClones) => "preallocated clones",
            None => "unknown",
        }
        .to_owned()
    };
    push("reentrancy", reentrancy(old), reentrancy(new));
    let password = |meta: &ViMetadata| match meta.password_protected {
        Some(true) => "on".to_owned(),
        // A file without a password record has no password either.
        Some(false) | None => "off".to_owned(),
    };
    push("password protection", password(old), password(new));
    out
}

// ------------------------------------------------------------ dependencies

fn dependency(link: &LinkRef) -> Dependency {
    Dependency {
        path: clean(&link.path.to_string()),
        name: clean(&link.qualified_name.join(":")),
    }
}

fn dependencies(old: &[LinkRef], new: &[LinkRef]) -> DependencyChanges {
    // A dependency is a path and a name. The link type and the heap ids of
    // the users of a link are left out: one file is often linked several
    // times, and its users belong to the block diagram comparison.
    let old: BTreeSet<Dependency> = old.iter().map(dependency).collect();
    let new: BTreeSet<Dependency> = new.iter().map(dependency).collect();
    DependencyChanges {
        added: new.difference(&old).cloned().collect(),
        removed: old.difference(&new).cloned().collect(),
    }
}

// ----------------------------------------------------------- block diagram

/// Remove characters that would break the one-line-per-change format.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Description of a node without its id or location.
pub fn describe_node(node: &Node) -> String {
    let text = match (node.class.as_str(), &node.name, node.primitive) {
        ("sRN", _, _) => "diagram boundary".to_owned(),
        // Structures are named by `label`; it falls back to `describe`.
        (_, None, None) => label(node),
        // A function the table does not name is told apart by its id.
        ("prim", None, Some(id)) if function_name(node).is_none() => {
            format!("built-in function {id}")
        }
        _ => describe(node),
    };
    clean(&text)
}

fn type_name(data_type: Option<DataType>) -> Option<String> {
    Some(match data_type? {
        // Says nothing a reader could use.
        DataType::Other | DataType::Void => return None,
        DataType::ErrorCluster => "error cluster".to_owned(),
        DataType::Reference(kind) => format!("{kind:?} reference").to_lowercase(),
        other => format!("{other:?}").to_lowercase(),
    })
}

struct FlatNode<'a> {
    node: &'a Node,
    /// Index of the diagram the node is in.
    diagram: usize,
    /// Indices of the node's nested diagrams, one per frame.
    frames: Vec<usize>,
}

struct FlatDiagram<'a> {
    diagram: &'a Diagram,
    /// Owning structure and frame index; `None` for the top level.
    parent: Option<(usize, usize)>,
}

/// A wire connection: source terminal, sink terminal, owning diagram.
type Edge = (i32, i32, usize);

/// A block diagram with its nesting flattened into index-linked lists.
#[derive(Default)]
struct Flat<'a> {
    nodes: Vec<FlatNode<'a>>,
    diagrams: Vec<FlatDiagram<'a>>,
    /// First node with each id.
    by_uid: HashMap<i32, usize>,
    /// First node that owns each terminal id.
    owner: HashMap<i32, usize>,
    edges: Vec<Edge>,
}

impl<'a> Flat<'a> {
    fn new(diagram: Option<&'a BlockDiagram>) -> Self {
        let mut flat = Self::default();
        let Some(diagram) = diagram else {
            return flat;
        };
        flat.diagrams.push(FlatDiagram {
            diagram: &diagram.root,
            parent: None,
        });
        // The list grows while it is walked: breadth first, no recursion.
        let mut next = 0;
        while next < flat.diagrams.len() {
            let current = flat.diagrams[next].diagram;
            for node in &current.nodes {
                let index = flat.nodes.len();
                let first_frame = flat.diagrams.len();
                for (frame, inner) in node.diagrams.iter().enumerate() {
                    flat.diagrams.push(FlatDiagram {
                        diagram: inner,
                        parent: Some((index, frame)),
                    });
                }
                flat.by_uid.entry(node.uid).or_insert(index);
                for terminal in &node.terminals {
                    flat.owner.entry(terminal.uid).or_insert(index);
                }
                flat.nodes.push(FlatNode {
                    node,
                    diagram: next,
                    frames: (first_frame..flat.diagrams.len()).collect(),
                });
            }
            for wire in &current.wires {
                for sink in &wire.sinks {
                    flat.edges.push((wire.source, *sink, next));
                }
            }
            next += 1;
        }
        flat
    }

    /// Enclosing structures of a diagram, outermost first.
    fn location(&self, mut diagram: usize) -> Vec<String> {
        let mut out = Vec::new();
        // A parent's diagram always has a lower index, so this terminates.
        while let Some((parent, frame)) = self.diagrams[diagram].parent {
            let node = self.nodes[parent].node;
            let mut text = format!("{} #{}", describe_node(node), node.uid);
            if node.diagrams.len() > 1 {
                let _ = write!(text, " frame {frame}");
            }
            out.push(text);
            diagram = self.nodes[parent].diagram;
        }
        out.reverse();
        out
    }

    fn node_ref(&self, index: usize) -> NodeRef {
        let flat = &self.nodes[index];
        NodeRef {
            uid: flat.node.uid,
            class: clean(&flat.node.class),
            description: describe_node(flat.node),
            location: self.location(flat.diagram),
        }
    }

    fn end(&self, terminal: i32) -> WireEnd {
        WireEnd {
            terminal,
            node: self.owner.get(&terminal).map(|&index| {
                let node = self.nodes[index].node;
                format!("{} #{}", describe_node(node), node.uid)
            }),
        }
    }

    /// Type of the data on a connection, from whichever end knows it.
    fn wire_type(&self, (source, sink, _): Edge) -> Option<String> {
        let type_of = |uid: i32| {
            let node = self.nodes[*self.owner.get(&uid)?].node;
            let terminal = node.terminals.iter().find(|t| t.uid == uid)?;
            type_name(terminal.data_type)
        };
        type_of(source).or_else(|| type_of(sink))
    }

    fn wire_ref(&self, edge: Edge) -> WireRef {
        let (source, sink, diagram) = edge;
        WireRef {
            data_type: self.wire_type(edge),
            from: self.end(source),
            to: self.end(sink),
            location: self.location(diagram),
        }
    }
}

/// Structural signature used by the second matching pass.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Signature<'a> {
    class: &'a str,
    name: Option<&'a str>,
    primitive: Option<u32>,
    terminal_types: Vec<Option<DataType>>,
    /// Interned nesting path, comparable between the two revisions.
    path: usize,
}

/// Gives equal nesting paths (structure classes and frame indices from the
/// top level down) the same number on both sides.
#[derive(Default)]
struct Paths<'a> {
    ids: HashMap<(usize, &'a str, usize), usize>,
}

impl<'a> Paths<'a> {
    /// Path number of every diagram of `flat`. The top level is 0.
    fn of(&mut self, flat: &Flat<'a>) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::with_capacity(flat.diagrams.len());
        for diagram in &flat.diagrams {
            let id = match diagram.parent {
                None => 0,
                Some((parent, frame)) => {
                    let owner = &flat.nodes[parent];
                    let key = (out[owner.diagram], owner.node.class.as_str(), frame);
                    let next = self.ids.len() + 1;
                    *self.ids.entry(key).or_insert(next)
                }
            };
            out.push(id);
        }
        out
    }
}

fn position(node: &Node) -> (i16, i16, i16, i16, i32) {
    let b = node.bounds.unwrap_or(Rect {
        top: i16::MAX,
        left: i16::MAX,
        bottom: i16::MAX,
        right: i16::MAX,
    });
    (b.top, b.left, b.bottom, b.right, node.uid)
}

fn signature<'a>(flat: &FlatNode<'a>, path: usize) -> Signature<'a> {
    let mut terminal_types: Vec<_> = flat.node.terminals.iter().map(|t| t.data_type).collect();
    terminal_types.sort();
    Signature {
        class: flat.node.class.as_str(),
        name: flat.node.name.as_deref(),
        primitive: flat.node.primitive,
        terminal_types,
        path,
    }
}

/// Pair the nodes of two diagrams. Returns `(old, new, how)` in old order.
fn match_nodes<'a>(old: &Flat<'a>, new: &Flat<'a>) -> Vec<(usize, usize, MatchKind)> {
    let mut pairs = Vec::new();
    let mut old_used = vec![false; old.nodes.len()];
    let mut new_used = vec![false; new.nodes.len()];

    for (index, flat) in old.nodes.iter().enumerate() {
        // Only the first node with an id can be found by it.
        if old.by_uid.get(&flat.node.uid) != Some(&index) {
            continue;
        }
        if let Some(&other) = new.by_uid.get(&flat.node.uid) {
            if new.nodes[other].node.class == flat.node.class {
                old_used[index] = true;
                new_used[other] = true;
                pairs.push((index, other, MatchKind::Uid));
            }
        }
    }

    let mut paths = Paths::default();
    let old_paths = paths.of(old);
    let new_paths = paths.of(new);
    let mut groups: BTreeMap<Signature<'_>, (Vec<usize>, Vec<usize>)> = BTreeMap::new();
    for (index, flat) in old.nodes.iter().enumerate() {
        if !old_used[index] {
            let group = groups
                .entry(signature(flat, old_paths[flat.diagram]))
                .or_default();
            group.0.push(index);
        }
    }
    // Without leftovers on the old side there is nothing to pair with.
    if !groups.is_empty() {
        for (index, flat) in new.nodes.iter().enumerate() {
            if !new_used[index] {
                if let Some(group) = groups.get_mut(&signature(flat, new_paths[flat.diagram])) {
                    group.1.push(index);
                }
            }
        }
    }
    for (_, (mut olds, mut news)) in groups {
        olds.sort_by_key(|&i| position(old.nodes[i].node));
        news.sort_by_key(|&i| position(new.nodes[i].node));
        for (o, n) in olds.into_iter().zip(news) {
            pairs.push((o, n, MatchKind::Signature));
        }
    }
    pairs.sort_by_key(|&(o, _, _)| o);
    pairs
}

/// Pair the terminals of two matched nodes: by id, then by data type and
/// data object class in list order. Yields `(old uid, new uid)`.
fn match_terminals(old: &Node, new: &Node, mut matched: impl FnMut(i32, i32)) {
    type Key<'a> = (Option<DataType>, Option<&'a str>);
    let new_uids: HashSet<i32> = new.terminals.iter().map(|t| t.uid).collect();
    let old_uids: HashSet<i32> = old.terminals.iter().map(|t| t.uid).collect();
    let mut waiting: HashMap<Key<'_>, Vec<i32>> = HashMap::new();
    for terminal in new.terminals.iter().rev() {
        if !old_uids.contains(&terminal.uid) {
            let key = (terminal.data_type, terminal.dco_class.as_deref());
            waiting.entry(key).or_default().push(terminal.uid);
        }
    }
    for terminal in &old.terminals {
        if new_uids.contains(&terminal.uid) {
            matched(terminal.uid, terminal.uid);
        } else {
            let key = (terminal.data_type, terminal.dco_class.as_deref());
            if let Some(other) = waiting.get_mut(&key).and_then(Vec::pop) {
                matched(terminal.uid, other);
            }
        }
    }
}

/// Pair the frames of two matched structures: by id, then the rest in
/// order. Records `old diagram index -> new diagram index`.
fn match_frames(
    old: &Flat<'_>,
    old_node: &FlatNode<'_>,
    new: &Flat<'_>,
    new_node: &FlatNode<'_>,
    same: &mut HashMap<usize, usize>,
) {
    if old_node.frames.is_empty() || new_node.frames.is_empty() {
        return;
    }
    // New frames by id, last first, so that `pop` takes them in order.
    let mut by_uid: HashMap<i32, Vec<usize>> = HashMap::new();
    for &frame in new_node.frames.iter().rev() {
        by_uid
            .entry(new.diagrams[frame].diagram.uid)
            .or_default()
            .push(frame);
    }
    let mut rest: Vec<usize> = Vec::new();
    let mut taken: HashSet<usize> = HashSet::new();
    for &frame in &old_node.frames {
        let uid = old.diagrams[frame].diagram.uid;
        match by_uid.get_mut(&uid).and_then(Vec::pop) {
            Some(twin) => {
                taken.insert(twin);
                same.insert(frame, twin);
            }
            None => rest.push(frame),
        }
    }
    let free = new_node.frames.iter().filter(|f| !taken.contains(f));
    for (frame, twin) in rest.into_iter().zip(free) {
        same.insert(frame, *twin);
    }
}

/// `+1 numeric, -2 string`: how the terminal types of a node changed.
fn terminal_delta(old: &Node, new: &Node) -> String {
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    let name = |t: Option<DataType>| match t {
        None => "unknown type".to_owned(),
        Some(DataType::Void) => "void".to_owned(),
        Some(other) => type_name(Some(other)).unwrap_or_else(|| "other".to_owned()),
    };
    for terminal in &old.terminals {
        *counts.entry(name(terminal.data_type)).or_default() -= 1;
    }
    for terminal in &new.terminals {
        *counts.entry(name(terminal.data_type)).or_default() += 1;
    }
    let parts: Vec<String> = counts
        .into_iter()
        .filter(|(_, n)| *n != 0)
        .map(|(name, n)| format!("{n:+} {name}"))
        .collect();
    parts.join(", ")
}

fn quoted(name: Option<&str>) -> String {
    name.map_or_else(|| "none".to_owned(), |n| format!("`{}`", clean(n)))
}

fn place(location: &[String]) -> String {
    if location.is_empty() {
        "top level".to_owned()
    } else {
        location.join(" > ")
    }
}

/// Where a terminal ends up after matching, so that connections of the two
/// revisions can be compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum End {
    /// Present in both revisions; the id is the one in the new revision.
    Both(i32),
    OldOnly(i32),
    NewOnly(i32),
}

/// Compare the diagrams and fill `out.nodes` and `out.wires`. Returns the
/// number of moves that were left out.
fn diagrams(
    old: Option<&BlockDiagram>,
    new: Option<&BlockDiagram>,
    options: DiffOptions,
    out: &mut ViDiff,
) -> usize {
    let old = Flat::new(old);
    let new = Flat::new(new);
    let pairs = match_nodes(&old, &new);

    // Old diagram index -> new diagram index, for telling whether a node
    // is still in the same place of the nesting.
    let mut same_diagram: HashMap<usize, usize> = HashMap::new();
    if !old.diagrams.is_empty() && !new.diagrams.is_empty() {
        same_diagram.insert(0, 0);
    }
    // Old terminal id -> new terminal id.
    let mut terminals: HashMap<i32, i32> = HashMap::new();
    let mut new_terminals: HashSet<i32> = HashSet::new();
    let mut old_matched = vec![false; old.nodes.len()];
    let mut new_matched = vec![false; new.nodes.len()];
    for &(o, n, _) in &pairs {
        old_matched[o] = true;
        new_matched[n] = true;
        let (old_node, new_node) = (&old.nodes[o], &new.nodes[n]);
        match_terminals(old_node.node, new_node.node, |from, to| {
            if !new_terminals.contains(&to) && !terminals.contains_key(&from) {
                terminals.insert(from, to);
                new_terminals.insert(to);
            }
        });
        match_frames(&old, old_node, &new, new_node, &mut same_diagram);
    }

    let mut moves_ignored = 0;
    for &(o, n, matched_by) in &pairs {
        let (old_flat, new_flat) = (&old.nodes[o], &new.nodes[n]);
        let (old_node, new_node) = (old_flat.node, new_flat.node);
        let details = node_details(&old, old_flat, &new, new_flat, &same_diagram);
        if !details.is_empty() {
            out.nodes.changed.push(NodeChange {
                node: new.node_ref(n),
                old_uid: old_node.uid,
                matched_by,
                details,
            });
        }
        // A boundary has no position of its own: its bounds follow those of
        // the structure around it, which is reported itself.
        if old_node.bounds != new_node.bounds && new_node.class != "sRN" {
            if options.ignore_moves {
                moves_ignored += 1;
            } else {
                out.nodes.moved.push(NodeMove {
                    node: new.node_ref(n),
                    old_uid: old_node.uid,
                    matched_by,
                    from: old_node.bounds,
                    to: new_node.bounds,
                });
            }
        }
    }
    for (index, matched) in old_matched.iter().enumerate() {
        if !matched {
            out.nodes.removed.push(old.node_ref(index));
        }
    }
    for (index, matched) in new_matched.iter().enumerate() {
        if !matched {
            out.nodes.added.push(new.node_ref(index));
        }
    }
    wires(
        &old,
        &new,
        &pairs,
        &terminals,
        &new_terminals,
        &mut out.wires,
    );

    out.nodes.added.sort_by_cached_key(|n| (n.text(), n.uid));
    out.nodes.removed.sort_by_cached_key(|n| (n.text(), n.uid));
    out.nodes
        .changed
        .sort_by_cached_key(|c| (c.node.text(), c.node.uid, c.old_uid));
    out.nodes
        .moved
        .sort_by_cached_key(|m| (m.node.text(), m.node.uid, m.old_uid));
    let wire_key = |w: &WireRef| (w.text(), w.from.terminal, w.to.terminal);
    out.wires.added.sort_by_cached_key(wire_key);
    out.wires.removed.sort_by_cached_key(wire_key);
    out.wires.changed.sort_by_cached_key(|c| {
        let (text, from, to) = wire_key(&c.wire);
        (text, from, to, c.old_from_terminal, c.old_to_terminal)
    });
    moves_ignored
}

/// What changed about a matched node apart from its bounds, one phrase per
/// difference. `same_diagram` maps old diagram indices to new ones.
fn node_details(
    old: &Flat<'_>,
    old_flat: &FlatNode<'_>,
    new: &Flat<'_>,
    new_flat: &FlatNode<'_>,
    same_diagram: &HashMap<usize, usize>,
) -> Vec<String> {
    let (old_node, new_node) = (old_flat.node, new_flat.node);
    let mut details = Vec::new();
    if old_node.name != new_node.name {
        details.push(format!(
            "name {} -> {}",
            quoted(old_node.name.as_deref()),
            quoted(new_node.name.as_deref())
        ));
    }
    if old_node.primitive != new_node.primitive {
        // The id stays in the text: two ids can share a name.
        let id = |p: Option<u32>| match p {
            None => "none".to_owned(),
            Some(p) => match buzzard_rsrc::primitive_name(p) {
                Some(function) => format!("{p} ({})", function.name),
                None => p.to_string(),
            },
        };
        details.push(format!(
            "function id {} -> {}",
            id(old_node.primitive),
            id(new_node.primitive)
        ));
    }
    let delta = terminal_delta(old_node, new_node);
    if old_node.terminals.len() != new_node.terminals.len() {
        let mut text = format!(
            "terminals {} -> {}",
            old_node.terminals.len(),
            new_node.terminals.len()
        );
        if !delta.is_empty() {
            let _ = write!(text, " ({delta})");
        }
        details.push(text);
    } else if !delta.is_empty() {
        details.push(format!("terminal types changed ({delta})"));
    }
    if old_node.diagrams.len() != new_node.diagrams.len() {
        details.push(format!(
            "frames {} -> {}",
            old_node.diagrams.len(),
            new_node.diagrams.len()
        ));
    }
    if same_diagram.get(&old_flat.diagram) != Some(&new_flat.diagram) {
        details.push(format!(
            "enclosing structure {} -> {}",
            place(&old.location(old_flat.diagram)),
            place(&new.location(new_flat.diagram))
        ));
    }
    details
}

/// Compare the wires of the two diagrams as connections between matched
/// terminals and fill `out`. `terminals` maps old terminal ids to new ones
/// and `new_terminals` holds the new ids that were matched.
fn wires(
    old: &Flat<'_>,
    new: &Flat<'_>,
    pairs: &[(usize, usize, MatchKind)],
    terminals: &HashMap<i32, i32>,
    new_terminals: &HashSet<i32>,
    out: &mut WireChanges,
) {
    type Between = (usize, usize, Option<String>);
    let old_end = |uid: i32| match terminals.get(&uid) {
        Some(&twin) => End::Both(twin),
        // A terminal no node owns on either side is taken to be the same.
        None if !old.owner.contains_key(&uid) && !new.owner.contains_key(&uid) => End::Both(uid),
        None => End::OldOnly(uid),
    };
    let new_end = |uid: i32| {
        if new_terminals.contains(&uid)
            || (!old.owner.contains_key(&uid) && !new.owner.contains_key(&uid))
        {
            End::Both(uid)
        } else {
            End::NewOnly(uid)
        }
    };
    let mut old_edges: BTreeMap<(End, End), Edge> = BTreeMap::new();
    for &edge in &old.edges {
        old_edges
            .entry((old_end(edge.0), old_end(edge.1)))
            .or_insert(edge);
    }
    let mut new_edges: BTreeMap<(End, End), Edge> = BTreeMap::new();
    for &edge in &new.edges {
        new_edges
            .entry((new_end(edge.0), new_end(edge.1)))
            .or_insert(edge);
    }
    // A connection that left one terminal and one that arrived at another
    // terminal, between the same two nodes and carrying the same type, are
    // one connection that changed terminals.
    let twin: HashMap<usize, usize> = pairs.iter().map(|&(o, n, _)| (o, n)).collect();
    let mut rewired: BTreeMap<Between, (Vec<Edge>, Vec<Edge>)> = BTreeMap::new();
    let mut removed: Vec<Edge> = Vec::new();
    for (key, &edge) in &old_edges {
        if new_edges.contains_key(key) {
            continue;
        }
        let node = |uid: i32| {
            old.owner
                .get(&uid)
                .and_then(|index| twin.get(index))
                .copied()
        };
        match (node(edge.0), node(edge.1)) {
            (Some(from), Some(to)) => {
                let between = (from, to, old.wire_type(edge));
                rewired.entry(between).or_default().0.push(edge);
            }
            _ => removed.push(edge),
        }
    }
    for (key, &edge) in &new_edges {
        if old_edges.contains_key(key) {
            continue;
        }
        let node = |uid: i32| new.owner.get(&uid).copied();
        let waiting = match (node(edge.0), node(edge.1)) {
            (Some(from), Some(to)) => rewired.get_mut(&(from, to, new.wire_type(edge))),
            _ => None,
        };
        match waiting {
            Some(group) => group.1.push(edge),
            None => out.added.push(new.wire_ref(edge)),
        }
    }
    for (_, (olds, news)) in rewired {
        let common = olds.len().min(news.len());
        for (before, after) in olds.iter().zip(&news) {
            out.changed.push(WireChange {
                wire: new.wire_ref(*after),
                old_from_terminal: before.0,
                old_to_terminal: before.1,
            });
        }
        removed.extend(&olds[common..]);
        for &edge in &news[common..] {
            out.added.push(new.wire_ref(edge));
        }
    }
    for edge in removed {
        out.removed.push(old.wire_ref(edge));
    }
}

// --------------------------------------------------------------- rendering

impl NodeRef {
    /// `subVI `Close.vi` #40 in for loop #12`.
    pub fn text(&self) -> String {
        let mut text = format!("{} #{}", self.description, self.uid);
        if !self.location.is_empty() {
            let _ = write!(text, " in {}", self.location.join(" > "));
        }
        text
    }
}

impl WireEnd {
    fn text(&self) -> String {
        match &self.node {
            Some(node) => node.clone(),
            None => format!("terminal #{}", self.terminal),
        }
    }
}

impl WireRef {
    /// `error cluster wire from A #1 to B #2 in for loop #12`.
    pub fn text(&self) -> String {
        let mut text = match &self.data_type {
            Some(data_type) => format!("{data_type} wire"),
            None => "wire".to_owned(),
        };
        let _ = write!(text, " from {} to {}", self.from.text(), self.to.text());
        if !self.location.is_empty() {
            let _ = write!(text, " in {}", self.location.join(" > "));
        }
        text
    }
}

impl Dependency {
    /// ``dependency `path` ``, or ``dependency `name` at `path` `` when the
    /// path does not end in the item name.
    pub fn text(&self) -> String {
        // The name is only worth showing when the path does not end in it.
        let item = self.name.rsplit(':').next().unwrap_or("");
        if item.is_empty() || self.path.ends_with(item) {
            format!("dependency `{}`", self.path)
        } else {
            format!("dependency `{}` at `{}`", self.name, self.path)
        }
    }
}

fn rect_text(rect: Option<Rect>, with_size: bool) -> String {
    match rect {
        None => "unknown".to_owned(),
        Some(r) if with_size => format!("{},{} {}x{}", r.left, r.top, r.width(), r.height()),
        Some(r) => format!("{},{}", r.left, r.top),
    }
}

impl NodeMove {
    /// `X #5 (120,40 -> 150,40)`; sizes are shown when they differ.
    pub fn text(&self) -> String {
        let size = |r: Option<Rect>| r.map(|r| (r.width(), r.height()));
        let resized = size(self.from) != size(self.to);
        format!(
            "{} ({} -> {})",
            self.node.text(),
            rect_text(self.from, resized),
            rect_text(self.to, resized)
        )
    }
}

impl Summary {
    /// One line such as `3 nodes added, 1 removed; 2 wires added`.
    pub fn line(&self) -> String {
        let group = |one: &str, many: &str, counts: &[(usize, &str)]| -> Option<String> {
            let mut parts = counts.iter().filter(|(n, _)| *n > 0);
            let (n, verb) = parts.next()?;
            let noun = if *n == 1 { one } else { many };
            let mut text = format!("{n} {noun} {verb}");
            for (n, verb) in parts {
                let _ = write!(text, ", {n} {verb}");
            }
            Some(text)
        };
        let groups: Vec<String> = [
            group(
                "node",
                "nodes",
                &[
                    (self.nodes_added, "added"),
                    (self.nodes_removed, "removed"),
                    (self.nodes_changed, "changed"),
                    (self.nodes_moved, "moved"),
                ],
            ),
            group(
                "wire",
                "wires",
                &[
                    (self.wires_added, "added"),
                    (self.wires_removed, "removed"),
                    (self.wires_changed, "changed"),
                ],
            ),
            group("setting", "settings", &[(self.settings_changed, "changed")]),
            group(
                "dependency",
                "dependencies",
                &[
                    (self.dependencies_added, "added"),
                    (self.dependencies_removed, "removed"),
                ],
            ),
        ]
        .into_iter()
        .flatten()
        .collect();
        let mut line = if groups.is_empty() {
            "no differences".to_owned()
        } else {
            groups.join("; ")
        };
        if self.moves_ignored > 0 {
            let noun = if self.moves_ignored == 1 {
                "move"
            } else {
                "moves"
            };
            let _ = write!(line, " ({} {noun} ignored)", self.moves_ignored);
        }
        line
    }
}

impl ViDiff {
    /// Every change as `(section, line)`, in report order.
    fn lines(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        for s in &self.settings {
            let text = format!("changed: {} {} -> {}", s.setting, s.old, s.new);
            out.push(("Settings", text));
        }
        for d in &self.dependencies.added {
            out.push(("Dependencies", format!("added: {}", d.text())));
        }
        for d in &self.dependencies.removed {
            out.push(("Dependencies", format!("removed: {}", d.text())));
        }
        for n in &self.nodes.added {
            out.push(("Block diagram", format!("added: {}", n.text())));
        }
        for n in &self.nodes.removed {
            out.push(("Block diagram", format!("removed: {}", n.text())));
        }
        for c in &self.nodes.changed {
            let text = format!("changed: {}: {}", c.node.text(), c.details.join("; "));
            out.push(("Block diagram", text));
        }
        for m in &self.nodes.moved {
            out.push(("Block diagram", format!("moved: {}", m.text())));
        }
        for w in &self.wires.added {
            out.push(("Block diagram", format!("added: {}", w.text())));
        }
        for w in &self.wires.removed {
            out.push(("Block diagram", format!("removed: {}", w.text())));
        }
        for c in &self.wires.changed {
            let text = format!(
                "changed: {}: attached to a different terminal",
                c.wire.text()
            );
            out.push(("Block diagram", text));
        }
        out
    }
}

/// Plain text: a summary line, then notes, then one line per change.
pub fn to_text(diff: &ViDiff) -> String {
    let mut out = diff.summary.line();
    out.push('\n');
    for note in &diff.notes {
        let _ = writeln!(out, "note: {note}");
    }
    for (_, line) in diff.lines() {
        let _ = writeln!(out, "{line}");
    }
    out
}

/// Markdown for a review comment: the summary in bold, then one bullet
/// list per section.
pub fn to_markdown(diff: &ViDiff) -> String {
    let mut out = format!("**{}**\n", diff.summary.line());
    for note in &diff.notes {
        let _ = write!(out, "\n> {note}\n");
    }
    let mut section = "";
    for (name, line) in diff.lines() {
        if name != section {
            let _ = write!(out, "\n#### {name}\n\n");
            section = name;
        }
        let _ = writeln!(out, "- {line}");
    }
    out
}

#[cfg(test)]
mod tests;
