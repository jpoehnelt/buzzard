//! The block diagram as a dataflow graph: nodes, terminals and wires.
//!
//! Built from the `BDHb` object heap. In the heap every object is defined
//! once, with a class and a unique id, and is referred to elsewhere by that
//! id. A diagram lists its nodes and its signals (wires); a node lists its
//! terminals; a signal lists the terminals it connects, source first.
//! Structures such as loops and case structures are nodes that own nested
//! diagrams.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::blocks::heap::{Heap, HeapNode};
use crate::blocks::linkinfo;
use crate::blocks::types::{DataType, TypeTable};
use crate::blocks::zlib;
use crate::container::RsrcFile;
use crate::error::Result;

/// Bit of a data object's `objFlags` that marks it as an output.
///
/// Checked against 1,100 real diagrams: on node parameters (`parm`,
/// `iUseDCO`, `hGrowCItem`, ...) the bit agrees with the direction implied
/// by wires in all but 3 of over 30,000 wired terminals. Tunnels and other
/// structure objects use the bit differently; see
/// [`Terminal::direction_is_reliable`].
const DCO_IS_OUTPUT: u32 = 0x1;

/// Data object classes whose output flag has been verified on real files.
const RELIABLE_DIRECTION: [&str; 16] = [
    "aBuildDCO",
    "aIDCO",
    "aRepDCO",
    "concatDCO",
    "cpdArithDCO",
    "dmxDCO",
    "hGrowCItem",
    "iUseDCO",
    "invokeItem",
    "mergeErrorsDCO",
    "overridableParm",
    "parm",
    "polyIUseDCO",
    "printfArg",
    "propItem",
    "aDelDCO",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Input,
    Output,
}

/// A rectangle in diagram pixels, as stored in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Rect {
    pub top: i16,
    pub left: i16,
    pub bottom: i16,
    pub right: i16,
}

