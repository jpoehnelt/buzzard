//! Finding state machines on a block diagram and building their models.
//!
//! # The classic pattern
//!
//! A machine is a while loop `W`, a shift register of `W`, and a case
//! structure `C` somewhere inside `W` whose selector is wired from the
//! register's left terminal, either directly or through tunnels that pass
//! a value in unchanged. The selector must be an enumeration, a string or
//! an integer. Every such `(W, register, C)` is one [`Machine`]:
//!
//! * a **state** is a frame of `C`, named by its case label (item names,
//!   strings, or numbers and ranges, joined with `, `; a default frame
//!   without values of its own is named `Default`). The machine is "in" a
//!   state during an iteration whose register value selects that frame;
//! * the **successors** of a state are the frames selected by the values
//!   that can reach the register's right terminal during such an
//!   iteration;
//! * the **stop** behaviour of a state is what can reach the loop's
//!   condition terminal during such an iteration;
//! * the **initial** states are the frames selected by the values wired
//!   to the register's left terminal from outside the loop.
//!
//! # How a value is followed
//!
//! A value is followed backwards from the terminal that consumes it. Only
//! these steps are understood; they are the only things found between a
//! constant and the register in the nine real state machine files the
//! pattern was studied on (editor versions 13, 17, 20 and 25):
//!
//! * a **constant** of a simple type ends the walk with its value;
//! * the **left terminal of the machine's own register** means "the
//!   current state", so the successor is the state itself;
//! * an **input tunnel** of any structure that passes its value unchanged
//!   continues at the wire outside;
//! * the built-in **Select** function gives either of its two inputs;
//! * an **output tunnel of a case, event or disable structure** gives the
//!   value any of its frames provides, except that for `C` itself only
//!   the frame of the state in question counts. A frame that leaves the
//!   tunnel unwired provides the type's default (zero, which is the first
//!   item of an enumeration; False; the empty string), but only when the
//!   file marks the tunnel as using the default;
//! * an **output tunnel of a stacked sequence** gives what its one wired
//!   frame provides, and one of a **nested while loop** what the loop's
//!   diagram provides.
//!
//! Anything else makes the result unknown: a subVI, any other function
//! (arithmetic, comparison, string functions), a control or indicator, a
//! local or global variable, a property node, a bundle or unbundle, an
//! auto-indexing, concatenating or conditional loop tunnel, the output
//! tunnel of a for loop (which yields a default when the loop runs zero
//! times), a flat sequence, a feedback node, another shift register, a
//! constant of a type that is not decoded, a wire whose two ends
//! contradict what the file says about their direction, and a walk that
//! gets too deep or too long. An unknown successor is
//! [`Successors::Unknown`], an unknown stop value [`Stop::Maybe`] and an
//! unknown initial value `None`. The model therefore over-approximates
//! the diagram: it never leaves out a transition the diagram can make.
//!
//! # From a value to a state
//!
//! A written value is resolved through the selector table of `C`, never
//! by comparing label text: a number selects the frame whose value or
//! range contains it, a string the frame that lists it, and a value no
//! frame lists selects the default frame. So a frame that handles several
//! values is one state, whichever of its values is written.
//!
//! * A constant counts only when its numeric type is exactly the type of
//!   the selector and of both register terminals, so that no hidden
//!   conversion can change it. An enumeration constant must also carry
//!   the name the selector's own item list has at its value.
//! * Selectors wider than 32 bits, and unsigned ones whose table holds
//!   negative numbers, are not resolved.
//! * A string that is not plain ASCII is not resolved, because its bytes
//!   cannot be compared reliably after decoding. Whether a case structure
//!   matches case-insensitively is not read from the file; a string that
//!   equals a label only when case is ignored is modelled as selecting
//!   either that frame or the default one.
//! * A non-empty string no label matches, even ignoring case, is recorded
//!   in [`State::unmatched`]: it runs the default frame, which is what
//!   the "undefined state name" finding reports. The empty string and a
//!   type default fall to the default frame without a finding, since
//!   leaving a tunnel unwired to return to the default state is an idiom.
//!
//! # Assumptions and limits
//!
//! * **Uninitialised registers.** A register with nothing wired to its
//!   left terminal keeps its value from the previous call of the VI, so
//!   its first state is unknown and nothing is claimed about
//!   reachability.
//! * **Stacked registers** (several left terminals) are skipped: which
//!   terminal holds the latest value was not verified on a real file.
//! * **Several case structures** in one loop that select on the same
//!   register give one machine each. Each is sound on its own; a case
//!   structure other than `C` on the path is treated as any other case
//!   structure (all frames).
//! * **Several registers.** Only the register that feeds the selector is
//!   the state; every other register is opaque data.
//! * **Event structures.** An event structure inside a frame contributes
//!   every one of its frames, whichever event they handle. When its
//!   timeout terminal is wired it also contributes the type's default,
//!   because a timeout that no frame handles ends the structure without
//!   running a frame, and which frame handles the timeout is not decoded.
//!   That it may wait forever is not modelled; it only removes behaviours.
//! * **Nesting.** `C` may sit inside other structures of `W`. In an
//!   iteration that does not execute `C` at all the state is still the
//!   frame the register value selects, and the values the other
//!   structures provide are included.
//! * **Stopping** means the loop's own condition terminal. A state stops
//!   `Always` or `Never` only when nothing but Boolean constants (and
//!   unwired-default tunnels) can reach the terminal in that state, read
//!   with the terminal's "stop if true" or "continue if true" setting.
//!   When the diagram calls the built-in Stop or Quit function anywhere,
//!   every state is `Maybe`. An abort from inside a subVI, by an error
//!   dialog or by the user is outside the model.
//! * **Integer selectors** are reported only when at least one frame
//!   writes a constant successor; otherwise the register is far more
//!   likely a counter than a state.
//! * **Not machines:** selectors that are Booleans or error clusters,
//!   selectors fed by a subVI, a function, a control or the loop counter,
//!   for loops, timed loops, and a state held in a cluster or a feedback
//!   node. A selector type or table encoding that `buzzard-rsrc` does not
//!   decode gives no machine.
//! * **Wire direction.** The file lists a wire's source first in all but
//!   a handful of wires per ten thousand. Every step checks the role of
//!   both ends against independent facts (tunnel direction flags,
//!   parameter indices, object classes) and gives up on a mismatch.
//! * **Stale type definitions.** An enumeration's item names are the copy
//!   stored in the file. If the type definition changed since the file
//!   was saved, the names shown are the old ones; values are unaffected.
//! * Not compared with a running editor: the semantics above come from
//!   the file format and the product documentation.
//!
//! # String queue machines
//!
//! In a JKI-style machine the register holds a whole queue of state names
//! as one string, and subVIs split and extend it. That is a pushdown
//! system computed by library code, not a finite automaton, and it is
//! **not modelled**. [`QueueMachine`] carries one heuristic check only:
//! names in string constants wired (directly, or through the steps above)
//! into an `Add State(s) to Queue` call, or into the queue register from
//! outside, that match no case label and so run the default case. A queue
//! string is read the way the real files use it: one state per line, the
//! name ending at `>>` (arguments follow), `//` starting a comment and
//! `/* */` enclosing one. In 16 of the 17 real machines studied every
//! enqueued name read this way has a case; the other, a documentation
//! example, enqueues three names it has no case for. The check proves
//! nothing: names built at run time or enqueued by other VIs are not
//! seen, the comment syntax is inferred from those files alone, and the
//! library is recognised by the names of its subVIs.

