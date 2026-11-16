//! The block diagram as a dataflow graph: nodes, terminals and wires.
//!
//! Built from the `BDHb` object heap. In the heap every object is defined
//! once, with a class and a unique id, and is referred to elsewhere by that
//! id. A diagram lists its nodes and its signals (wires); a node lists its
//! terminals; a signal lists the terminals it connects, source first.
//! Structures such as loops and case structures are nodes that own nested
//! diagrams.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::Serialize;

use crate::blocks::heap::{Content, Heap, HeapNode};
use crate::blocks::linkinfo;
use crate::blocks::types::{printable, serialize_items, DataType, NumericType, TypeTable, Value};
use crate::blocks::zlib;
use crate::container::RsrcFile;
use crate::error::Result;
use crate::reader::decode_text;

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

/// Bit of a loop condition terminal's `objFlags` that is set for "Stop if
/// True" and clear for "Continue if True".
///
/// Checked on 290 loops saved by versions 13 to 25: the flags are `0x10001`
/// (274) or `0x1` (16); see [`LTST_ALWAYS_SET`]. All seven loops whose
/// terminal is fed by a constant True have the bit set and the one fed by
/// a constant False has it clear; the opposite reading would make all
/// eight loops run forever.
const STOP_IF_TRUE: u32 = 0x1_0000;

/// Bit of a loop condition terminal's `objFlags` that is set on every one
/// of those 290 loops. A terminal without it has no readable flags, and
/// its polarity is then not taken from them.
const LTST_ALWAYS_SET: u32 = 0x1;

/// Every field that a case structure object has in 1,713 case structures
/// of 1,773 real files. The field that names the default case is absent
/// when the default is frame 0 (1,153 of them), so its absence is
/// trusted only in an object with no field beyond these.
const SELECT_FIELDS: [&str; 17] = [
    "SelectDefaultCase",
    "SelectNRightType",
    "SelectRangeArray32",
    "SelectSelLabFlags",
    "SelectStringArray",
    "bounds",
    "caseSelDCO",
    "clumpNum",
    "contRect",
    "dIdx",
    "diagramList",
    "label",
    "objFlags",
    "selString",
    "shortCount",
    "termList",
    "tunnelList",
];

/// Bit of an output tunnel's `objFlags` that is set when at least one frame
/// leaves the tunnel unwired and the type's default value is used there.
///
/// Checked on 3,761 output tunnels of case structures: the bit is set on
/// all 637 with an unwired frame and on none of the others.
const DEFAULT_IF_UNWIRED: u32 = 0x10_0000;

/// Data object classes of tunnels whose layout has been verified on real
/// files: one inner terminal per frame and one outer terminal, all sharing
/// the data object, with the output flag agreeing with the wires.
const TUNNEL_CLASSES: [&str; 4] = ["lpTun", "selTun", "seqTun", "commentTun"];

/// Attribute of a list field that states how many elements it has.
const ATTR_ELEMENTS: i32 = -5;

/// Most selector table entries read from one case structure.
const MAX_SELECTOR_ENTRIES: usize = 65_536;

/// Which way data flows through a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// The terminal takes data in.
    #[default]
    Input,
    /// The terminal produces data; a wire attached to it starts there.
    Output,
}

/// What a tunnel does to the value that crosses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelMode {
    /// The value passes unchanged.
    Plain,
    /// A loop tunnel that indexes an array on the way in or builds one on
    /// the way out.
    AutoIndex,
    /// A loop output tunnel that concatenates the arrays of each iteration.
    Concatenate,
    /// A mode this crate does not know.
    Other,
}

/// A terminal that is one side of a tunnel through a structure's border.
///
/// The two sides of a tunnel are separate terminals with the same
/// [`Terminal::dco`]: the outer one belongs to the structure node, the inner
/// ones (one per frame) to the first node of each nested diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Tunnel {
    /// Whether the value leaves the structure.
    pub output: bool,
    /// What a loop does with the value at its border.
    pub mode: TunnelMode,
    /// A loop output tunnel that only takes the iterations a condition
    /// selects.
    pub conditional: bool,
    /// An output tunnel that at least one frame leaves unwired, with "Use
    /// Default If Unwired" supplying the type's default value there: zero
    /// (the first item of an enumeration), False, or the empty string.
    pub default_if_unwired: bool,
}

/// What a case selector is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectorKind {
    /// A Boolean: the frames are False and True.
    Boolean,
    /// A signed or unsigned integer.
    Integer,
    /// An enumeration.
    Enum,
    /// A string.
    String,
    /// An error cluster: the frames are No Error and Error.
    ErrorCluster,
}