impl Rect {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let word = |i: usize| i16::from_be_bytes([bytes[i], bytes[i + 1]]);
        (bytes.len() == 8).then(|| Self {
            top: word(0),
            left: word(2),
            bottom: word(4),
            right: word(6),
        })
    }

    pub fn width(&self) -> i32 {
        i32::from(self.right) - i32::from(self.left)
    }

    pub fn height(&self) -> i32 {
        i32::from(self.bottom) - i32::from(self.top)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Terminal {
    pub uid: i32,
    /// Position: relative to the owning node's top-left corner, except for
    /// front-panel terminals, which are positioned in their diagram.
    pub bounds: Option<Rect>,
    pub direction: Direction,
    pub wired: bool,
    /// `None` when the type table has no entry for the terminal.
    pub data_type: Option<DataType>,
    /// Id of the data object behind the terminal. The two sides of a tunnel
    /// are separate terminals that share one data object.
    pub dco: Option<i32>,
    /// Class of that data object, e.g. `parm`, `selTun`, `lSR`.
    pub dco_class: Option<String>,
}

impl Terminal {
    /// Whether [`Terminal::direction`] can be trusted. It always can for a
    /// wired terminal; for an unwired one only when its data object is of a
    /// class whose direction flag has been verified.
    pub fn direction_is_reliable(&self) -> bool {
        self.wired
            || self
                .dco_class
                .as_deref()
                .is_some_and(|class| RELIABLE_DIRECTION.contains(&class))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Node {
    pub uid: i32,
    /// Object class, e.g. `prim` (built-in function), `iUse` (subVI call),
    /// `whileLoop`, `select` (case structure), `sRN` (the diagram's own
    /// boundary: front-panel terminals or the inner side of tunnels).
    pub class: String,
    /// Name of the called file for subVI calls and similar linked nodes.
    pub name: Option<String>,
    /// Resource id of a built-in function. The ids are stable across
    /// versions. The file does not name the function;
    /// [`primitive_name`](crate::primitive_name) looks the id up in a
    /// vendored table.
    pub primitive: Option<u32>,
    /// Position within the enclosing diagram. Nodes inside a structure are
    /// positioned relative to the structure's top-left corner.
    pub bounds: Option<Rect>,
    pub terminals: Vec<Terminal>,
    /// Nested diagrams of a structure, one per frame or case.
    pub diagrams: Vec<Diagram>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Wire {
    pub uid: i32,
    pub source: i32,
    pub sinks: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Diagram {
    pub uid: i32,
    pub nodes: Vec<Node>,
    pub wires: Vec<Wire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockDiagram {
    pub root: Diagram,
}

impl BlockDiagram {
    /// Decode the block diagram of a file. Returns `Ok(None)` when the file
    /// has no block diagram heap (for example a control).
    pub fn from_rsrc(file: &RsrcFile<'_>) -> Result<Option<Self>> {
        let Some(raw) = file.first(b"BDHb") else {
            return Ok(None);
        };
        let data = zlib::decompress(raw)?;
        let heap = Heap::parse(&data)?;

        // Types are best-effort: a diagram without them is still useful.
        let types = file
            .first(b"VCTP")
            .and_then(|vctp| zlib::decompress(vctp).ok())
            .and_then(|vctp| TypeTable::parse(&vctp, file.first(b"DTHP")).ok())
            .unwrap_or_default();

        let mut names = HashMap::new();
        for resource in file.of_type(b"LIbd") {
            for link in linkinfo::scan(resource.data) {
                if let Some(name) = link.name() {
                    for uid in &link.heap_uids {
                        names.entry(*uid as i32).or_insert_with(|| name.to_owned());
                    }
                }
            }
        }
        // Front-panel terminals on the diagram point at data objects that
        // are defined in the front-panel heap.
        let panel_data = file
            .first(b"FPHb")
            .and_then(|raw| zlib::decompress(raw).ok());
        let panel = panel_data
            .as_deref()
            .and_then(|data| Heap::parse(data).ok());
        Ok(Self::from_heaps(&heap, panel.as_ref(), &types, &names))
    }

    /// Build the graph from an already decoded block diagram heap alone.
    pub fn from_heap(
        heap: &Heap<'_>,
        types: &TypeTable,
        names: &HashMap<i32, String>,
    ) -> Option<Self> {
        Self::from_heaps(heap, None, types, names)
    }

    /// Build the graph from the block diagram heap, resolving references
    /// to front-panel objects through the front-panel heap when given.
    pub fn from_heaps(
        heap: &Heap<'_>,
        panel: Option<&Heap<'_>>,
        types: &TypeTable,
        names: &HashMap<i32, String>,
    ) -> Option<Self> {
        let mut builder = Builder {
            objects: HashMap::new(),
            sources: HashSet::new(),
            sinks: HashSet::new(),
            seen: HashSet::new(),
            types,
            names,
        };
        for root in &heap.roots {
            builder.index(root);
        }
        // Diagram objects take precedence; the panel only fills in the rest.
        for root in panel.iter().flat_map(|panel| &panel.roots) {
            builder.index_objects(root);
        }
        let root = heap
            .roots
            .iter()
            .flat_map(|root| std::iter::once(root).chain(&root.children))
            .find(|node| node.class_name() == Some("diag"))?;
        Some(Self {
            root: builder.diagram(root),
        })
    }

    /// Every diagram, outermost first, depth-first.
    pub fn diagrams(&self) -> Vec<&Diagram> {
        let mut out = Vec::new();
        let mut stack = vec![&self.root];
        while let Some(diagram) = stack.pop() {
            out.push(diagram);
            let nested = diagram.nodes.iter().flat_map(|n| &n.diagrams);
            stack.extend(nested.collect::<Vec<_>>().into_iter().rev());
        }
        out
    }

    /// Every node in every diagram, in a stable order.
    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.diagrams().into_iter().flat_map(|d| &d.nodes)
    }

    /// Every wire in every diagram, in a stable order.
    pub fn wires(&self) -> impl Iterator<Item = &Wire> {
        self.diagrams().into_iter().flat_map(|d| &d.wires)
    }
}

struct Builder<'h, 'a> {
    /// Defined objects by unique id.
    objects: HashMap<i32, &'h HeapNode<'a>>,
    sources: HashSet<i32>,
    sinks: HashSet<i32>,
    /// Diagrams and nodes already built, so a reference cycle cannot recurse.
    seen: HashSet<i32>,
    types: &'h TypeTable,
    names: &'h HashMap<i32, String>,
}

fn rect(node: &HeapNode<'_>, field: &str) -> Option<Rect> {
    node.child_named(field)
        .and_then(HeapNode::bytes)
        .and_then(Rect::parse)
}

fn class_of(node: &HeapNode<'_>) -> Option<String> {
    let id = node.class()?;
    Some(
        node.class_name()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("class{id}")),
    )
}

impl<'h, 'a> Builder<'h, 'a> {
    fn index(&mut self, node: &'h HeapNode<'a>) {
        if let (Some(_), Some(uid)) = (node.class(), node.uid()) {
            self.objects.entry(uid).or_insert(node);
        }
        if node.class_name() == Some("signal") {
            let mut terminals = self.list(node, "termList").filter_map(HeapNode::uid);
            self.sources.extend(terminals.next());
            self.sinks.extend(terminals);
        }
        for child in &node.children {
            self.index(child);
        }
    }

    /// Record object definitions only, ignoring any signals.
    fn index_objects(&mut self, node: &'h HeapNode<'a>) {
        if let (Some(_), Some(uid)) = (node.class(), node.uid()) {
            self.objects.entry(uid).or_insert(node);
        }
        for child in &node.children {
            self.index_objects(child);
        }
    }

    /// Elements of a named list child, e.g. `nodeList`.
    fn list(
        &self,
        node: &'h HeapNode<'a>,
        name: &str,
    ) -> impl Iterator<Item = &'h HeapNode<'a>> + use<'h, 'a> {
        node.child_named(name)
            .into_iter()
            .flat_map(|list| &list.children)
    }

    /// The object an element stands for: itself if it is a definition,
    /// otherwise the object its `uid` refers to.
    fn resolve(&self, element: &'h HeapNode<'a>) -> Option<&'h HeapNode<'a>> {
        if element.class().is_some() {
            Some(element)
        } else {
            self.objects.get(&element.uid()?).copied()
        }
    }

    fn diagram(&mut self, diag: &'h HeapNode<'a>) -> Diagram {
        let uid = diag.uid().unwrap_or(0);
        let mut out = Diagram {
            uid,
            ..Diagram::default()
        };
        if !self.seen.insert(uid) {
            return out;
        }
        let elements: Vec<_> = self.list(diag, "nodeList").collect();
        for element in elements {
            let Some(object) = self.resolve(element) else {
                continue;
            };
            let Some(node_uid) = object.uid() else {
                continue;
            };
            if self.seen.insert(node_uid) {
                out.nodes.push(self.node(object, node_uid));
            }
        }
        let signals: Vec<_> = self.list(diag, "signalList").collect();
        for element in signals {
            let Some(signal) = self.resolve(element) else {
                continue;
            };
            let mut terminals = self.list(signal, "termList").filter_map(HeapNode::uid);
            if let (Some(wire_uid), Some(source)) = (signal.uid(), terminals.next()) {
                out.wires.push(Wire {
                    uid: wire_uid,
                    source,
                    sinks: terminals.collect(),
                });
            }
        }
        out
    }

    fn node(&mut self, object: &'h HeapNode<'a>, uid: i32) -> Node {
        let terminals = self
            .list(object, "termList")
            .filter_map(|element| self.resolve(element))
            .filter_map(|term| self.terminal(term))
            .collect();
        let nested: Vec<_> = self
            .list(object, "diagramList")
            .filter_map(|element| self.resolve(element))
            .collect();
        Node {
            uid,
            class: class_of(object).unwrap_or_default(),
            name: self.names.get(&uid).cloned(),
            primitive: object.child_named("primResID").and_then(HeapNode::uint),
            bounds: rect(object, "bounds"),
            terminals,
            diagrams: nested.into_iter().map(|d| self.diagram(d)).collect(),
        }
    }

    fn terminal(&self, term: &'h HeapNode<'a>) -> Option<Terminal> {
        let uid = term.uid()?;
        let dco_element = term.child_named("dco");
        let dco = dco_element.and_then(|element| self.resolve(element));
        let flags = dco
            .and_then(|d| d.child_named("objFlags"))
            .and_then(HeapNode::uint)
            .unwrap_or(0);
        let type_id = dco
            .and_then(|d| d.child_named("typeDesc"))
            .and_then(HeapNode::uint);

        let is_source = self.sources.contains(&uid);
        let is_sink = self.sinks.contains(&uid);
        // A wire states the direction outright; the flag decides otherwise.
        let direction = if is_source {
            Direction::Output
        } else if is_sink || flags & DCO_IS_OUTPUT == 0 {
            Direction::Input
        } else {
            Direction::Output
        };
        let bounds = dco
            .and_then(|d| rect(d, "termBounds"))
            .or_else(|| rect(term, "termBounds"))
            .or_else(|| rect(term, "bounds"));
        Some(Terminal {
            uid,
            bounds,
            direction,
            wired: is_source || is_sink,
            data_type: type_id.and_then(|id| self.types.heap_type(id)),
            dco: dco_element.and_then(HeapNode::uid),
            dco_class: dco.and_then(class_of),
        })
    }
}