use std::collections::{BTreeSet, HashMap, HashSet};

use buzzard_rsrc::{
    BlockDiagram, CaseSelector, DataType, Diagram, Node, NumericType, SelectorKind, SelectorMatch,
    Terminal, TunnelMode, Value,
};
use serde::Serialize;

use super::model::{Machine, State, Stop, Successors};

/// Resource id of the built-in Select function. Its parameters are 0 the
/// output, 1 the value when false, 2 the selector, 3 the value when true
/// (164 instances in files of versions 13 to 25).
const SELECT: u32 = 1516;
/// Built-in functions that end the program: Stop and Quit.
const ABORTS: [u32; 2] = [1350, 1352];
/// Deepest chain of steps followed for one value.
const MAX_DEPTH: usize = 128;
/// Most steps spent on one value, so that a diagram built to branch
/// exponentially cannot stall the analysis.
const MAX_STEPS: usize = 20_000;
/// Name prefixes of the subVIs of the JKI state machine library.
const PARSE_QUEUE: &str = "Parse State Queue";
const ADD_TO_QUEUE: &str = "Add State(s) to Queue";

/// A string/queue state machine, which is recognised but not modelled.
/// See the module documentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueueMachine {
    /// Where the machine is on the diagram.
    pub location: String,
    /// Every case label, sorted.
    pub states: Vec<String>,
    /// Label of the default case, which runs for a name without a case.
    pub default: String,
    /// State names found in enqueued string constants, sorted.
    pub enqueued: Vec<String>,
    /// Enqueued names that match no case label. A heuristic, not a proof.
    pub unhandled: Vec<String>,
}

/// The machines found on one block diagram.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Extraction {
    pub machines: Vec<Machine>,
    pub queue_machines: Vec<QueueMachine>,
}