/// One entry of a case structure's selector table.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectorMatch {
    /// A value, or an inclusive range of values, of a Boolean (0 is False,
    /// 1 is True), integer, enumeration or error code selector. A missing
    /// end is open: `start: Some(2), end: None` is the label `2..`.
    Range {
        /// First value of the range; `None` for a range open below.
        start: Option<i32>,
        /// Last value of the range; `None` for a range open above.
        end: Option<i32>,
    },
    /// A string the selector is compared with.
    Text(String),
}

/// Which values select which frame of a case structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaseSelector {
    /// The type of the selector.
    pub kind: SelectorKind,
    /// Item names of an enumeration selector, in value order. The list
    /// is shared with the type table, not copied per case structure.
    #[serde(
        skip_serializing_if = "<[String]>::is_empty",
        serialize_with = "serialize_items"
    )]
    pub enum_items: Arc<[String]>,
    /// File name of the type definition the selector's type is an
    /// instance of, e.g. `State.ctl`. The item names above are then the
    /// copy this file stored when it was saved, which the type definition
    /// may no longer agree with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_definition: Option<String>,
    /// The values that select each frame, one list per entry of
    /// [`Node::diagrams`]. Only the default frame may have none (and any
    /// frame of a Boolean or error cluster selector, which has no
    /// default).
    pub frames: Vec<Vec<SelectorMatch>>,
    /// Index of the frame that handles every value without an entry.
    /// Boolean and error cluster selectors have none.
    pub default_frame: Option<u32>,
}

impl CaseSelector {
    /// The frame's label as the editor draws it, e.g. `"Idle", Default`,
    /// `1, 3..5`, `True` or `No Error`.
    pub fn label(&self, frame: usize) -> String {
        let mut parts: Vec<String> = self
            .frames
            .get(frame)
            .into_iter()
            .flatten()
            .map(|entry| self.entry_label(entry))
            .collect();
        if self.default_frame.is_some_and(|d| d as usize == frame) {
            parts.push("Default".to_owned());
        }
        parts.join(", ")
    }

    fn value_label(&self, value: i32) -> String {
        match self.kind {
            SelectorKind::Boolean => if value != 0 { "True" } else { "False" }.to_owned(),
            SelectorKind::Enum => match usize::try_from(value)
                .ok()
                .and_then(|index| self.enum_items.get(index))
            {
                Some(name) => format!("\"{}\"", printable(name)),
                None => value.to_string(),
            },
            _ => value.to_string(),
        }
    }

    fn entry_label(&self, entry: &SelectorMatch) -> String {
        let (start, end) = match entry {
            SelectorMatch::Text(text) => return format!("\"{}\"", printable(text)),
            SelectorMatch::Range { start, end } => (*start, *end),
        };
        if self.kind == SelectorKind::ErrorCluster {
            // Two stored forms: plain values 0 and 1, or an open range up
            // to the smallest code for "No Error" and a fully open range
            // for "Error".
            match (start, end) {
                (Some(0), Some(0)) | (None, Some(i32::MIN)) => return "No Error".to_owned(),
                (Some(1), Some(1)) | (None, None) => return "Error".to_owned(),
                _ => {}
            }
        }
        match (start, end) {
            (Some(start), Some(end)) if start == end => self.value_label(start),
            (start, end) => format!(
                "{}..{}",
                start.map(|v| self.value_label(v)).unwrap_or_default(),
                end.map(|v| self.value_label(v)).unwrap_or_default()
            ),
        }
    }
}

/// The condition terminal of a loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct StopTerminal {
    /// Data object of the terminal; the terminal itself is the one with
    /// this [`Terminal::dco`] on the first node of the loop's diagram.
    pub dco: i32,
    /// True for "Stop if True", false for "Continue if True".
    pub stop_if_true: bool,
}

/// A shift register of a loop: the data objects of its two sides.
///
/// Each side has an outer terminal on the loop node and an inner terminal
/// on the first node of the loop's diagram, both with the side's data
/// object as their [`Terminal::dco`]. The inner left terminal supplies the
/// value at the start of an iteration and the inner right terminal takes
/// the value for the next one; the outer left terminal takes the initial
/// value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ShiftRegister {
    /// Left sides. More than one means a stacked register, which holds the
    /// values of several earlier iterations; their order is not verified.
    pub left: Vec<i32>,
    /// Data object of the right side, which takes the value for the next
    /// iteration.
    pub right: i32,
}

