//! Builds block diagrams of state machines for the extraction tests, in
//! the shape the decoder gives real ones: a while loop with a shift
//! register, a case structure selected by it, and whatever each frame
//! wires to the next-state and stop tunnels.

use buzzard_rsrc::{
    BlockDiagram, CaseSelector, DataType, Diagram, Direction, LoopInfo, Node, NumericType,
    SelectorKind, SelectorMatch, ShiftRegister, StopTerminal, Terminal, Tunnel, TunnelMode, Value,
    Wire,
};

/// Resource ids of the built-in functions the tests place.
pub const SELECT: u32 = 1516;
pub const ADD: u32 = 1050;
pub const EQUAL: u32 = 1102;
pub const STOP: u32 = 1350;

/// What is wired to a sink.
#[derive(Debug, Clone)]
pub enum Expr {
    /// Nothing.
    Unwired,
    /// A constant of the type the sink expects.
    Const(Value),
    /// A constant of another numeric type.
    Typed(Value, NumericType),
    /// The value of the machine's state register in this iteration.
    Current,
    /// The Select function: value when true, value when false.
    Select(Box<Expr>, Box<Expr>),
    /// An output of a subVI.
    Call,
    /// Another built-in function with these inputs.
    Function(u32, Vec<Expr>),
    /// A front-panel control.
    Control,
    /// A case structure with one expression per frame.
    Case {
        frames: Vec<Expr>,
        default_if_unwired: bool,
    },
    /// An event structure with one expression per frame.
    Event {
        frames: Vec<Expr>,
        default_if_unwired: bool,
        timeout_wired: bool,
    },
    /// A diagram disable structure.
    Disable(Vec<Expr>),
    /// A stacked sequence; at most one frame should be wired.
    Sequence(Vec<Expr>),
    /// A nested while loop passing the value out through a plain tunnel.
    While(Box<Expr>),
    /// A nested for loop passing the value out through a plain tunnel.
    For(Box<Expr>),
    /// A nested for loop collecting the values into an array.
    AutoIndexed(Box<Expr>),
}

pub fn select(when_true: Expr, when_false: Expr) -> Expr {
    Expr::Select(Box::new(when_true), Box::new(when_false))
}

pub fn text(value: &str) -> Expr {
    Expr::Const(Value::String(value.to_owned()))
}

pub fn boolean(value: bool) -> Expr {
    Expr::Const(Value::Boolean(value))
}

pub fn integer(value: i64) -> Expr {
    Expr::Const(Value::Integer(value))
}

/// The type of a wire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ty {
    pub data: DataType,
    pub numeric: Option<NumericType>,
}

pub const BOOLEAN: Ty = Ty {
    data: DataType::Boolean,
    numeric: None,
};
pub const STRING: Ty = Ty {
    data: DataType::String,
    numeric: None,
};
pub const ENUM16: Ty = Ty {
    data: DataType::Numeric,
    numeric: Some(NumericType::Enum16),
};
pub const INT32: Ty = Ty {
    data: DataType::Numeric,
    numeric: Some(NumericType::I32),
};

/// One frame of the machine's case structure.
#[derive(Debug, Clone)]
pub struct Frame {
    pub matches: Vec<SelectorMatch>,
    pub next: Expr,
    pub stop: Expr,
}

/// An event structure around the machine's case structure, which sits in
/// its first frame. The other frames write the next state themselves.
#[derive(Debug, Clone)]
pub struct Wrap {
    /// Next state and stop value of each further event frame.
    pub others: Vec<(Expr, Expr)>,
    pub timeout_wired: bool,
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub kind: SelectorKind,
    pub items: Vec<&'static str>,
    pub state: Ty,
    pub frames: Vec<Frame>,
    pub default: Option<u32>,
    pub initial: Expr,
    pub stop_if_true: bool,
    /// Feed the condition terminal with this instead of the case
    /// structure's stop tunnel.
    pub loop_stop: Option<Expr>,
    /// Feed the register with this instead of the case structure's
    /// next-state tunnel.
    pub loop_next: Option<Expr>,
    pub wrap: Option<Wrap>,
    /// The type definition file the selector's type is an instance of.
    pub type_definition: Option<&'static str>,
}

/// A value of the frame: `one(3)` is the label `3`.
pub fn one(value: i32) -> SelectorMatch {
    SelectorMatch::Range {
        start: Some(value),
        end: Some(value),
    }
}