/// Find every state machine on the diagram. The result is deterministic.
pub fn extract(diagram: &BlockDiagram) -> Extraction {
    let index = Index::new(diagram);
    let mut out = Extraction::default();
    // Queue machine candidates: case structure, parser call, machine.
    let mut queues: Vec<(usize, usize, QueueMachine)> = Vec::new();
    for (case, node) in index.nodes.iter().enumerate() {
        let Some(selector) = &node.selector else {
            continue;
        };
        if node.class != "select" || selector.frames.len() != node.diagrams.len() {
            continue;
        }
        let Some(selected_by) = unique(node, |t| t.dco_class.as_deref() == Some("caseSel")) else {
            continue;
        };
        let Some((origin, terminal)) = index.origin(selected_by) else {
            continue;
        };
        if let Some(machine) = index.machine(case, selector, selected_by, origin, terminal) {
            out.machines.push(machine);
        } else if let Some(machine) = index.queue_machine(case, selector, origin) {
            queues.push((case, origin, machine));
        }
    }
    // The parser also hands out the arguments of a state, and the case
    // structures that select on those sit inside the case structure of
    // the states. Only the outermost one is the machine.
    let nested = |case: usize, parser: usize| {
        queues
            .iter()
            .any(|(outer, other, _)| *other == parser && index.is_inside(case, *outer))
    };
    out.queue_machines = queues
        .iter()
        .filter(|(case, parser, _)| !nested(*case, *parser))
        .map(|(_, _, machine)| machine.clone())
        .collect();
    out
}

/// The one terminal of a node that satisfies a condition.
fn unique(node: &Node, wanted: impl Fn(&Terminal) -> bool) -> Option<&Terminal> {
    let mut found = node.terminals.iter().filter(|t| wanted(t));
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

/// The one terminal of a node with the given data object.
fn with_dco(node: &Node, dco: i32) -> Option<&Terminal> {
    unique(node, |t| t.dco == Some(dco))
}

/// What a value can be.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Atom {
    /// A constant and the numeric type of its terminal.
    Constant(Value, Option<NumericType>),
    /// The default of the type, from a tunnel a frame leaves unwired.
    TypeDefault,
    /// The value the machine's register holds in this iteration.
    Current,
}

/// Every value a wire can carry, or `None` when that is unknown.
type Values = Option<BTreeSet<Atom>>;

fn one(atom: Atom) -> Values {
    Some(BTreeSet::from([atom]))
}

fn union(a: Values, b: Values) -> Values {
    let (mut a, b) = (a?, b?);
    a.extend(b);
    Some(a)
}

/// The diagram flattened for lookups by id.
struct Index<'a> {
    nodes: Vec<&'a Node>,
    /// Index of the diagram each node sits in.
    node_diagram: Vec<usize>,
    /// Diagram index of each frame of each node.
    node_frames: Vec<Vec<usize>>,
    /// The structure node and frame number that own each diagram.
    diagram_parent: Vec<Option<(usize, usize)>>,
    /// Node index of each diagram's first node, its boundary.
    diagram_first: Vec<Option<usize>>,
    /// Terminal id to node index and position in the node.
    owner: HashMap<i32, (usize, usize)>,
    /// Terminals that must not be relied on: an id used twice, a sink of
    /// two different sources, or a terminal that is both source and sink.
    dubious: HashSet<i32>,
    /// Sink terminal to the source of its wire.
    source: HashMap<i32, i32>,
    /// Whether the diagram calls a function that ends the program.
    aborts: bool,
}

