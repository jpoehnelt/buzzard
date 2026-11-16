//! Dataflow checks over a decoded block diagram.
//!
//! These run once per file when it is loaded; only the findings are kept,
//! so the diagram itself does not stay in memory.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use buzzard_rsrc::{BlockDiagram, DataType, Direction, Node, RefKind, Terminal};

/// What a dataflow check found in one file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Finding {
    /// An error cluster output that is wired to nothing. `places` counts
    /// how many nodes with the same description have the problem.
    UnwiredError { node: String, places: usize },
    /// A reference that is created and then never reaches anything that
    /// could close it.
    UnclosedReference { node: String, kind: RefKind },
}

/// Node classes that behave like a call: inputs in, outputs out, no nested
/// diagrams or tunnels.
const CALL_CLASSES: [&str; 4] = ["prim", "iUse", "polyIUse", "dynIUse"];

/// Node classes whose error output is checked by [`Finding::UnwiredError`].
const ERROR_CLASSES: [&str; 9] = [
    "prim",
    "iUse",
    "polyIUse",
    "dynIUse",
    "extFunc",
    "propNode",
    "invokeNode",
    "callByRefNode",
    "printf",
];

/// Reference kinds that hold a resource until explicitly closed.
fn must_be_closed(kind: RefKind) -> bool {
    matches!(
        kind,
        RefKind::DataLog
            | RefKind::File
            | RefKind::Tcp
            | RefKind::Udp
            | RefKind::Bluetooth
            | RefKind::Irda
            | RefKind::Notifier
            | RefKind::Queue
            | RefKind::UserEvent
            | RefKind::DataValue
            | RefKind::Tdms
            | RefKind::DataSocket
            | RefKind::Fifo
            | RefKind::Automation
            | RefKind::DotNet
    )
}

/// Name of the built-in function a node is, when the node has a function id
/// and the vendored table knows it. The name is not unique and, for ids the
/// table marks unverified, may be wrong.
pub fn function_name(node: &Node) -> Option<&'static str> {
    Some(buzzard_rsrc::primitive_name(node.primitive?)?.name)
}

/// Human-readable description of a node for use in messages.
///
/// A built-in function is named when its id is in the vendored table
/// (``built-in function `Add` ``); otherwise it is only `built-in function`.
pub fn describe(node: &Node) -> String {
    let what = match node.class.as_str() {
        "prim" => "built-in function",
        "iUse" => "subVI",
        "polyIUse" => "polymorphic subVI",
        "dynIUse" => "dynamic dispatch subVI",
        "extFunc" => "call library function node",
        "propNode" => "property node",
        "invokeNode" => "invoke node",
        "callByRefNode" => "call by reference node",
        "printf" => "format string function",
        // The remaining names are read off the class names, which follow
        // the function names closely.
        "scanf" => "scan string function",
        "mux" => "bundle",
        "nMux" => "bundle or unbundle by name",
        "aBuild" => "build array",
        "aIndx" => "index array",
        "aDel" => "delete from array",
        "concat" => "concatenate strings",
        "cpdArith" => "compound arithmetic",
        "mergeErrors" => "merge errors",
        other => other,
    };
    match (&node.name, function_name(node)) {
        (Some(name), _) => format!("{what} `{name}`"),
        (None, Some(function)) if node.class == "prim" => format!("{what} `{function}`"),
        (None, _) => what.to_owned(),
    }
}

fn reference_kind(terminal: &Terminal) -> Option<RefKind> {
    match terminal.data_type {
        Some(DataType::Reference(kind)) => Some(kind),
        _ => None,
    }
}

/// Run every dataflow check. The result is sorted.
pub fn analyze(diagram: &BlockDiagram) -> Vec<Finding> {
    let mut findings = unwired_errors(diagram);
    findings.extend(unclosed_references(diagram));
    findings.sort();
    findings
}

/// Error handlers sit at the end of an error chain by design; their own
/// error output is normally left unwired.
fn ends_error_chain(node: &Node) -> bool {
    node.name
        .as_deref()
        .is_some_and(|name| name.to_ascii_lowercase().contains("error handler"))
}