pub fn range(start: Option<i32>, end: Option<i32>) -> SelectorMatch {
    SelectorMatch::Range { start, end }
}

pub fn label(value: &str) -> SelectorMatch {
    SelectorMatch::Text(value.to_owned())
}

impl Spec {
    /// An enumeration machine with one frame per item, in item order.
    /// Every frame holds its state and never stops until told otherwise.
    pub fn enumeration(items: &[&'static str]) -> Self {
        Self {
            kind: SelectorKind::Enum,
            items: items.to_vec(),
            state: ENUM16,
            frames: (0..items.len() as i32)
                .map(|value| Frame {
                    matches: vec![one(value)],
                    next: Expr::Current,
                    stop: boolean(false),
                })
                .collect(),
            default: None,
            initial: Expr::Unwired,
            stop_if_true: true,
            loop_stop: None,
            loop_next: None,
            wrap: None,
            type_definition: None,
        }
    }

    /// A string machine with one frame per label.
    pub fn strings(labels: &[&'static str]) -> Self {
        Self {
            kind: SelectorKind::String,
            state: STRING,
            items: Vec::new(),
            frames: labels
                .iter()
                .map(|name| Frame {
                    matches: vec![label(name)],
                    next: Expr::Current,
                    stop: boolean(false),
                })
                .collect(),
            ..Self::enumeration(&[])
        }
    }

    /// An integer machine with the given frames.
    pub fn integers(frames: Vec<Vec<SelectorMatch>>) -> Self {
        Self {
            kind: SelectorKind::Integer,
            state: INT32,
            frames: frames
                .into_iter()
                .map(|matches| Frame {
                    matches,
                    next: Expr::Current,
                    stop: boolean(false),
                })
                .collect(),
            ..Self::enumeration(&[])
        }
    }

    /// The enumeration constant of an item.
    pub fn item(&self, name: &str) -> Expr {
        let value = self
            .items
            .iter()
            .position(|item| *item == name)
            .unwrap_or_else(|| panic!("no item {name}"));
        Expr::Const(Value::Enum {
            value: value as u32,
            name: Some(name.to_owned()),
        })
    }

    /// Set what the frame at `index` wires to the next-state tunnel.
    pub fn next(mut self, index: usize, next: Expr) -> Self {
        self.frames[index].next = next;
        self
    }

    /// Set the next state of the frame at `index` to an item constant.
    pub fn goes(self, index: usize, name: &str) -> Self {
        let next = self.item(name);
        self.next(index, next)
    }

    pub fn stop(mut self, index: usize, stop: Expr) -> Self {
        self.frames[index].stop = stop;
        self
    }

    pub fn starts(mut self, initial: Expr) -> Self {
        self.initial = initial;
        self
    }

    pub fn starts_in(self, name: &str) -> Self {
        let initial = self.item(name);
        self.starts(initial)
    }

    pub fn default(mut self, frame: u32) -> Self {
        self.default = Some(frame);
        self
    }

    pub fn build(&self) -> BlockDiagram {
        Build { uid: 0 }.machine(self)
    }
}

struct Build {
    uid: i32,
}

/// Ids fixed by the builder so that tests can find the parts.
pub const LOOP: i32 = 9_000;
pub const CASE: i32 = 9_001;
pub const LEFT: i32 = 9_002;
pub const RIGHT: i32 = 9_003;
pub const CONDITION: i32 = 9_004;

fn plain(output: bool, default_if_unwired: bool) -> Tunnel {
    Tunnel {
        output,
        mode: TunnelMode::Plain,
        conditional: false,
        default_if_unwired,
    }
}

impl Build {
    fn id(&mut self) -> i32 {
        self.uid += 1;
        self.uid
    }

    fn terminal(&mut self, class: &str, ty: Ty, dco: Option<i32>) -> Terminal {
        let uid = self.id();
        Terminal {
            uid,
            data_type: Some(ty.data),
            numeric: ty.numeric,
            dco: Some(dco.unwrap_or(uid + 100_000)),
            dco_class: Some(class.to_owned()),
            ..Terminal::default()
        }
    }

    fn tunnel(&mut self, class: &str, ty: Ty, dco: i32, tunnel: Tunnel) -> Terminal {
        Terminal {
            tunnel: Some(tunnel),
            ..self.terminal(class, ty, Some(dco))
        }
    }

    /// An empty diagram with its boundary node.
    fn diagram(&mut self) -> Diagram {
        let uid = self.id();
        let boundary = Node {
            uid: self.id(),
            class: "sRN".to_owned(),
            ..Node::default()
        };
        Diagram {
            uid,
            nodes: vec![boundary],
            ..Diagram::default()
        }
    }

    fn wire(&mut self, diagram: &mut Diagram, source: i32, sink: i32) {
        let uid = self.id();
        diagram.wires.push(Wire {
            uid,
            source,
            sinks: vec![sink],
        });
    }

    fn function(&mut self, id: u32, ty: Ty, inputs: usize) -> Node {
        let mut terminals = vec![Terminal {
            param: Some(0),
            direction: Direction::Output,
            ..self.terminal("parm", ty, None)
        }];
        for index in 1..=inputs {
            terminals.push(Terminal {
                param: Some(index as u32),
                ..self.terminal("parm", ty, None)
            });
        }
        Node {
            uid: self.id(),
            class: "prim".to_owned(),
            primitive: Some(id),
            terminals,
            ..Node::default()
        }
    }

    /// Place whatever produces `expr` in `diagram` and wire it to `sink`.
    /// `current` is the terminal in this diagram that carries the state.
    fn feed(
        &mut self,
        diagram: &mut Diagram,
        expr: &Expr,
        sink: i32,
        ty: Ty,
        current: Option<i32>,
    ) {
        match expr {
            Expr::Unwired => {}
            Expr::Const(value) | Expr::Typed(value, _) => {
                let ty = match expr {
                    Expr::Typed(_, numeric) => Ty {
                        numeric: Some(*numeric),
                        ..ty
                    },
                    _ => ty,
                };
                let constant = Terminal {
                    constant: Some(value.clone()),
                    direction: Direction::Output,
                    ..self.terminal("bDConstDCO", ty, None)
                };
                let source = constant.uid;
                diagram.nodes[0].terminals.push(constant);
                self.wire(diagram, source, sink);
            }
            Expr::Current => {
                let source = current.expect("the state is not available here");
                self.wire(diagram, source, sink);
            }
            Expr::Control => {
                let control = Terminal {
                    direction: Direction::Output,
                    ..self.terminal("fPDCO", ty, None)
                };
                let source = control.uid;
                diagram.nodes[0].terminals.push(control);
                self.wire(diagram, source, sink);
            }
            Expr::Call => {
                let output = Terminal {
                    param: Some(0),
                    direction: Direction::Output,
                    ..self.terminal("iUseDCO", ty, None)
                };
                let source = output.uid;
                diagram.nodes.push(Node {
                    uid: self.id(),
                    class: "iUse".to_owned(),
                    name: Some("Decide.vi".to_owned()),
                    terminals: vec![output],
                    ..Node::default()
                });
                self.wire(diagram, source, sink);
            }
            Expr::Select(when_true, when_false) => {
                let mut node = self.function(SELECT, ty, 3);
                node.terminals[2].data_type = Some(DataType::Boolean);
                node.terminals[2].numeric = None;
                // Listed in an order other than the parameter order, as a
                // reminder that the order of terminals means nothing.
                node.terminals.swap(1, 3);
                let by = |param: u32| {
                    let found = node.terminals.iter().find(|t| t.param == Some(param));
                    found.unwrap().uid
                };
                let (output, on, off) = (by(0), by(3), by(1));
                diagram.nodes.push(node);
                self.wire(diagram, output, sink);
                self.feed(diagram, when_true, on, ty, current);
                self.feed(diagram, when_false, off, ty, current);
            }
            Expr::Function(id, inputs) => {
                let node = self.function(*id, ty, inputs.len());
                let uids: Vec<i32> = node.terminals.iter().map(|t| t.uid).collect();
                diagram.nodes.push(node);
                self.wire(diagram, uids[0], sink);
                for (input, uid) in inputs.iter().zip(&uids[1..]) {
                    self.feed(diagram, input, *uid, ty, current);
                }
            }
            Expr::Case {
                frames,
                default_if_unwired,
            } => {
                let tunnel = plain(true, *default_if_unwired);
                self.structure(
                    diagram, "select", "selTun", frames, tunnel, sink, ty, current,
                );
            }
            Expr::Event {
                frames,
                default_if_unwired,
                timeout_wired,
            } => {
                let tunnel = plain(true, *default_if_unwired);
                let node = self.structure(
                    diagram,
                    "eventStruct",
                    "selTun",
                    frames,
                    tunnel,
                    sink,
                    ty,
                    current,
                );
                let timeout = self.terminal("eventTimeOut", INT32, None);
                let uid = timeout.uid;
                diagram.nodes[node].terminals.push(timeout);
                if *timeout_wired {
                    self.feed(diagram, &integer(100), uid, INT32, None);
                }
            }
            Expr::Disable(frames) => {
                let tunnel = plain(true, false);
                self.structure(
                    diagram,
                    "commentNode",
                    "commentTun",
                    frames,
                    tunnel,
                    sink,
                    ty,
                    current,
                );
            }
            Expr::Sequence(frames) => {
                let tunnel = plain(true, false);
                self.structure(
                    diagram, "sequence", "seqTun", frames, tunnel, sink, ty, current,
                );
            }
            Expr::While(inner) | Expr::For(inner) | Expr::AutoIndexed(inner) => {
                let class = match expr {
                    Expr::While(_) => "whileLoop",
                    _ => "forLoop",
                };
                let mut tunnel = plain(true, false);
                if matches!(expr, Expr::AutoIndexed(_)) {
                    tunnel.mode = TunnelMode::AutoIndex;
                }
                let frames = [(**inner).clone()];
                let node =
                    self.structure(diagram, class, "lpTun", &frames, tunnel, sink, ty, current);
                diagram.nodes[node].loop_info = Some(LoopInfo::default());
            }
        }
    }

    /// A structure whose output tunnel feeds `sink`, with one frame per
    /// expression. The state is passed in through an input tunnel.
    /// Returns the position of the structure in `diagram.nodes`.
    #[allow(clippy::too_many_arguments)]
    fn structure(
        &mut self,
        diagram: &mut Diagram,
        class: &str,
        tunnel_class: &str,
        frames: &[Expr],
        tunnel: Tunnel,
        sink: i32,
        ty: Ty,
        current: Option<i32>,
    ) -> usize {
        let out = self.id();
        let outer = Terminal {
            direction: Direction::Output,
            ..self.tunnel(tunnel_class, ty, out, tunnel)
        };
        let source = outer.uid;
        let mut node = Node {
            uid: self.id(),
            class: class.to_owned(),
            terminals: vec![outer],
            // A case structure has a selector table: here a Boolean or,
            // for another number of frames, an integer one.
            selector: (class == "select").then(|| CaseSelector {
                kind: match frames.len() {
                    2 => SelectorKind::Boolean,
                    _ => SelectorKind::Integer,
                },
                enum_items: [].into(),
                type_definition: None,
                frames: (0..frames.len() as i32).map(|v| vec![one(v)]).collect(),
                default_frame: (frames.len() != 2).then_some(0),
            }),
            ..Node::default()
        };
        // The state enters through an input tunnel of its own.
        let state_in = current.map(|current| {
            let dco = self.id();
            let outer = self.tunnel(tunnel_class, ty, dco, plain(false, false));
            self.wire(diagram, current, outer.uid);
            node.terminals.push(outer);
            dco
        });
        for expr in frames {
            let mut frame = self.diagram();
            let inner = self.tunnel(tunnel_class, ty, out, tunnel);
            let inner_uid = inner.uid;
            frame.nodes[0].terminals.push(inner);
            let inner_current = state_in.map(|dco| {
                let inner = Terminal {
                    direction: Direction::Output,
                    ..self.tunnel(tunnel_class, ty, dco, plain(false, false))
                };
                let uid = inner.uid;
                frame.nodes[0].terminals.push(inner);
                uid
            });
            self.feed(&mut frame, expr, inner_uid, ty, inner_current);
            node.diagrams.push(frame);
        }
        diagram.nodes.push(node);
        self.wire(diagram, source, sink);
        diagram.nodes.len() - 1
    }

    fn machine(&mut self, spec: &Spec) -> BlockDiagram {
        let state = spec.state;
        let mut root = self.diagram();
        let mut inside = self.diagram();

        // The loop's outer terminals and the inner side on its boundary.
        let left_outer = self.terminal("lSR", state, Some(LEFT));
        let right_outer = Terminal {
            direction: Direction::Output,
            ..self.terminal("rSR", state, Some(RIGHT))
        };
        let left_inner = Terminal {
            direction: Direction::Output,
            ..self.terminal("lSR", state, Some(LEFT))
        };
        let right_inner = self.terminal("rSR", state, Some(RIGHT));
        let condition = self.terminal("lTst", BOOLEAN, Some(CONDITION));
        let (left_in, right_in, stop_in) = (left_inner.uid, right_inner.uid, condition.uid);
        let initial_sink = left_outer.uid;
        inside.nodes[0]
            .terminals
            .extend([left_inner, right_inner, condition]);

        // The case structure, directly in the loop or in the first frame
        // of an event structure.
        match &spec.wrap {
            None => {
                let (case, next, stop) = self.case(spec, &mut inside, left_in);
                inside.nodes.push(case);
                if spec.loop_next.is_none() {
                    self.wire(&mut inside, next, right_in);
                }
                if spec.loop_stop.is_none() {
                    self.wire(&mut inside, stop, stop_in);
                }
            }
            Some(wrap) => {
                let (next_dco, stop_dco, state_dco) = (self.id(), self.id(), self.id());
                let next_tunnel = plain(true, false);
                let stop_tunnel = plain(true, true);
                let next_outer = Terminal {
                    direction: Direction::Output,
                    ..self.tunnel("selTun", state, next_dco, next_tunnel)
                };
                let stop_outer = Terminal {
                    direction: Direction::Output,
                    ..self.tunnel("selTun", BOOLEAN, stop_dco, stop_tunnel)
                };
                let state_outer = self.tunnel("selTun", state, state_dco, plain(false, false));
                let timeout = self.terminal("eventTimeOut", INT32, None);
                let (next, stop, timeout_uid) = (next_outer.uid, stop_outer.uid, timeout.uid);
                self.wire(&mut inside, left_in, state_outer.uid);
                let mut event = Node {
                    uid: self.id(),
                    class: "eventStruct".to_owned(),
                    terminals: vec![next_outer, stop_outer, state_outer, timeout],
                    ..Node::default()
                };
                for index in 0..=wrap.others.len() {
                    let mut frame = self.diagram();
                    let next_inner = self.tunnel("selTun", state, next_dco, next_tunnel);
                    let stop_inner = self.tunnel("selTun", BOOLEAN, stop_dco, stop_tunnel);
                    let state_inner = Terminal {
                        direction: Direction::Output,
                        ..self.tunnel("selTun", state, state_dco, plain(false, false))
                    };
                    let (next_sink, stop_sink, current) =
                        (next_inner.uid, stop_inner.uid, state_inner.uid);
                    frame.nodes[0]
                        .terminals
                        .extend([next_inner, stop_inner, state_inner]);
                    if index == 0 {
                        let (case, next, stop) = self.case(spec, &mut frame, current);
                        frame.nodes.push(case);
                        self.wire(&mut frame, next, next_sink);
                        self.wire(&mut frame, stop, stop_sink);
                    } else {
                        let (next, stop) = &wrap.others[index - 1];
                        self.feed(&mut frame, next, next_sink, state, Some(current));
                        self.feed(&mut frame, stop, stop_sink, BOOLEAN, Some(current));
                    }
                    event.diagrams.push(frame);
                }
                inside.nodes.push(event);
                if wrap.timeout_wired {
                    self.feed(&mut inside, &integer(100), timeout_uid, INT32, None);
                }
                self.wire(&mut inside, next, right_in);
                self.wire(&mut inside, stop, stop_in);
            }
        }
        if let Some(next) = &spec.loop_next {
            self.feed(&mut inside, next, right_in, state, Some(left_in));
        }
        if let Some(stop) = &spec.loop_stop {
            self.feed(&mut inside, stop, stop_in, BOOLEAN, Some(left_in));
        }

        root.nodes.push(Node {
            uid: LOOP,
            class: "whileLoop".to_owned(),
            terminals: vec![left_outer, right_outer],
            diagrams: vec![inside],
            loop_info: Some(LoopInfo {
                stop: Some(StopTerminal {
                    dco: CONDITION,
                    stop_if_true: spec.stop_if_true,
                }),
                shift_registers: vec![ShiftRegister {
                    left: vec![LEFT],
                    right: RIGHT,
                }],
            }),
            ..Node::default()
        });
        self.feed(&mut root, &spec.initial, initial_sink, state, None);
        let mut diagram = BlockDiagram { root };
        mark_wired(&mut diagram);
        diagram
    }

    /// The machine's case structure, selected by `state`. Returns it with
    /// the outer terminals of its next-state and stop tunnels.
    fn case(&mut self, spec: &Spec, diagram: &mut Diagram, state: i32) -> (Node, i32, i32) {
        let ty = spec.state;
        let unwired = |pick: fn(&Frame) -> &Expr| {
            spec.frames
                .iter()
                .any(|frame| matches!(pick(frame), Expr::Unwired))
        };
        let next_tunnel = plain(true, unwired(|frame| &frame.next));
        let stop_tunnel = plain(true, unwired(|frame| &frame.stop));
        let (next_dco, stop_dco, state_dco) = (self.id(), self.id(), self.id());
        let selected_by = self.terminal("caseSel", ty, None);
        let state_outer = self.tunnel("selTun", ty, state_dco, plain(false, false));
        let next_outer = Terminal {
            direction: Direction::Output,
            ..self.tunnel("selTun", ty, next_dco, next_tunnel)
        };
        let stop_outer = Terminal {
            direction: Direction::Output,
            ..self.tunnel("selTun", BOOLEAN, stop_dco, stop_tunnel)
        };
        let (next, stop) = (next_outer.uid, stop_outer.uid);
        self.wire(diagram, state, selected_by.uid);
        self.wire(diagram, state, state_outer.uid);
        let mut case = Node {
            uid: CASE,
            class: "select".to_owned(),
            terminals: vec![selected_by, state_outer, next_outer, stop_outer],
            selector: Some(CaseSelector {
                kind: spec.kind,
                enum_items: spec.items.iter().map(|item| (*item).to_owned()).collect(),
                type_definition: spec.type_definition.map(str::to_owned),
                frames: spec.frames.iter().map(|f| f.matches.clone()).collect(),
                default_frame: spec.default,
            }),
            ..Node::default()
        };
        for frame in &spec.frames {
            let mut inner = self.diagram();
            let state_inner = Terminal {
                direction: Direction::Output,
                ..self.tunnel("selTun", ty, state_dco, plain(false, false))
            };
            let next_inner = self.tunnel("selTun", ty, next_dco, next_tunnel);
            let stop_inner = self.tunnel("selTun", BOOLEAN, stop_dco, stop_tunnel);
            let (current, next_sink, stop_sink) = (state_inner.uid, next_inner.uid, stop_inner.uid);
            inner.nodes[0]
                .terminals
                .extend([state_inner, next_inner, stop_inner]);
            self.feed(&mut inner, &frame.next, next_sink, ty, Some(current));
            self.feed(&mut inner, &frame.stop, stop_sink, BOOLEAN, Some(current));
            case.diagrams.push(inner);
        }
        (case, next, stop)
    }
}

/// Set `wired` on every terminal that is an end of a wire, as the decoder
/// does.
pub fn mark_wired(diagram: &mut BlockDiagram) {
    let ends: std::collections::HashSet<i32> = diagram
        .wires()
        .flat_map(|wire| std::iter::once(wire.source).chain(wire.sinks.iter().copied()))
        .collect();
    visit(&mut diagram.root, &mut |node| {
        for terminal in &mut node.terminals {
            terminal.wired = ends.contains(&terminal.uid);
        }
    });
}

/// Call `change` on every node of the diagram, outermost first.
pub fn visit(diagram: &mut Diagram, change: &mut dyn FnMut(&mut Node)) {
    for node in &mut diagram.nodes {
        change(node);
        for frame in &mut node.diagrams {
            visit(frame, change);
        }
    }
}

/// Change the node with the given id.
pub fn with_node(diagram: &mut BlockDiagram, uid: i32, change: impl FnOnce(&mut Node)) {
    let mut change = Some(change);
    visit(&mut diagram.root, &mut |node| {
        if node.uid == uid {
            if let Some(change) = change.take() {
                change(node);
            }
        }
    });
    assert!(change.is_none(), "no node #{uid}");
}

/// Change every terminal that satisfies a condition; returns how many.
pub fn with_terminals(
    diagram: &mut BlockDiagram,
    wanted: impl Fn(&Terminal) -> bool,
    change: impl Fn(&mut Terminal),
) -> usize {
    let mut count = 0;
    visit(&mut diagram.root, &mut |node| {
        for terminal in node.terminals.iter_mut().filter(|t| wanted(t)) {
            change(terminal);
            count += 1;
        }
    });
    count
}