impl<'a> Index<'a> {
    fn new(diagram: &'a BlockDiagram) -> Self {
        let mut index = Self {
            nodes: Vec::new(),
            node_diagram: Vec::new(),
            node_frames: Vec::new(),
            diagram_parent: Vec::new(),
            diagram_first: Vec::new(),
            owner: HashMap::new(),
            dubious: HashSet::new(),
            source: HashMap::new(),
            aborts: false,
        };
        let mut sources: HashSet<i32> = HashSet::new();
        let mut stack: Vec<(&'a Diagram, Option<(usize, usize)>)> = vec![(&diagram.root, None)];
        while let Some((inner, parent)) = stack.pop() {
            let at = index.diagram_parent.len();
            index.diagram_parent.push(parent);
            index.diagram_first.push(None);
            if let Some((node, frame)) = parent {
                index.node_frames[node][frame] = at;
            }
            for (position, node) in inner.nodes.iter().enumerate() {
                let number = index.nodes.len();
                index.nodes.push(node);
                index.node_diagram.push(at);
                index
                    .node_frames
                    .push(vec![usize::MAX; node.diagrams.len()]);
                if position == 0 {
                    index.diagram_first[at] = Some(number);
                }
                index.aborts |=
                    node.class == "prim" && node.primitive.is_some_and(|id| ABORTS.contains(&id));
                for (place, terminal) in node.terminals.iter().enumerate() {
                    if index.owner.insert(terminal.uid, (number, place)).is_some() {
                        index.dubious.insert(terminal.uid);
                    }
                }
                for (frame, nested) in node.diagrams.iter().enumerate() {
                    stack.push((nested, Some((number, frame))));
                }
            }
            for wire in &inner.wires {
                sources.insert(wire.source);
                for &sink in &wire.sinks {
                    if index
                        .source
                        .insert(sink, wire.source)
                        .is_some_and(|earlier| earlier != wire.source)
                    {
                        index.dubious.insert(sink);
                    }
                }
            }
        }
        let both: Vec<i32> = sources
            .into_iter()
            .filter(|uid| index.source.contains_key(uid))
            .collect();
        index.dubious.extend(both);
        index
    }

    fn terminal(&self, uid: i32) -> Option<(usize, &'a Terminal)> {
        if self.dubious.contains(&uid) {
            return None;
        }
        let &(node, place) = self.owner.get(&uid)?;
        Some((node, &self.nodes[node].terminals[place]))
    }

    /// Whether the node is the boundary of its diagram: the first node,
    /// which carries the inner side of tunnels and the constants.
    fn is_boundary(&self, node: usize) -> bool {
        self.nodes[node].class == "sRN" && self.diagram_first[self.node_diagram[node]] == Some(node)
    }

    /// The structure a boundary node's diagram is a frame of.
    fn structure_of(&self, boundary: usize) -> Option<usize> {
        Some(self.diagram_parent[self.node_diagram[boundary]]?.0)
    }

    /// The source of the wire into a sink terminal. `None` when the
    /// terminal is unwired or its wire cannot be relied on.
    fn source_of(&self, sink: &Terminal) -> Option<(usize, &'a Terminal)> {
        self.terminal(sink.uid)?;
        self.terminal(*self.source.get(&sink.uid)?)
    }

    /// For the inner side of a tunnel that passes a value in unchanged:
    /// the outer side, on the structure.
    fn outside(&self, boundary: usize, inner: &Terminal) -> Option<&'a Terminal> {
        let tunnel = inner.tunnel?;
        if tunnel.output || tunnel.mode != TunnelMode::Plain || tunnel.conditional {
            return None;
        }
        let outer = with_dco(self.nodes[self.structure_of(boundary)?], inner.dco?)?;
        (outer.tunnel == Some(tunnel)).then_some(outer)
    }

    /// Where the value of a sink comes from, looking through tunnels that
    /// pass a value in unchanged.
    fn origin(&self, sink: &Terminal) -> Option<(usize, &'a Terminal)> {
        let (mut node, mut terminal) = self.source_of(sink)?;
        for _ in 0..MAX_DEPTH {
            if !self.is_boundary(node) {
                break;
            }
            let Some(outer) = self.outside(node, terminal) else {
                break;
            };
            (node, terminal) = self.source_of(outer)?;
        }
        Some((node, terminal))
    }

    /// Whether an event structure has nothing wired to its timeout
    /// terminal, so that it always runs exactly one frame.
    fn waits_forever(&self, structure: &Node) -> bool {
        unique(structure, |t| {
            t.dco_class.as_deref() == Some("eventTimeOut")
        })
        .is_some_and(|timeout| !timeout.wired && !self.source.contains_key(&timeout.uid))
    }