/// The parts of a loop that carry state from one iteration to the next.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct LoopInfo {
    /// `None` when the loop has no condition terminal (a plain for loop).
    pub stop: Option<StopTerminal>,
    /// Only registers whose two sides name each other are listed.
    pub shift_registers: Vec<ShiftRegister>,
}

/// A rectangle in diagram pixels, as stored in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Rect {
    /// Top edge.
    pub top: i16,
    /// Left edge.
    pub left: i16,
    /// Bottom edge.
    pub bottom: i16,
    /// Right edge.
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

    /// Width in pixels: `right - left`.
    pub fn width(&self) -> i32 {
        i32::from(self.right) - i32::from(self.left)
    }

    /// Height in pixels: `bottom - top`.
    pub fn height(&self) -> i32 {
        i32::from(self.bottom) - i32::from(self.top)
    }
}

/// A point on a node where a wire can attach.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Terminal {
    /// Unique id of the terminal object; wires refer to terminals by it.
    pub uid: i32,
    /// Position: relative to the owning node's top-left corner, except for
    /// front-panel terminals, which are positioned in their diagram.
    pub bounds: Option<Rect>,
    /// Which way data flows; see [`direction_is_reliable`](Self::direction_is_reliable).
    pub direction: Direction,
    /// Whether any wire attaches to the terminal.
    pub wired: bool,
    /// `None` when the type table has no entry for the terminal.
    pub data_type: Option<DataType>,
    /// Id of the data object behind the terminal. The two sides of a tunnel
    /// are separate terminals that share one data object.
    pub dco: Option<i32>,
    /// Class of that data object, e.g. `parm`, `selTun`, `lSR`.
    pub dco_class: Option<String>,
    /// How a numeric terminal's value is stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numeric: Option<NumericType>,
    /// The value of a block-diagram constant of a simple type. A constant
    /// is a terminal of its diagram's first node with the data object
    /// class `bDConstDCO`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constant: Option<Value>,
    /// Set when the terminal is one side of a tunnel of a verified class.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel: Option<Tunnel>,
    /// Which parameter the terminal is: for a built-in function its
    /// position in the function's parameter list (0 is usually the
    /// output), for a subVI call its connector pane slot. Unlike the order
    /// of [`Node::terminals`], this identifies the parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<u32>,
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

/// An object on a diagram: a function, subVI call, structure or the
/// diagram's boundary.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Node {
    /// Unique id of the node object in the heap.
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
    /// Terminals in the order the node's `termList` gives them.
    pub terminals: Vec<Terminal>,
    /// Nested diagrams of a structure, one per frame or case.
    pub diagrams: Vec<Diagram>,
    /// The selector table of a case structure whose selector is a Boolean,
    /// an integer, an enumeration, a string or an error cluster.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<CaseSelector>,
    /// Condition terminal and shift registers of a while or for loop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_info: Option<LoopInfo>,
}

/// A wire (a `signal` object): one source terminal feeding any number of
/// sinks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Wire {
    /// Unique id of the signal object in the heap.
    pub uid: i32,
    /// Uid of the terminal that drives the wire.
    pub source: i32,
    /// Uids of the terminals the wire feeds.
    pub sinks: Vec<i32>,
}