fn unwired_errors(diagram: &BlockDiagram) -> Vec<Finding> {
    let mut places: BTreeMap<String, usize> = BTreeMap::new();
    for node in diagram.nodes() {
        if !ERROR_CLASSES.contains(&node.class.as_str()) || ends_error_chain(node) {
            continue;
        }
        let dropped = node.terminals.iter().any(|t| {
            t.data_type == Some(DataType::ErrorCluster)
                && !t.wired
                && t.direction == Direction::Output
                && t.direction_is_reliable()
        });
        if dropped {
            *places.entry(describe(node)).or_default() += 1;
        }
    }
    places
        .into_iter()
        .map(|(node, places)| Finding::UnwiredError { node, places })
        .collect()
}

fn unclosed_references(diagram: &BlockDiagram) -> Vec<Finding> {
    let nodes: Vec<&Node> = diagram.nodes().collect();
    // Terminal uid -> index of the node that owns it.
    let mut owner: HashMap<i32, usize> = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        for terminal in &node.terminals {
            owner.entry(terminal.uid).or_insert(index);
        }
    }
    let mut sinks: HashMap<i32, &[i32]> = HashMap::new();
    for wire in diagram.wires() {
        sinks.entry(wire.source).or_insert(&wire.sinks);
    }
    let is_call = |node: &Node| CALL_CLASSES.contains(&node.class.as_str());

    let mut findings = Vec::new();
    // Only built-in functions are treated as openers. A subVI that returns
    // a reference is far more often an accessor handing out one that
    // already exists than a wrapper that opens a new one.
    for node in nodes.iter().filter(|n| n.class == "prim") {
        for created in &node.terminals {
            let Some(kind) = reference_kind(created).filter(|k| must_be_closed(*k)) else {
                continue;
            };
            if created.direction != Direction::Output || !created.direction_is_reliable() {
                continue;
            }
            // A node that also takes a reference of this kind passes it
            // through; only a node that produces one from nothing opens it.
            let takes_one = node
                .terminals
                .iter()
                .any(|t| t.direction == Direction::Input && reference_kind(t) == Some(kind));
            if takes_one {
                continue;
            }
            if !reaches_a_close(created, kind, &nodes, &owner, &sinks, &is_call) {
                findings.push(Finding::UnclosedReference {
                    node: describe(node),
                    kind,
                });
            }
        }
    }
    findings.sort();
    findings.dedup();
    findings
}