    /// The inner side of a structure's tunnel in each frame.
    fn inside(&self, structure: usize, outer: &Terminal) -> Option<Vec<&'a Terminal>> {
        let node = self.nodes[structure];
        let dco = outer.dco?;
        with_dco(node, dco).filter(|only| only.uid == outer.uid)?;
        self.node_frames[structure]
            .iter()
            .map(|&frame| {
                let boundary = (*self.diagram_first.get(frame)?)?;
                let inner = with_dco(self.nodes[boundary], dco)?;
                (self.is_boundary(boundary) && inner.tunnel == outer.tunnel).then_some(inner)
            })
            .collect()
    }

    /// Build the classic machine of a case structure whose selector comes
    /// from `origin`, if that is the left side of a while loop's register.
    fn machine(
        &self,
        case: usize,
        selector: &CaseSelector,
        selected_by: &Terminal,
        origin: usize,
        left_inner: &'a Terminal,
    ) -> Option<Machine> {
        if !matches!(
            selector.kind,
            SelectorKind::Enum | SelectorKind::Integer | SelectorKind::String
        ) || selector.frames.len() < 2
        {
            return None;
        }
        if left_inner.dco_class.as_deref() != Some("lSR") || !self.is_boundary(origin) {
            return None;
        }
        let owner = self.structure_of(origin)?;
        let looping = self.nodes[owner];
        if looping.class != "whileLoop" || looping.diagrams.len() != 1 {
            return None;
        }
        let info = looping.loop_info.as_ref()?;
        let left = left_inner.dco?;
        // A stacked register has several left sides and is not modelled.
        let register = info.shift_registers.iter().find(|r| r.left == [left])?;
        let boundary = self.nodes[origin];
        with_dco(boundary, left)?;
        let right_inner = with_dco(boundary, register.right)?;
        let left_outer = with_dco(looping, left)?;
        with_dco(looping, register.right)?;

        let resolver = Resolver::new(selector, selected_by, left_inner, right_inner);
        let names = state_names(selector);
        let mut tracer = Tracer {
            index: self,
            state: Some((origin, left)),
            frame: None,
            steps: 0,
        };

        let stop_terminal = info.stop.and_then(|stop| {
            let terminal = with_dco(boundary, stop.dco)?;
            (terminal.data_type == Some(DataType::Boolean)).then_some((terminal, stop.stop_if_true))
        });
        let mut states = Vec::with_capacity(names.len());
        for (frame, name) in names.iter().enumerate() {
            tracer.frame = Some((case, frame));
            let written = tracer.values(right_inner);
            let (successors, unmatched) = resolver
                .as_ref()
                .and_then(|resolver| resolver.successors(written, frame, &names))
                .unwrap_or((Successors::Unknown, BTreeSet::new()));
            let stop = match stop_terminal {
                Some((terminal, stop_if_true)) if !self.aborts => {
                    stop_of(tracer.values(terminal), stop_if_true)
                }
                _ => Stop::Maybe,
            };
            states.push(State {
                name: name.clone(),
                successors,
                unmatched,
                stop,
            });
        }
        if selector.kind == SelectorKind::Integer
            && states.iter().all(|s| s.successors == Successors::Unknown)
        {
            return None;
        }

        tracer.frame = None;
        tracer.state = None;
        // An unwired left terminal is an uninitialised register.
        let first = tracer.values(left_outer);
        let initial = resolver.as_ref().and_then(|resolver| {
            let mut initial = BTreeSet::new();
            for atom in first? {
                for target in resolver.resolve(&atom, None)? {
                    let frame = match target {
                        Target::Frame(frame) => frame,
                        Target::Unmatched(_) => selector.default_frame? as usize,
                    };
                    initial.insert(names.get(frame)?.clone());
                }
            }
            Some(initial)
        });
        Some(Machine {
            location: format!(
                "while loop #{}, case structure #{}",
                looping.uid, self.nodes[case].uid
            ),
            states,
            initial,
            default: selector
                .default_frame
                .and_then(|frame| names.get(frame as usize).cloned()),
        })
    }

    /// The nearest while loop around a node, as a node index.
    fn enclosing_while(&self, node: usize) -> Option<usize> {
        let mut diagram = self.node_diagram[node];
        for _ in 0..=self.diagram_parent.len() {
            let (parent, _) = self.diagram_parent[diagram]?;
            if self.nodes[parent].class == "whileLoop" {
                return Some(parent);
            }
            diagram = self.node_diagram[parent];
        }
        None
    }

    /// Whether a node sits anywhere inside a structure.
    fn is_inside(&self, node: usize, structure: usize) -> bool {
        let mut diagram = self.node_diagram[node];
        for _ in 0..=self.diagram_parent.len() {
            match self.diagram_parent[diagram] {
                Some((parent, _)) if parent == structure => return true,
                Some((parent, _)) => diagram = self.node_diagram[parent],
                None => return false,
            }
        }
        false
    }

    /// Recognise a string/queue machine: a string case structure with a
    /// default case whose selector is an output of the library's queue
    /// parser. Only the heuristic check is made; see the module docs.
    fn queue_machine(
        &self,
        case: usize,
        selector: &CaseSelector,
        origin: usize,
    ) -> Option<QueueMachine> {
        let is_call = |node: &Node, prefix: &str| {
            matches!(node.class.as_str(), "iUse" | "polyIUse")
                && node.name.as_deref().is_some_and(|n| n.starts_with(prefix))
        };
        let parser = self.nodes[origin];
        if selector.kind != SelectorKind::String || !is_call(parser, PARSE_QUEUE) {
            return None;
        }
        let default = selector.default_frame? as usize;
        let labels: BTreeSet<&str> = selector
            .frames
            .iter()
            .flatten()
            .filter_map(|entry| match entry {
                SelectorMatch::Text(text) => Some(text.as_str()),
                SelectorMatch::Range { .. } => None,
            })
            .collect();
        let looping = self.enclosing_while(case);
        let mut tracer = Tracer {
            index: self,
            state: None,
            frame: None,
            steps: 0,
        };
        let mut texts: BTreeSet<String> = BTreeSet::new();
        let mut collect = |tracer: &mut Tracer<'_, 'a>, sink: &Terminal| {
            for atom in tracer.values(sink).into_iter().flatten() {
                if let Atom::Constant(Value::String(text), _) = atom {
                    texts.insert(text);
                }
            }
        };
        let is_string_input = |t: &&Terminal| {
            t.data_type == Some(DataType::String) && self.source.contains_key(&t.uid)
        };
        // Constants handed to the library call that extends the queue.
        for (number, node) in self.nodes.iter().enumerate() {
            let in_scope = looping.is_none_or(|looping| self.is_inside(number, looping));
            if in_scope && is_call(node, ADD_TO_QUEUE) {
                for terminal in node.terminals.iter().filter(is_string_input) {
                    collect(&mut tracer, terminal);
                }
            }
        }
        // The queue the register starts with.
        for input in parser.terminals.iter().filter(is_string_input) {
            let Some((node, left)) = self.origin(input) else {
                continue;
            };
            if left.dco_class.as_deref() != Some("lSR") || !self.is_boundary(node) {
                continue;
            }
            let outer = self
                .structure_of(node)
                .zip(left.dco)
                .and_then(|(owner, dco)| with_dco(self.nodes[owner], dco));
            let Some(outer) = outer else {
                continue;
            };
            // The first states are usually added by a call of the same
            // library function outside the loop.
            match self.origin(outer) {
                Some((first, _)) if is_call(self.nodes[first], ADD_TO_QUEUE) => {
                    for terminal in self.nodes[first].terminals.iter().filter(is_string_input) {
                        collect(&mut tracer, terminal);
                    }
                }
                _ => collect(&mut tracer, outer),
            }
        }

        let enqueued: BTreeSet<String> = texts.iter().flat_map(|text| queued_names(text)).collect();
        let unhandled = enqueued
            .iter()
            .filter(|name| {
                name.is_ascii()
                    && !labels
                        .iter()
                        .any(|label| label.to_lowercase() == name.to_lowercase())
            })
            .cloned()
            .collect();
        Some(QueueMachine {
            location: match looping {
                Some(looping) => format!(
                    "while loop #{}, case structure #{}",
                    self.nodes[looping].uid, self.nodes[case].uid
                ),
                None => format!("case structure #{}", self.nodes[case].uid),
            },
            states: labels.iter().map(|label| (*label).to_owned()).collect(),
            default: selector.label(default),
            enqueued: enqueued.into_iter().collect(),
            unhandled,
        })
    }
}