/// Something the heap holds that the decoded [`Diagram`] does not carry
/// over faithfully.
///
/// The decoder is tolerant: it skips what it cannot resolve so that the
/// rest of a damaged or unfamiliar file stays usable. That makes the
/// *absence* of a wire or a node evidence only in a diagram without a
/// recorded loss. No loss is recorded for any of the 1,773 real files the
/// decoder was checked on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Loss {
    /// An element of the node list that names no object or an object
    /// without an id, or a node list that does not hold as many elements
    /// as it states. A node that no list names at all is noticed through
    /// its terminals; one without terminals is not noticed.
    Node,
    /// A node listed a second time, here or in another diagram.
    RepeatedNode,
    /// An element of a node's terminal list that names no terminal object
    /// or one without an id, or a terminal object that no decoded node
    /// lists.
    Terminal,
    /// A terminal id used by two terminals.
    RepeatedTerminal,
    /// An element of a structure's frame list that names no diagram, a
    /// diagram reached a second time, or a diagram object that was never
    /// reached.
    Frame,
    /// An element of the signal list that names no signal object, a
    /// signal without an id, with fewer than two ends or with an end that
    /// has no id, or a signal object that no decoded diagram lists.
    Wire,
    /// A signal id used by two wires.
    RepeatedWire,
    /// A wire end that is not a terminal of a node of the wire's diagram.
    WireEnd,
    /// A terminal that is the source of two wires, a sink of two wires,
    /// listed twice by one wire, or both a source and a sink.
    WireConflict,
    /// An object id defined twice in the heap, so that references to it
    /// are ambiguous.
    RepeatedObject,
    /// An object with a field whose tag is not known, or with two fields
    /// of one tag. A field that is absent has a meaning (zero; frame 0
    /// for the default case), so a field hidden behind a damaged tag
    /// changes what the object says without leaving a gap. None of the
    /// 572,878 objects in the diagram heaps of 1,773 real files has
    /// either, apart from field 0 of the classes that number their own
    /// fields.
    Field,
    /// The heap's byte stream is irregular
    /// ([`Heap::irregular`](crate::blocks::heap::Heap::irregular)), in
    /// the diagram heap or the front panel heap: nodes may have been
    /// swallowed without leaving any trace in the tree.
    Stream,
}
/// One diagram: the top-level block diagram, or one frame or case of a
/// structure.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Diagram {
    /// Unique id of the `diag` object, or 0 if it has none.
    pub uid: i32,
    /// Nodes from the diagram's `nodeList`, in order.
    pub nodes: Vec<Node>,
    /// Wires from the diagram's `signalList`, in order.
    pub wires: Vec<Wire>,
    /// The kinds of loss met while decoding this diagram, sorted and
    /// without repeats; empty for a diagram that decoded completely.
    /// Losses that cannot be placed (an object nothing lists, an id
    /// defined twice) are recorded on the root diagram. Losses inside a
    /// nested diagram are recorded there, not here; see
    /// [`BlockDiagram::is_complete`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub losses: Vec<Loss>,
}

/// The block diagram of a VI as a tree of diagrams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockDiagram {
    /// The top-level diagram; structures on it hold the nested ones.
    pub root: Diagram,
}

impl BlockDiagram {
    /// Decode the block diagram of a file. Returns `Ok(None)` when the file
    /// has no block diagram heap (for example a control).
    ///
    /// # Errors
    ///
    /// If the `BDHb` block does not decompress or parse. The type table and
    /// front-panel heap are optional: when they are missing or do not parse,
    /// terminals simply lack types or front-panel objects.
    pub fn from_rsrc(file: &RsrcFile<'_>) -> Result<Option<Self>> {
        let Some(raw) = file.first(b"BDHb") else {
            return Ok(None);
        };
        let data = zlib::decompress(raw)?;
        Self::from_inflated_heap(file, &data)
    }

    /// Decode the block diagram from the already inflated `BDHb` heap,
    /// taking the types, the names of linked files and the front panel
    /// from the file. Returns `Ok(None)` when the heap holds no diagram.
    ///
    /// # Errors
    ///
    /// Returns an error when the heap's byte stream cannot be read.
    pub fn from_inflated_heap(file: &RsrcFile<'_>, data: &[u8]) -> Result<Option<Self>> {
        let heap = Heap::parse(data)?;

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
                        names
                            .entry(uid.cast_signed())
                            .or_insert_with(|| name.to_owned());
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
            terminal_uids: HashSet::new(),
            wire_uids: HashSet::new(),
            heap: Census::default(),
            built: Census::default(),
            repeated_object: false,
            odd_field: false,
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
        let mut root = builder.diagram(root);
        // What the heap defines must all have arrived somewhere: a list
        // whose field tag is damaged takes its elements out of sight
        // without leaving an element that fails to resolve.
        let (counted, built) = (builder.heap, builder.built);
        let unplaced = [
            (counted.terminals != built.terminals, Loss::Terminal),
            (counted.diagrams != built.diagrams, Loss::Frame),
            (counted.signals != built.signals, Loss::Wire),
            (builder.repeated_object, Loss::RepeatedObject),
            (builder.odd_field, Loss::Field),
            (
                heap.irregular || panel.is_some_and(|panel| panel.irregular),
                Loss::Stream,
            ),
        ];
        root.losses.extend(
            unplaced
                .into_iter()
                .filter_map(|(lost, loss)| lost.then_some(loss)),
        );
        root.losses.sort();
        root.losses.dedup();
        Some(Self { root })
    }