/// Follow a reference from the terminal that creates it. Returns true as
/// soon as it reaches anything that might close it or carry it out of
/// view: a node that consumes it without passing it on, a front-panel
/// terminal, a tunnel, or any object this analysis does not model. Returns
/// false only when every path ends at an unwired output.
fn reaches_a_close(
    created: &Terminal,
    kind: RefKind,
    nodes: &[&Node],
    owner: &HashMap<i32, usize>,
    sinks: &HashMap<i32, &[i32]>,
    is_call: &impl Fn(&Node) -> bool,
) -> bool {
    let mut queue = VecDeque::from([created.uid]);
    let mut visited: HashSet<i32> = HashSet::from([created.uid]);
    while let Some(output) = queue.pop_front() {
        for &sink in sinks.get(&output).copied().unwrap_or_default() {
            let Some(node) = owner.get(&sink).map(|&i| nodes[i]) else {
                return true;
            };
            if !is_call(node) {
                return true;
            }
            let passed_on: Vec<&Terminal> = node
                .terminals
                .iter()
                .filter(|t| t.direction == Direction::Output && reference_kind(t) == Some(kind))
                .collect();
            if passed_on.is_empty() || passed_on.iter().any(|t| !t.direction_is_reliable()) {
                return true;
            }
            for terminal in passed_on {
                if visited.insert(terminal.uid) {
                    queue.push_back(terminal.uid);
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzzard_rsrc::{Diagram, Wire};

    #[test]
    fn built_in_functions_are_named_from_the_table() {
        let function = |id| Node {
            primitive: id,
            ..node(1, "prim", None, vec![])
        };
        assert_eq!(describe(&function(Some(1050))), "built-in function `Add`");
        assert_eq!(function_name(&function(Some(1050))), Some("Add"));
        // Unverified names are used as well.
        assert!(!buzzard_rsrc::primitive_name(8011).unwrap().verified);
        assert_eq!(
            describe(&function(Some(8011))),
            "built-in function `Close Reference`"
        );
        // An id outside the table, and a function without an id.
        assert_eq!(describe(&function(Some(999_999))), "built-in function");
        assert_eq!(function_name(&function(Some(999_999))), None);
        assert_eq!(describe(&function(None)), "built-in function");
        // The linked name of a call wins, and other classes are not renamed.
        let call = Node {
            primitive: Some(1050),
            ..node(1, "iUse", Some("Sum.vi"), vec![])
        };
        assert_eq!(describe(&call), "subVI `Sum.vi`");
        let other = Node {
            primitive: Some(1050),
            ..node(1, "concat", None, vec![])
        };
        assert_eq!(describe(&other), "concatenate strings");
    }

    #[test]
    fn findings_name_the_function() {
        let add = |uid| Node {
            primitive: Some(1050),
            ..node(uid, "prim", None, vec![terminal(uid + 1, Output, ERROR)])
        };
        let unknown = Node {
            primitive: Some(999_999),
            ..node(30, "prim", None, vec![terminal(31, Output, ERROR)])
        };
        assert_eq!(
            analyze(&diagram(vec![add(10), add(20), unknown], &[])),
            [
                Finding::UnwiredError {
                    node: "built-in function".to_owned(),
                    places: 1
                },
                Finding::UnwiredError {
                    node: "built-in function `Add`".to_owned(),
                    places: 2
                },
            ]
        );
    }

    fn terminal(uid: i32, direction: Direction, data_type: DataType) -> Terminal {
        Terminal {
            uid,
            bounds: None,
            direction,
            wired: false,
            data_type: Some(data_type),
            dco: Some(uid + 1000),
            dco_class: Some("parm".to_owned()),
            ..Terminal::default()
        }
    }

    fn node(uid: i32, class: &str, name: Option<&str>, terminals: Vec<Terminal>) -> Node {
        Node {
            uid,
            class: class.to_owned(),
            name: name.map(str::to_owned),
            primitive: None,
            bounds: None,
            terminals,
            diagrams: Vec::new(),
            ..Node::default()
        }
    }

    /// Build a diagram and mark terminals as wired according to the wires.
    fn diagram(mut nodes: Vec<Node>, wires: &[(i32, &[i32])]) -> BlockDiagram {
        let wired: HashSet<i32> = wires
            .iter()
            .flat_map(|(source, sinks)| std::iter::once(*source).chain(sinks.iter().copied()))
            .collect();
        for terminal in nodes.iter_mut().flat_map(|n| &mut n.terminals) {
            terminal.wired = wired.contains(&terminal.uid);
        }
        let wires = wires
            .iter()
            .enumerate()
            .map(|(i, (source, sinks))| Wire {
                uid: 900 + i as i32,
                source: *source,
                sinks: sinks.to_vec(),
            })
            .collect();
        BlockDiagram {
            root: Diagram {
                uid: 1,
                nodes,
                wires,
            },
        }
    }

    const ERROR: DataType = DataType::ErrorCluster;
    const QUEUE: DataType = DataType::Reference(RefKind::Queue);
    use Direction::{Input, Output};

    #[test]
    fn reports_unwired_error_outputs_grouped_by_node() {
        let call = |uid| {
            node(
                uid,
                "iUse",
                Some("Write.vi"),
                vec![
                    terminal(uid + 1, Input, ERROR),
                    terminal(uid + 2, Output, ERROR),
                ],
            )
        };
        let wired = node(
            30,
            "prim",
            None,
            vec![terminal(31, Output, ERROR), terminal(32, Input, ERROR)],
        );
        let bd = diagram(vec![call(10), call(20), wired], &[(31, &[11])]);
        assert_eq!(
            analyze(&bd),
            [Finding::UnwiredError {
                node: "subVI `Write.vi`".to_owned(),
                places: 2
            }]
        );
    }

    #[test]
    fn error_handlers_and_accessor_subvis_are_not_reported() {
        let handler = node(
            10,
            "iUse",
            Some("Simple Error Handler.vi"),
            vec![terminal(11, Input, ERROR), terminal(12, Output, ERROR)],
        );
        // A subVI that hands out a queue it does not own.
        let accessor = node(
            20,
            "iUse",
            Some("Read Mailbox.vi"),
            vec![terminal(21, Output, QUEUE)],
        );
        assert_eq!(analyze(&diagram(vec![handler, accessor], &[])), []);
    }

    #[test]
    fn ignores_error_terminals_it_cannot_judge() {
        // A tunnel's direction flag is not reliable, and the diagram
        // boundary is not a call.
        let mut tunnel = terminal(11, Output, ERROR);
        tunnel.dco_class = Some("selTun".to_owned());
        let bd = diagram(
            vec![
                node(10, "select", None, vec![tunnel]),
                node(20, "sRN", None, vec![terminal(21, Output, ERROR)]),
                node(30, "iUse", None, vec![terminal(31, Input, ERROR)]),
            ],
            &[],
        );
        assert_eq!(analyze(&bd), []);
    }

    fn obtain(uid: i32) -> Node {
        node(uid, "prim", None, vec![terminal(uid + 1, Output, QUEUE)])
    }
    fn enqueue(uid: i32) -> Node {
        node(
            uid,
            "prim",
            None,
            vec![
                terminal(uid + 1, Input, QUEUE),
                terminal(uid + 2, Output, QUEUE),
            ],
        )
    }
    fn release(uid: i32) -> Node {
        node(uid, "prim", None, vec![terminal(uid + 1, Input, QUEUE)])
    }
    fn leak() -> Vec<Finding> {
        vec![Finding::UnclosedReference {
            node: "built-in function".to_owned(),
            kind: RefKind::Queue,
        }]
    }

    #[test]
    fn reference_that_is_released_is_fine() {
        let bd = diagram(
            vec![obtain(10), enqueue(20), release(30)],
            &[(11, &[21]), (22, &[31])],
        );
        assert_eq!(analyze(&bd), []);
        // Released on a branch of the wire.
        let bd = diagram(
            vec![obtain(10), enqueue(20), release(30)],
            &[(11, &[21, 31])],
        );
        assert_eq!(analyze(&bd), []);
    }

    #[test]
    fn reference_that_dead_ends_is_reported() {
        // Never wired at all.
        assert_eq!(analyze(&diagram(vec![obtain(10)], &[])), leak());
        // Used, but the last node's output goes nowhere.
        let bd = diagram(
            vec![obtain(10), enqueue(20), enqueue(40)],
            &[(11, &[21]), (22, &[41])],
        );
        assert_eq!(analyze(&bd), leak());
    }

    #[test]
    fn reference_that_leaves_the_analysis_is_not_reported() {
        // Into an indicator or connector-pane terminal on the boundary.
        let boundary = node(50, "sRN", None, vec![terminal(51, Input, QUEUE)]);
        assert_eq!(
            analyze(&diagram(vec![obtain(10), boundary], &[(11, &[51])])),
            []
        );
        // Into a structure through a tunnel.
        let mut tunnel = terminal(61, Input, QUEUE);
        tunnel.dco_class = Some("lpTun".to_owned());
        let structure = node(60, "whileLoop", None, vec![tunnel]);
        assert_eq!(
            analyze(&diagram(vec![obtain(10), structure], &[(11, &[61])])),
            []
        );
        // To a terminal no node owns.
        assert_eq!(analyze(&diagram(vec![obtain(10)], &[(11, &[777])])), []);
    }

    #[test]
    fn reference_kinds_that_need_no_close_are_ignored() {
        let vi_ref = DataType::Reference(RefKind::ViServer);
        let open = node(10, "prim", None, vec![terminal(11, Output, vi_ref)]);
        assert_eq!(analyze(&diagram(vec![open], &[])), []);
    }

    #[test]
    fn cycles_in_the_wiring_terminate() {
        // Two pass-through nodes wired into each other.
        let bd = diagram(
            vec![obtain(10), enqueue(20), enqueue(40)],
            &[(11, &[21]), (22, &[41]), (42, &[21])],
        );
        assert_eq!(analyze(&bd), leak());
    }
}