/// State names in a queue string: one per line, ending at `>>`, with
/// `//` and `/* */` comments removed. See the module documentation for
/// where this syntax was established.
pub fn queued_names(text: &str) -> Vec<String> {
    let mut clean = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        clean.push_str(&rest[..start]);
        rest = match rest[start + 2..].find("*/") {
            Some(end) => &rest[start + 2 + end + 2..],
            None => "",
        };
    }
    clean.push_str(rest);
    clean
        .split(['\n', '\r'])
        .filter_map(|line| {
            let name = line.split(">>").next()?.split("//").next()?.trim();
            (!name.is_empty()).then(|| name.to_owned())
        })
        .collect()
}

/// One name per frame, unique within the machine.
fn state_names(selector: &CaseSelector) -> Vec<String> {
    let value = |value: i32| match selector.kind {
        SelectorKind::Enum => usize::try_from(value)
            .ok()
            .and_then(|index| selector.enum_items.get(index))
            .cloned()
            .unwrap_or_else(|| value.to_string()),
        _ => value.to_string(),
    };
    let mut names: Vec<String> = selector
        .frames
        .iter()
        .enumerate()
        .map(|(frame, entries)| {
            let parts: Vec<String> = entries
                .iter()
                .map(|entry| match entry {
                    SelectorMatch::Text(text) if text.is_empty() => "\"\"".to_owned(),
                    SelectorMatch::Text(text) => text.clone(),
                    SelectorMatch::Range { start, end } if start == end && start.is_some() => {
                        start.map(value).unwrap_or_default()
                    }
                    SelectorMatch::Range { start, end } => format!(
                        "{}..{}",
                        start.map(value).unwrap_or_default(),
                        end.map(value).unwrap_or_default()
                    ),
                })
                .collect();
            if !parts.is_empty() {
                parts.join(", ")
            } else if selector.default_frame == Some(frame as u32) {
                "Default".to_owned()
            } else {
                format!("frame {frame}")
            }
        })
        .collect();
    // Two frames with one name would be one state to the model.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for name in &names {
        *seen.entry(name.clone()).or_default() += 1;
    }
    let mut taken: HashSet<String> = names.iter().cloned().collect();
    for (frame, name) in names.iter_mut().enumerate() {
        if seen[name.as_str()] > 1 {
            let mut renamed = format!("{name} (frame {frame})");
            while !taken.insert(renamed.clone()) {
                renamed.push('\'');
            }
            *name = renamed;
        }
    }
    names
}