    /// Whether every diagram decoded without a recorded [`Loss`]. Only
    /// then does a missing wire or node mean that the file has none.
    pub fn is_complete(&self) -> bool {
        self.diagrams().iter().all(|d| d.losses.is_empty())
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

/// How many objects of each kind there are, for checking that everything
/// the heap defines arrived in the decoded diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Census {
    /// Objects of the terminal classes.
    terminals: usize,
    /// Objects of the class `diag`.
    diagrams: usize,
    /// Objects of the class `signal`.
    signals: usize,
}

/// What an object of the diagram heap is, for the [`Census`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Terminal,
    Diagram,
    Signal,
    Other,
}

fn kind_of(object: &HeapNode<'_>) -> Kind {
    match object.class_name() {
        Some("signal") => Kind::Signal,
        Some("diag") => Kind::Diagram,
        Some("term" | "fPTerm") => Kind::Terminal,
        _ => Kind::Other,
    }
}

struct Builder<'h, 'a> {
    /// Defined objects by unique id.
    objects: HashMap<i32, &'h HeapNode<'a>>,
    sources: HashSet<i32>,
    sinks: HashSet<i32>,
    /// Diagrams and nodes already built, so a reference cycle cannot recurse.
    seen: HashSet<i32>,
    /// Ids of the terminals and wires already built.
    terminal_uids: HashSet<i32>,
    wire_uids: HashSet<i32>,
    /// What the heap defines and what was built from it.
    heap: Census,
    built: Census,
    /// Whether an object id is defined twice in the diagram heap.
    repeated_object: bool,
    /// Whether an object has a field of an unknown tag or a field twice.
    odd_field: bool,
    types: &'h TypeTable,
    names: &'h HashMap<i32, String>,
}

fn rect(node: &HeapNode<'_>, field: &str) -> Option<Rect> {
    node.child_named(field)
        .and_then(HeapNode::bytes)
        .and_then(Rect::parse)
}

fn flags(object: &HeapNode<'_>) -> u32 {
    object
        .child_named("objFlags")
        .and_then(HeapNode::uint)
        .unwrap_or(0)
}

fn class_of(node: &HeapNode<'_>) -> Option<String> {
    let id = node.class()?;
    Some(
        node.class_name()
            .map_or_else(|| format!("class{id}"), str::to_owned),
    )
}

impl<'h, 'a> Builder<'h, 'a> {
    fn index(&mut self, node: &'h HeapNode<'a>) {
        if node.class().is_some() {
            Self::count(&mut self.heap, node);
            // Only objects of a known class: the fields of an unknown
            // one cannot be judged. Field 0 is the first field of the
            // classes that number their own.
            if node.class_name().is_some() && !self.odd_field {
                let mut tags = HashSet::new();
                self.odd_field = node.children.iter().any(|field| {
                    (field.tag != 0 && field.tag_name().is_none()) || !tags.insert(field.tag)
                });
            }
            if let Some(uid) = node.uid() {
                match self.objects.entry(uid) {
                    Entry::Occupied(_) => self.repeated_object = true,
                    Entry::Vacant(entry) => {
                        entry.insert(node);
                    }
                }
            }
        }
        if node.class_name() == Some("signal") {
            let mut terminals = Self::list(node, "termList").filter_map(HeapNode::uid);
            self.sources.extend(terminals.next());
            self.sinks.extend(terminals);
        }
        for child in &node.children {
            self.index(child);
        }
    }