fn stop_of(values: Values, stop_if_true: bool) -> Stop {
    let Some(values) = values else {
        return Stop::Maybe;
    };
    let (mut on, mut off) = (false, false);
    for atom in values {
        match atom {
            Atom::Constant(Value::Boolean(true), _) => on = true,
            // An unwired Boolean tunnel provides False.
            Atom::Constant(Value::Boolean(false), _) | Atom::TypeDefault => off = true,
            _ => return Stop::Maybe,
        }
    }
    match (on, off) {
        (true, false) if stop_if_true => Stop::Always,
        (false, true) if !stop_if_true => Stop::Always,
        (true, false) | (false, true) => Stop::Never,
        _ => Stop::Maybe,
    }
}

/// What a written value selects.
enum Target {
    Frame(usize),
    /// A string no case is labelled with; the default case runs.
    Unmatched(String),
}

/// Maps written values to frames through the selector table.
struct Resolver<'s> {
    selector: &'s CaseSelector,
    numeric: Option<NumericType>,
}

impl<'s> Resolver<'s> {
    /// `None` when written values cannot be resolved reliably: the types
    /// of the selector and the register differ, or the table is of a kind
    /// whose encoding is not trusted.
    fn new(
        selector: &'s CaseSelector,
        selected_by: &Terminal,
        left: &Terminal,
        right: &Terminal,
    ) -> Option<Self> {
        let ends = [selected_by, left, right];
        let numeric = selected_by.numeric;
        if selector.kind == SelectorKind::String {
            if ends.iter().any(|t| t.data_type != Some(DataType::String)) {
                return None;
            }
        } else {
            let narrow = matches!(
                numeric?,
                NumericType::I8
                    | NumericType::I16
                    | NumericType::I32
                    | NumericType::U8
                    | NumericType::U16
                    | NumericType::U32
                    | NumericType::Enum8
                    | NumericType::Enum16
                    | NumericType::Enum32
            );
            if !narrow || ends.iter().any(|t| t.numeric != numeric) {
                return None;
            }
            // An unsigned table that holds a negative number stores a
            // value above the 32-bit signed range in a way not verified.
            let negative = selector.frames.iter().flatten().any(|entry| match entry {
                SelectorMatch::Range { start, end } => {
                    start.is_some_and(|v| v < 0) || end.is_some_and(|v| v < 0)
                }
                SelectorMatch::Text(_) => false,
            });
            if numeric?.is_unsigned() && negative {
                return None;
            }
        }
        Some(Self { selector, numeric })
    }

    fn number(&self, value: i64) -> Option<Vec<Target>> {
        let value = i32::try_from(value).ok()?;
        let hits: Vec<usize> = (0..self.selector.frames.len())
            .filter(|&frame| {
                self.selector.frames[frame].iter().any(|entry| match entry {
                    SelectorMatch::Range { start, end } => {
                        start.is_none_or(|start| start <= value)
                            && end.is_none_or(|end| value <= end)
                    }
                    SelectorMatch::Text(_) => false,
                })
            })
            .collect();
        match hits[..] {
            [] => Some(vec![Target::Frame(self.selector.default_frame? as usize)]),
            [frame] => Some(vec![Target::Frame(frame)]),
            _ => None,
        }
    }

    fn text(&self, value: &str) -> Option<Vec<Target>> {
        if !value.is_ascii() {
            return None;
        }
        let frames_with = |same: &dyn Fn(&str) -> bool| -> Vec<usize> {
            (0..self.selector.frames.len())
                .filter(|&frame| {
                    self.selector.frames[frame]
                        .iter()
                        .any(|entry| matches!(entry, SelectorMatch::Text(text) if same(text)))
                })
                .collect()
        };
        match frames_with(&|label| label == value)[..] {
            [frame] => return Some(vec![Target::Frame(frame)]),
            [] => {}
            _ => return None,
        }
        let default = self.selector.default_frame? as usize;
        let lower = value.to_lowercase();
        let mut targets: Vec<Target> = frames_with(&|label| label.to_lowercase() == lower)
            .into_iter()
            .map(Target::Frame)
            .collect();
        if !targets.is_empty() || value.is_empty() {
            targets.push(Target::Frame(default));
        } else {
            targets.push(Target::Unmatched(value.to_owned()));
        }
        Some(targets)
    }

    /// The frames a value selects. `current` is the frame of the state
    /// being analysed, for a value that is the register's own.
    fn resolve(&self, atom: &Atom, current: Option<usize>) -> Option<Vec<Target>> {
        match (atom, self.selector.kind) {
            (Atom::Current, _) => Some(vec![Target::Frame(current?)]),
            (Atom::TypeDefault, SelectorKind::String) => self.text(""),
            (Atom::TypeDefault, _) => self.number(0),
            (Atom::Constant(Value::String(text), _), SelectorKind::String) => self.text(text),
            (Atom::Constant(Value::Enum { value, name }, numeric), SelectorKind::Enum) => {
                let item = self.selector.enum_items.get(*value as usize)?;
                (*numeric == self.numeric && name.as_ref() == Some(item))
                    .then(|| self.number(i64::from(*value)))
                    .flatten()
            }
            (Atom::Constant(Value::Integer(value), numeric), SelectorKind::Integer) => (*numeric
                == self.numeric)
                .then(|| self.number(*value))
                .flatten(),
            _ => None,
        }
    }

    fn successors(
        &self,
        written: Values,
        frame: usize,
        names: &[String],
    ) -> Option<(Successors, BTreeSet<String>)> {
        let mut known = BTreeSet::new();
        let mut unmatched = BTreeSet::new();
        for atom in written? {
            for target in self.resolve(&atom, Some(frame))? {
                match target {
                    Target::Frame(frame) => known.insert(names.get(frame)?.clone()),
                    Target::Unmatched(text) => unmatched.insert(text),
                };
            }
        }
        Some((Successors::Known(known), unmatched))
    }
}

/// Follows values backwards along wires. See the module documentation
/// for the steps it understands.
struct Tracer<'i, 'a> {
    index: &'i Index<'a>,
    /// The boundary node and data object of the machine's left register
    /// terminal, whose value is the current state.
    state: Option<(usize, i32)>,
    /// The machine's case structure and the frame being analysed.
    frame: Option<(usize, usize)>,
    steps: usize,
}

impl<'a> Tracer<'_, 'a> {
    /// The values a sink terminal can receive, with a fresh step budget.
    fn values(&mut self, sink: &Terminal) -> Values {
        self.steps = 0;
        self.sink(sink, None, 0)
    }

    /// The values a sink terminal can receive; `unwired` when nothing is
    /// wired to it.
    fn sink(&mut self, sink: &Terminal, unwired: Values, depth: usize) -> Values {
        if !sink.wired && !self.index.source.contains_key(&sink.uid) {
            return unwired;
        }
        let (node, source) = self.index.source_of(sink)?;
        self.source(node, source, depth + 1)
    }

    fn source(&mut self, at: usize, source: &'a Terminal, depth: usize) -> Values {
        self.steps += 1;
        if depth > MAX_DEPTH || self.steps > MAX_STEPS {
            return None;
        }
        let index = self.index;
        let node = index.nodes[at];
        if index.is_boundary(at) {
            return match source.dco_class.as_deref()? {
                "bDConstDCO" => one(Atom::Constant(source.constant.clone()?, source.numeric)),
                "lSR" if self.state == Some((at, source.dco?)) => one(Atom::Current),
                "lSR" => None,
                _ => self.sink(index.outside(at, source)?, None, depth),
            };
        }
        if node.class == "prim" && node.primitive == Some(SELECT) {
            if source.param != Some(0) {
                return None;
            }
            let input = |param: u32| unique(node, |t| t.param == Some(param));
            let (when_false, _, when_true) = (input(1)?, input(2)?, input(3)?);
            let a = self.sink(when_false, None, depth);
            let b = self.sink(when_true, None, depth);
            return union(a, b);
        }
        if node.diagrams.is_empty() {
            return None;
        }
        // The outer side of a structure's output tunnel.
        let tunnel = source.tunnel?;
        if !tunnel.output || tunnel.mode != TunnelMode::Plain || tunnel.conditional {
            return None;
        }
        let inner = index.inside(at, source)?;
        let class = source.dco_class.as_deref()?;
        match (node.class.as_str(), class) {
            ("select" | "eventStruct", "selTun") | ("commentNode", "commentTun") => {
                let frames = match self.frame {
                    Some((case, frame)) if case == at => frame..frame + 1,
                    _ => 0..inner.len(),
                };
                let unwired = match tunnel.default_if_unwired {
                    true => one(Atom::TypeDefault),
                    false => None,
                };
                let mut all = Some(BTreeSet::new());
                for terminal in inner.get(frames)? {
                    let unwired = unwired.clone();
                    all = union(all, self.sink(terminal, unwired, depth));
                    all.as_ref()?;
                }
                // An event structure whose timeout is wired can time out
                // without running any frame when no frame handles the
                // timeout; its tunnels then carry the type's default.
                // Which frame handles which event is not decoded.
                if node.class == "eventStruct" && !index.waits_forever(node) {
                    all = union(all, one(Atom::TypeDefault));
                }
                all.filter(|values| !values.is_empty())
            }
            ("sequence", "seqTun") => {
                // Exactly one frame of a stacked sequence writes the tunnel.
                let writer = {
                    let mut wired = inner
                        .iter()
                        .filter(|t| t.wired || index.source.contains_key(&t.uid));
                    let first = wired.next()?;
                    wired.next().is_none().then_some(*first)?
                };
                self.sink(writer, None, depth)
            }
            ("whileLoop", "lpTun") => match inner[..] {
                [terminal] => self.sink(terminal, None, depth),
                _ => None,
            },
            _ => None,
        }
    }
}