    fn count(census: &mut Census, object: &HeapNode<'_>) {
        match kind_of(object) {
            Kind::Terminal => census.terminals += 1,
            Kind::Diagram => census.diagrams += 1,
            Kind::Signal => census.signals += 1,
            Kind::Other => {}
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
        node: &'h HeapNode<'a>,
        name: &str,
    ) -> impl Iterator<Item = &'h HeapNode<'a>> + use<'h, 'a> {
        node.child_named(name)
            .into_iter()
            .flat_map(|list| &list.children)
    }

    /// Whether a named list child, if there is one, holds as many
    /// elements as it says it does. Every list states its length: all
    /// 112,335 node, signal, terminal, frame and selector lists of 1,773
    /// real files agree with theirs. A list that does not has lost or
    /// gained elements in a way that left the heap readable.
    fn counted(node: &HeapNode<'_>, name: &str) -> bool {
        node.child_named(name).is_none_or(|list| {
            list.attr(ATTR_ELEMENTS)
                .is_some_and(|stated| usize::try_from(stated) == Ok(list.children.len()))
        })
    }

    /// Whether every field of an object is one of the named ones.
    fn only_fields(object: &HeapNode<'_>, fields: &[&str]) -> bool {
        let named =
            |field: &HeapNode<'_>| field.tag_name().is_some_and(|name| fields.contains(&name));
        object.children.iter().all(named)
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
        if diag.uid().is_none() || !self.seen.insert(uid) {
            out.losses.push(Loss::Frame);
            return out;
        }
        Self::count(&mut self.built, diag);
        let mut losses = Vec::new();
        if !Self::counted(diag, "nodeList") {
            losses.push(Loss::Node);
        }
        if !Self::counted(diag, "signalList") {
            losses.push(Loss::Wire);
        }
        let elements: Vec<_> = Self::list(diag, "nodeList").collect();
        for element in elements {
            let Some((object, node_uid)) = self
                .resolve(element)
                .and_then(|object| Some((object, object.uid()?)))
            else {
                losses.push(Loss::Node);
                continue;
            };
            if self.seen.insert(node_uid) {
                out.nodes.push(self.node(object, node_uid, &mut losses));
            } else {
                losses.push(Loss::RepeatedNode);
            }
        }
        let own: HashSet<i32> = out
            .nodes
            .iter()
            .flat_map(|node| &node.terminals)
            .map(|terminal| terminal.uid)
            .collect();
        let (mut sources, mut sinks) = (HashSet::new(), HashSet::new());
        let signals: Vec<_> = Self::list(diag, "signalList").collect();
        for element in signals {
            let Some(signal) = self
                .resolve(element)
                .filter(|signal| kind_of(signal) == Kind::Signal)
            else {
                losses.push(Loss::Wire);
                continue;
            };
            let listed = Self::list(signal, "termList").count();
            let mut terminals = Self::list(signal, "termList").filter_map(HeapNode::uid);
            let (Some(wire_uid), Some(source)) = (signal.uid(), terminals.next()) else {
                losses.push(Loss::Wire);
                continue;
            };
            let wire = Wire {
                uid: wire_uid,
                source,
                sinks: terminals.collect(),
            };
            if listed < 2 || wire.sinks.len() + 1 != listed || !Self::counted(signal, "termList") {
                losses.push(Loss::Wire);
            }
            if self.wire_uids.insert(wire_uid) {
                Self::count(&mut self.built, signal);
            } else {
                losses.push(Loss::RepeatedWire);
            }
            if !sources.insert(wire.source) {
                losses.push(Loss::WireConflict);
            }
            for sink in &wire.sinks {
                if !sinks.insert(*sink) {
                    losses.push(Loss::WireConflict);
                }
            }
            if std::iter::once(&wire.source)
                .chain(&wire.sinks)
                .any(|end| !own.contains(end))
            {
                losses.push(Loss::WireEnd);
            }
            out.wires.push(wire);
        }
        if !sources.is_disjoint(&sinks) {
            losses.push(Loss::WireConflict);
        }
        losses.sort();
        losses.dedup();
        out.losses = losses;
        out
    }

    fn node(&mut self, object: &'h HeapNode<'a>, uid: i32, losses: &mut Vec<Loss>) -> Node {
        let mut terminals = Vec::new();
        if !Self::counted(object, "termList") {
            losses.push(Loss::Terminal);
        }
        if !Self::counted(object, "diagramList") {
            losses.push(Loss::Frame);
        }
        for element in Self::list(object, "termList") {
            let term = self
                .resolve(element)
                .filter(|term| kind_of(term) == Kind::Terminal);
            let Some((term, terminal)) = term.and_then(|term| Some((term, self.terminal(term)?)))
            else {
                losses.push(Loss::Terminal);
                continue;
            };
            if self.terminal_uids.insert(terminal.uid) {
                Self::count(&mut self.built, term);
            } else {
                losses.push(Loss::RepeatedTerminal);
            }
            terminals.push(terminal);
        }
        let frames = Self::list(object, "diagramList").count();
        let nested: Vec<_> = Self::list(object, "diagramList")
            .filter_map(|element| self.resolve(element))
            .filter(|diagram| kind_of(diagram) == Kind::Diagram)
            .collect();
        if nested.len() != frames {
            losses.push(Loss::Frame);
        }
        let class = class_of(object).unwrap_or_default();
        // A frame that could not be resolved would shift the indices the
        // selector table refers to.
        let selector = (class == "select" && nested.len() == frames)
            .then(|| self.selector(object, frames))
            .flatten();
        let loop_info =
            matches!(class.as_str(), "whileLoop" | "forLoop").then(|| self.loop_info(object));
        Node {
            uid,
            class,
            name: self.names.get(&uid).cloned(),
            primitive: object.child_named("primResID").and_then(HeapNode::uint),
            bounds: rect(object, "bounds"),
            terminals,
            diagrams: nested.into_iter().map(|d| self.diagram(d)).collect(),
            selector,
            loop_info,
        }
    }

    /// Decode a case structure's selector table. Returns `None` for a
    /// selector type or an encoding that is not understood, so that a
    /// table is either right or absent.
    fn selector(&self, object: &'h HeapNode<'a>, frames: usize) -> Option<CaseSelector> {
        let selector = self.resolve(object.child_named("caseSelDCO")?)?;
        let type_id = selector.child_named("typeDesc")?.uint()?;
        let info = self.types.heap_type_info(type_id)?;
        let kind = match (info.data_type, info.numeric) {
            (DataType::Boolean, _) => SelectorKind::Boolean,
            (DataType::ErrorCluster, _) => SelectorKind::ErrorCluster,
            (DataType::String, _) => SelectorKind::String,
            (DataType::Numeric, Some(numeric)) if numeric.is_enum() => SelectorKind::Enum,
            (DataType::Numeric, Some(numeric)) if numeric.is_integer() => SelectorKind::Integer,
            _ => return None,
        };
        let strings: Vec<String> = Self::list(object, "SelectStringArray")
            .take(MAX_SELECTOR_ENTRIES)
            .map(|element| element.bytes().map(decode_text).unwrap_or_default())
            .collect();
        let mut table: Vec<Vec<SelectorMatch>> = vec![Vec::new(); frames];
        let entries = object.child_named("SelectRangeArray32")?;
        if !Self::counted(object, "SelectRangeArray32") {
            return None;
        }
        if entries.children.len() > MAX_SELECTOR_ENTRIES {
            return None;
        }
        for entry in &entries.children {
            // The five fields are private to the entry's class and number
            // 0 to 4. Each of the 3,982 entries in 1,773 real files has
            // every field exactly once and nothing else, so an entry of
            // any other shape is damaged: a missing frame index would
            // otherwise read as frame 0 and move a value to another case.
            let complete = entry.children.len() == 5
                && (0..5).all(|tag| entry.children.iter().filter(|c| c.tag == tag).count() == 1);
            if !complete {
                return None;
            }
            let field = |tag: i32| match entry.child(tag) {
                None => None,
                Some(child) => match child.content {
                    Content::Bytes(_) => child.int(),
                    _ => Some(0),
                },
            };
            let (start, end) = (field(0)?, field(1)?);
            let (start_type, end_type) = (field(2)?, field(3)?);
            let frame = usize::try_from(field(4)?).ok()?;
            let matched = if kind == SelectorKind::String {
                // Strings are indices into the string list; no string
                // range has been seen, so one is not guessed at.
                if (start_type, end_type) != (0, 0) || start != end {
                    return None;
                }
                SelectorMatch::Text(strings.get(usize::try_from(start).ok()?)?.clone())
            } else {
                // 0 is a single value, 1 the closed end of a range and 3
                // an open end, whose stored value is filler.
                let bound = |value: i32, range_type: i32| match range_type {
                    0 | 1 => Some(Some(value)),
                    3 => Some(None),
                    _ => None,
                };
                if (start_type == 0) != (end_type == 0) || (start_type == 0 && start != end) {
                    return None;
                }
                SelectorMatch::Range {
                    start: bound(start, start_type)?,
                    end: bound(end, end_type)?,
                }
            };
            table.get_mut(frame)?.push(matched);
        }
        let default_frame = match kind {
            SelectorKind::Boolean | SelectorKind::ErrorCluster => None,
            _ => match object.child_named("SelectDefaultCase") {
                // An absent field is frame 0. That reading is only safe
                // when the field cannot be hiding under another name: a
                // damaged tag that happens to be a known one leaves the
                // object with a field no case structure has.
                None if Self::only_fields(object, &SELECT_FIELDS) => Some(0),
                None => return None,
                Some(field) => match field.uint()? {
                    0xFF => None,
                    index if (index as usize) < frames => Some(index),
                    _ => return None,
                },
            },
        };
        if default_frame.is_some() && frames == 0 {
            return None;
        }
        // A frame that no value selects and that is not the default
        // cannot occur in a file the editor saved (0 of 287 enumeration,
        // integer and string tables), so such a table contradicts the
        // frame list. Boolean and error cluster selectors have no default
        // and are left as they are.
        let has_default = !matches!(kind, SelectorKind::Boolean | SelectorKind::ErrorCluster);
        let uncovered = |(frame, entries): (usize, &Vec<SelectorMatch>)| {
            entries.is_empty()
                && default_frame.and_then(|index| usize::try_from(index).ok()) != Some(frame)
        };
        if has_default && table.iter().enumerate().any(uncovered) {
            return None;
        }
        Some(CaseSelector {
            kind,
            enum_items: info.enum_items.unwrap_or_else(|| Arc::from([])),
            type_definition: info.type_definition,
            frames: table,
            default_frame,
        })
    }

    fn loop_info(&self, object: &'h HeapNode<'a>) -> LoopInfo {
        let stop = object
            .child_named("loopTestDCO")
            .and_then(|element| self.resolve(element))
            .filter(|test| test.class_name() == Some("lTst"))
            // An absent flags field would read as "continue if true".
            .filter(|test| flags(test) & LTST_ALWAYS_SET != 0)
            .and_then(|test| {
                Some(StopTerminal {
                    dco: test.uid()?,
                    stop_if_true: flags(test) & STOP_IF_TRUE != 0,
                })
            });
        let mut shift_registers: Vec<ShiftRegister> = Vec::new();
        for element in Self::list(object, "srDCOList") {
            let Some(right) = self
                .resolve(element)
                .filter(|right| right.class_name() == Some("rSR"))
            else {
                continue;
            };
            let Some(right_uid) = right.uid() else {
                continue;
            };
            let left: Vec<i32> = Self::list(right, "lsrDCOList")
                .filter_map(HeapNode::uid)
                .collect();
            // Both sides must name each other.
            let agree = !left.is_empty()
                && left.iter().all(|uid| {
                    self.objects.get(uid).is_some_and(|left| {
                        left.class_name() == Some("lSR")
                            && left.child_named("rsrDCO").and_then(HeapNode::uid) == Some(right_uid)
                    })
                });
            if agree && shift_registers.iter().all(|r| r.right != right_uid) {
                shift_registers.push(ShiftRegister {
                    left,
                    right: right_uid,
                });
            }
        }
        LoopInfo {
            stop,
            shift_registers,
        }
    }

    fn terminal(&self, term: &'h HeapNode<'a>) -> Option<Terminal> {
        let uid = term.uid()?;
        let dco_element = term.child_named("dco");
        let dco = dco_element.and_then(|element| self.resolve(element));
        let flags = dco.map_or(0, flags);
        let dco_class = dco.and_then(class_of);
        let class = dco_class.as_deref().unwrap_or_default();
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
        let kind = type_id.and_then(|id| self.types.heap_type_kind(id));
        let constant = (class == "bDConstDCO")
            .then(|| {
                let bytes = dco?.child_named("ConstValue")?.bytes()?;
                self.types.constant(type_id?, bytes)
            })
            .flatten();
        let tunnel = TUNNEL_CLASSES.contains(&class).then(|| Tunnel {
            output: flags & DCO_IS_OUTPUT != 0,
            mode: match dco
                .and_then(|d| d.child_named("TunnelType"))
                .map(HeapNode::uint)
            {
                None => TunnelMode::Plain,
                Some(Some(1)) => TunnelMode::AutoIndex,
                Some(Some(2)) => TunnelMode::Concatenate,
                Some(_) => TunnelMode::Other,
            },
            conditional: dco.is_some_and(|d| {
                d.child_named("IsConditional").is_some()
                    || d.child_named("LpTunConditionDCO").is_some()
            }),
            default_if_unwired: flags & DEFAULT_IF_UNWIRED != 0,
        });
        // An absent index is zero.
        let index = |field: &str| {
            dco.and_then(|d| d.child_named(field))
                .map_or(Some(0), HeapNode::uint)
        };
        let param = match class {
            "parm" => index("parmIndex"),
            "iUseDCO" | "polyIUseDCO" => index("paramIdx"),
            _ => None,
        };
        Some(Terminal {
            uid,
            bounds,
            direction,
            wired: is_source || is_sink,
            data_type: kind.map(|(data_type, _)| data_type),
            dco: dco_element.and_then(HeapNode::uid),
            numeric: kind.and_then(|(_, numeric)| numeric),
            constant,
            tunnel,
            param,
            dco_class,
        })
    }
}
