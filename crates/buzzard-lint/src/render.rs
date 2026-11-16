//! Pictures of a block diagram.
//!
//! * [`to_svg`] places every node where the editor drew it, using the
//!   positions stored in the file. Wires are drawn as simple elbows between
//!   their end points, not along their original routes, and built-in
//!   functions are labelled boxes because their icons are not in the file.
//!   The label is the function's name where the vendored table has it.
//! * [`to_dot`] and [`to_mermaid`] emit the graph for tools that do their
//!   own layout (Graphviz, Mermaid).

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;

use buzzard_rsrc::{BlockDiagram, DataType, Diagram, Direction, Node, Terminal};

use crate::dataflow::{describe, function_name};

const STRUCTURES: [(&str, &str); 10] = [
    ("commentNode", "diagram disable structure"),
    ("whileLoop", "while loop"),
    ("forLoop", "for loop"),
    ("select", "case structure"),
    ("sequence", "stacked sequence"),
    ("flatSequence", "flat sequence"),
    ("eventStruct", "event structure"),
    ("decomposeRecomposeStructure", "in place element structure"),
    ("timeLoop", "timed loop"),
    ("condDisableStruct", "disable structure"),
];

/// Short human-readable label for a node.
pub fn label(node: &Node) -> String {
    if node.class == "sRN" {
        return "boundary".to_owned();
    }
    if let Some((_, name)) = STRUCTURES.iter().find(|(class, _)| *class == node.class) {
        return (*name).to_owned();
    }
    match (&node.name, node.primitive) {
        (Some(name), _) => name.clone(),
        (None, Some(id)) => match function_name(node) {
            Some(function) => function.to_owned(),
            None => format!("function {id}"),
        },
        (None, None) => describe(node),
    }
}

fn type_label(data_type: Option<DataType>) -> String {
    match data_type {
        None => "unknown".to_owned(),
        Some(DataType::ErrorCluster) => "error".to_owned(),
        Some(DataType::Reference(kind)) => format!("{kind:?} ref").to_lowercase(),
        Some(other) => format!("{other:?}").to_lowercase(),
    }
}

/// Wire colour, loosely following the editor's conventions.
fn colour(data_type: Option<DataType>) -> &'static str {
    match data_type {
        Some(DataType::Numeric) => "#d9761a",
        Some(DataType::Boolean) => "#2e9e44",
        Some(DataType::String) => "#d94fb0",
        Some(DataType::Path) => "#2aa198",
        Some(DataType::ErrorCluster) => "#a89200",
        Some(DataType::Reference(_)) => "#1f8f8f",
        Some(DataType::Cluster) => "#a0522d",
        Some(DataType::Array) => "#c2410c",
        Some(DataType::Variant) => "#7a5cc2",
        _ => "#6b7280",
    }
}

/// Name of a structure's frame: its number and, for a case structure
/// with a decoded selector, the case label, e.g. `frame 2: "Idle", Default`.
pub fn frame_title(node: &Node, index: usize) -> String {
    let label = node
        .selector
        .as_ref()
        .map(|selector| selector.label(index))
        .unwrap_or_default();
    if label.is_empty() {
        format!("frame {}", index + 1)
    } else {
        format!("frame {}: {}", index + 1, shorten(&label, 60))
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let head: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

type Point = (i32, i32);

/// One drawing surface: the top-level diagram, or a frame of a structure
/// other than its first (frames share one rectangle in the editor).
struct Panel<'a> {
    title: String,
    diagram: &'a Diagram,
    /// Size of the owning structure, for frames.
    size: Option<Point>,
    /// Tunnel positions by data object id, relative to the panel origin.
    tunnels: HashMap<i32, Point>,
}

#[derive(Default)]
struct Canvas {
    body: String,
    min: Point,
    max: Point,
    any: bool,
}

impl Canvas {
    fn cover(&mut self, x: i32, y: i32) {
        if self.any {
            self.min = (self.min.0.min(x), self.min.1.min(y));
            self.max = (self.max.0.max(x), self.max.1.max(y));
        } else {
            self.min = (x, y);
            self.max = (x, y);
            self.any = true;
        }
    }
}

fn is_structure(node: &Node) -> bool {
    !node.diagrams.is_empty()
}

/// Where a wire attaches to a terminal of `node` drawn at `origin`.
fn anchor(node: &Node, terminal: &Terminal, origin: Point) -> Option<Point> {
    let b = terminal.bounds?;
    let (left, right) = (i32::from(b.left), i32::from(b.right));
    let middle = (i32::from(b.top) + i32::from(b.bottom)) / 2;
    if node.class == "sRN" {
        // A front-panel terminal is its own box: leave from the right, enter
        // on the left.
        let x = if terminal.direction == Direction::Output {
            right
        } else {
            left
        };
        return Some((origin.0 + x, origin.1 + middle));
    }
    let base = node.bounds?;
    let x = (left + right) / 2;
    Some((
        origin.0 + i32::from(base.left) + x,
        origin.1 + i32::from(base.top) + middle,
    ))
}

/// Render the diagram as a standalone SVG document.
pub fn to_svg(diagram: &BlockDiagram) -> String {
    let mut queue: Vec<Panel<'_>> = vec![Panel {
        title: String::new(),
        diagram: &diagram.root,
        size: None,
        tunnels: HashMap::new(),
    }];
    let mut panels: Vec<(String, Canvas)> = Vec::new();
    let mut next = 0;
    while next < queue.len() {
        let mut canvas = Canvas::default();
        if let Some((w, h)) = queue[next].size {
            canvas.cover(0, 0);
            canvas.cover(w, h);
            let _ = writeln!(
                canvas.body,
                r##"<rect x="0" y="0" width="{w}" height="{h}" class="structure"/>"##
            );
        }
        // Explicit stack: a structure's first frame is drawn inside it.
        let mut stack: Vec<(&Diagram, Point, HashMap<i32, Point>)> = vec![(
            queue[next].diagram,
            (0, 0),
            std::mem::take(&mut queue[next].tunnels),
        )];
        while let Some((inner, origin, tunnels)) = stack.pop() {
            let mut anchors: HashMap<i32, Point> = HashMap::new();
            for node in &inner.nodes {
                draw_node(node, origin, &tunnels, &mut anchors, &mut canvas);
                if !is_structure(node) {
                    continue;
                }
                let Some(bounds) = node.bounds else { continue };
                let corner = (
                    origin.0 + i32::from(bounds.left),
                    origin.1 + i32::from(bounds.top),
                );
                // Tunnel positions relative to the structure's corner.
                let relative: HashMap<i32, Point> = node
                    .terminals
                    .iter()
                    .filter_map(|t| {
                        let at = anchors.get(&t.uid)?;
                        Some((t.dco?, (at.0 - corner.0, at.1 - corner.1)))
                    })
                    .collect();
                for (index, frame) in node.diagrams.iter().enumerate() {
                    if index == 0 {
                        let absolute = relative
                            .iter()
                            .map(|(dco, at)| (*dco, (corner.0 + at.0, corner.1 + at.1)))
                            .collect();
                        stack.push((frame, corner, absolute));
                    } else {
                        queue.push(Panel {
                            title: format!(
                                "{} #{}, {}",
                                label(node),
                                node.uid,
                                frame_title(node, index).replacen(
                                    &format!("frame {}", index + 1),
                                    &format!("frame {} of {}", index + 1, node.diagrams.len()),
                                    1
                                )
                            ),
                            diagram: frame,
                            size: Some((bounds.width(), bounds.height())),
                            tunnels: relative.clone(),
                        });
                    }
                }
            }
            draw_wires(inner, &anchors, &mut canvas);
        }
        panels.push((std::mem::take(&mut queue[next].title), canvas));
        next += 1;
    }

    // Stack the panels vertically.
    const MARGIN: i32 = 24;
    const HEADER: i32 = 22;
    let mut out = String::new();
    let mut y = MARGIN;
    let mut width = 320;
    let mut placed = String::new();
    for (title, canvas) in &panels {
        let (w, h) = (canvas.max.0 - canvas.min.0, canvas.max.1 - canvas.min.1);
        if !title.is_empty() {
            let _ = writeln!(
                placed,
                r##"<text x="{MARGIN}" y="{}" class="panel">{}</text>"##,
                y + 12,
                escape(title)
            );
            y += HEADER;
        }
        let _ = writeln!(
            placed,
            r##"<g transform="translate({},{})">"##,
            MARGIN - canvas.min.0,
            y - canvas.min.1
        );
        placed.push_str(&canvas.body);
        placed.push_str("</g>\n");
        y += h + MARGIN + 16;
        width = width.max(w + 2 * MARGIN);
    }
    let _ = writeln!(
        out,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {y}" width="{}" height="{}" font-family="DejaVu Sans, Helvetica, Arial, sans-serif">"##,
        width * 3 / 2,
        y * 3 / 2
    );
    out.push_str(
        "<style>\n\
         .bg{fill:#ffffff}\n\
         .node{fill:#f3f4f6;stroke:#374151;stroke-width:1}\n\
         .call{fill:#e0ecff;stroke:#1d4ed8;stroke-width:1}\n\
         .panelterm{fill:#fff7e6;stroke:#92400e;stroke-width:1}\n\
         .structure{fill:none;stroke:#111827;stroke-width:2}\n\
         .tunnel{stroke:#111827;stroke-width:0.6}\n\
         .wire{fill:none;stroke-width:1.6}\n\
         .name{font-size:7px;fill:#111827}\n\
         .panel{font-size:11px;font-weight:bold;fill:#111827}\n\
         </style>\n",
    );
    let _ = writeln!(out, r##"<rect class="bg" width="{width}" height="{y}"/>"##);
    out.push_str(&placed);
    out.push_str("</svg>\n");
    out
}

fn draw_node(
    node: &Node,
    origin: Point,
    tunnels: &HashMap<i32, Point>,
    anchors: &mut HashMap<i32, Point>,
    canvas: &mut Canvas,
) {
    for terminal in &node.terminals {
        let at = anchor(node, terminal, origin)
            // The inner side of a tunnel sits where its outer side is.
            .or_else(|| terminal.dco.and_then(|dco| tunnels.get(&dco).copied()));
        if let Some(at) = at {
            anchors.insert(terminal.uid, at);
            canvas.cover(at.0, at.1);
        }
    }
    if node.class == "sRN" {
        // The boundary itself has no shape; its front-panel terminals do.
        for terminal in &node.terminals {
            let Some(b) = terminal.bounds else { continue };
            let (x, y) = (origin.0 + i32::from(b.left), origin.1 + i32::from(b.top));
            canvas.cover(x, y - 10);
            canvas.cover(x + b.width(), y + b.height());
            let text = type_label(terminal.data_type);
            if terminal.dco_class.as_deref() != Some("fPDCO") {
                // The inner side of a tunnel: a small square on the border.
                let _ = writeln!(
                    canvas.body,
                    r##"<rect x="{x}" y="{y}" width="{}" height="{}" class="tunnel" fill="{}"><title>tunnel #{} ({text})</title></rect>"##,
                    b.width(),
                    b.height(),
                    colour(terminal.data_type),
                    terminal.uid,
                );
                continue;
            }
            let kind = if terminal.direction == Direction::Output {
                "control"
            } else {
                "indicator"
            };
            let _ = writeln!(
                canvas.body,
                r##"<g><title>{kind} terminal #{} ({text})</title><rect x="{x}" y="{y}" width="{}" height="{}" rx="2" class="panelterm" stroke="{}"/><text x="{x}" y="{}" class="name">{}</text></g>"##,
                terminal.uid,
                b.width(),
                b.height(),
                colour(terminal.data_type),
                y - 2,
                escape(&shorten(&format!("{kind}: {text}"), 30)),
            );
        }
        return;
    }
    let Some(b) = node.bounds else { return };
    let (x, y) = (origin.0 + i32::from(b.left), origin.1 + i32::from(b.top));
    canvas.cover(x, y - 10);
    canvas.cover(x + b.width(), y + b.height());
    let name = label(node);
    let class = if is_structure(node) {
        "structure"
    } else if node.name.is_some() {
        "call"
    } else {
        "node"
    };
    let _ = writeln!(
        canvas.body,
        r##"<g><title>{} #{}</title><rect x="{x}" y="{y}" width="{}" height="{}" rx="{}" class="{class}"/><text x="{x}" y="{}" class="name">{}</text></g>"##,
        escape(&describe(node)),
        node.uid,
        b.width(),
        b.height(),
        if is_structure(node) { 0 } else { 3 },
        y - 2,
        escape(&shorten(&name, 34)),
    );
}

fn draw_wires(diagram: &Diagram, anchors: &HashMap<i32, Point>, canvas: &mut Canvas) {
    let types: HashMap<i32, Option<DataType>> = diagram
        .nodes
        .iter()
        .flat_map(|n| &n.terminals)
        .map(|t| (t.uid, t.data_type))
        .collect();
    for wire in &diagram.wires {
        let Some(&(x1, y1)) = anchors.get(&wire.source) else {
            continue;
        };
        let data_type = types.get(&wire.source).copied().flatten().or_else(|| {
            wire.sinks
                .iter()
                .find_map(|s| types.get(s).copied().flatten())
        });
        for sink in &wire.sinks {
            let Some(&(x2, y2)) = anchors.get(sink) else {
                continue;
            };
            let middle = (x1 + x2) / 2;
            let _ = writeln!(
                canvas.body,
                r##"<path class="wire" stroke="{}" d="M{x1},{y1} H{middle} V{y2} H{x2}"><title>wire #{} ({})</title></path>"##,
                colour(data_type),
                wire.uid,
                type_label(data_type),
            );
        }
    }
}

/// Edges between nodes, one per distinct (from, to, type), in stable order.
fn edges(diagram: &BlockDiagram) -> BTreeSet<(i32, i32, String)> {
    let mut owner: HashMap<i32, i32> = HashMap::new();
    let mut types: HashMap<i32, Option<DataType>> = HashMap::new();
    for node in diagram.nodes() {
        for terminal in &node.terminals {
            owner.entry(terminal.uid).or_insert(node.uid);
            types.insert(terminal.uid, terminal.data_type);
        }
    }
    let mut out = BTreeSet::new();
    for wire in diagram.wires() {
        let Some(&from) = owner.get(&wire.source) else {
            continue;
        };
        let data_type = types.get(&wire.source).copied().flatten();
        for sink in &wire.sinks {
            if let Some(&to) = owner.get(sink) {
                out.insert((from, to, type_label(data_type)));
            }
        }
    }
    out
}

/// Walk the diagram as nested groups: a structure and each of its frames
/// open and close a group; every node is visited once, in a stable order.
fn walk<'a>(diagram: &'a BlockDiagram, mut visit: impl FnMut(Step<'a>)) {
    enum Todo<'a> {
        Node(&'a Node),
        Frame(&'a Node, usize),
        Close,
    }
    let mut stack: Vec<Todo<'a>> = diagram.root.nodes.iter().rev().map(Todo::Node).collect();
    while let Some(todo) = stack.pop() {
        match todo {
            Todo::Close => visit(Step::Close),
            Todo::Node(node) if is_structure(node) => {
                visit(Step::OpenStructure(node));
                stack.push(Todo::Close);
                for index in (0..node.diagrams.len()).rev() {
                    stack.push(Todo::Frame(node, index));
                }
            }
            Todo::Node(node) => visit(Step::Node(node)),
            Todo::Frame(node, index) => {
                visit(Step::OpenFrame(node, index));
                stack.push(Todo::Close);
                stack.extend(node.diagrams[index].nodes.iter().rev().map(Todo::Node));
            }
        }
    }
}

enum Step<'a> {
    Node(&'a Node),
    OpenStructure(&'a Node),
    OpenFrame(&'a Node, usize),
    Close,
}

/// Render the diagram as a Graphviz `digraph`. Structures and their frames
/// become nested clusters; a structure's own node stands for its tunnels.
pub fn to_dot(diagram: &BlockDiagram) -> String {
    let quote = |text: &str| text.replace('\\', "\\\\").replace('"', "\\\"");
    let mut out = String::from(
        "digraph diagram {\n  rankdir=LR;\n  node [shape=box, fontname=\"Helvetica\", fontsize=10];\n  edge [fontname=\"Helvetica\", fontsize=9];\n",
    );
    let mut depth = 1;
    walk(diagram, |step| {
        let pad = "  ".repeat(depth);
        match step {
            Step::Node(node) => {
                let _ = writeln!(
                    out,
                    "{pad}n{} [label=\"{} #{}\"];",
                    node.uid,
                    quote(&label(node)),
                    node.uid
                );
            }
            Step::OpenStructure(node) => {
                let _ = writeln!(
                    out,
                    "{pad}subgraph cluster_s{} {{\n{pad}  label=\"{} #{}\";\n{pad}  n{} [label=\"tunnels\", shape=ellipse];",
                    node.uid,
                    quote(&label(node)),
                    node.uid,
                    node.uid
                );
                depth += 1;
            }
            Step::OpenFrame(node, index) => {
                let _ = writeln!(
                    out,
                    "{pad}subgraph cluster_f{}_{index} {{\n{pad}  label=\"{}\";",
                    node.uid,
                    quote(&frame_title(node, index))
                );
                depth += 1;
            }
            Step::Close => {
                depth = depth.saturating_sub(1).max(1);
                let _ = writeln!(out, "{}}}", "  ".repeat(depth));
            }
        }
    });
    for (from, to, kind) in edges(diagram) {
        let _ = writeln!(out, "  n{from} -> n{to} [label=\"{kind}\"];");
    }
    out.push_str("}\n");
    out
}

/// Render the diagram as a Mermaid flowchart, grouped like [`to_dot`].
pub fn to_mermaid(diagram: &BlockDiagram) -> String {
    let clean = |text: &str| text.replace('"', "'").replace(['[', ']', '{', '}'], " ");
    let mut out = String::from("flowchart LR\n");
    let mut depth = 1;
    walk(diagram, |step| {
        let pad = "  ".repeat(depth);
        match step {
            Step::Node(node) => {
                let _ = writeln!(
                    out,
                    "{pad}n{}[\"{} #{}\"]",
                    node.uid,
                    clean(&label(node)),
                    node.uid
                );
            }
            Step::OpenStructure(node) => {
                let _ = writeln!(
                    out,
                    "{pad}subgraph s{}[\"{} #{}\"]\n{pad}  n{}((\"tunnels\"))",
                    node.uid,
                    clean(&label(node)),
                    node.uid,
                    node.uid
                );
                depth += 1;
            }
            Step::OpenFrame(node, index) => {
                let _ = writeln!(
                    out,
                    "{pad}subgraph f{}_{index}[\"{}\"]",
                    node.uid,
                    clean(&frame_title(node, index))
                );
                depth += 1;
            }
            Step::Close => {
                depth = depth.saturating_sub(1).max(1);
                let _ = writeln!(out, "{}end", "  ".repeat(depth));
            }
        }
    });
    for (from, to, kind) in edges(diagram) {
        let _ = writeln!(out, "  n{from} -->|{kind}| n{to}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzzard_rsrc::{Rect, RsrcFile};
    use std::path::Path;

    fn fixture(name: &str) -> BlockDiagram {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../buzzard-rsrc/tests/fixtures/pylavi")
            .join(name);
        let bytes = std::fs::read(path).unwrap();
        BlockDiagram::from_rsrc(&RsrcFile::parse(&bytes).unwrap())
            .unwrap()
            .unwrap()
    }

    #[test]
    fn svg_of_a_real_diagram_has_every_node_and_wire() {
        let svg = to_svg(&fixture("add.vi"));
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 "));
        assert!(svg.trim_end().ends_with("</svg>"));
        // Two controls, one indicator, one function, three wires.
        assert_eq!(svg.matches("<title>control terminal").count(), 2);
        assert_eq!(svg.matches("<title>indicator terminal").count(), 1);
        assert_eq!(svg.matches("class=\"node\"").count(), 1);
        assert!(svg.contains(">Add</text>"), "{svg}");
        assert_eq!(svg.matches("<path class=\"wire\"").count(), 3);
        // The function box is 32 pixels square, where the editor put it.
        assert!(
            svg.contains("width=\"32\" height=\"32\" rx=\"3\" class=\"node\""),
            "{svg}"
        );
        assert_eq!(svg, to_svg(&fixture("add.vi")));
    }

    #[test]
    fn subvi_calls_are_named_in_every_format() {
        let diagram = fixture("relative_link.vi");
        assert!(to_svg(&diagram).contains(">empty.vi</text>"));
        assert!(to_dot(&diagram).contains("[label=\"empty.vi #43\"];"));
        assert!(to_mermaid(&diagram).contains("n43[\"empty.vi #43\"]"));
    }

    #[test]
    fn graph_formats_list_nodes_and_edges() {
        let diagram = fixture("add.vi");
        assert_eq!(
            to_mermaid(&diagram),
            "flowchart LR\n  n7[\"boundary #7\"]\n  n43[\"Add #43\"]\n  \
             n7 -->|numeric| n43\n  n43 -->|numeric| n7\n"
        );
        let dot = to_dot(&diagram);
        assert!(dot.starts_with("digraph diagram {\n"));
        assert!(dot.contains("  n7 -> n43 [label=\"numeric\"];\n"));
        assert!(dot.ends_with("}\n"));
    }

    fn node(uid: i32, class: &str, bounds: Option<Rect>, diagrams: Vec<Diagram>) -> Node {
        Node {
            uid,
            class: class.to_owned(),
            name: Some("a \"quoted\" <name> & [more]".to_owned()),
            primitive: None,
            bounds,
            terminals: Vec::new(),
            diagrams,
            ..Node::default()
        }
    }

    #[test]
    fn structures_nest_and_extra_frames_get_their_own_panel() {
        let rect = |top, left, bottom, right| {
            Some(Rect {
                top,
                left,
                bottom,
                right,
            })
        };
        let frame = |uid, inner| Diagram {
            uid,
            nodes: vec![node(inner, "prim", rect(10, 10, 42, 42), vec![])],
            wires: vec![],
            losses: Vec::new(),
        };
        let case = node(
            5,
            "select",
            rect(100, 100, 300, 400),
            vec![frame(6, 60), frame(7, 70), frame(8, 80)],
        );
        let diagram = BlockDiagram {
            root: Diagram {
                uid: 1,
                nodes: vec![case],
                wires: vec![],
                losses: Vec::new(),
            },
        };

        let svg = to_svg(&diagram);
        assert_eq!(svg.matches("class=\"panel\"").count(), 2);
        assert!(svg.contains(">case structure #5, frame 2 of 3</text>"));
        assert!(svg.contains(">case structure #5, frame 3 of 3</text>"));
        // Text is escaped.
        assert!(svg.contains("a &quot;quoted&quot; &lt;name&gt; &amp; [more]"));
        assert!(!svg.contains("<name>"));

        let mermaid = to_mermaid(&diagram);
        assert_eq!(mermaid.matches("subgraph").count(), 4);
        assert_eq!(
            mermaid.matches("\n    end\n").count() + mermaid.matches("\n  end\n").count(),
            4
        );
        assert!(!mermaid.contains('['.to_string().repeat(2).as_str()));
        let dot = to_dot(&diagram);
        assert_eq!(dot.matches("subgraph cluster_").count(), 4);
        assert_eq!(dot.matches('{').count(), dot.matches('}').count());
        assert!(dot.contains("a \\\"quoted\\\" <name> & [more] #60"));
    }

    fn function(uid: i32, id: u32) -> Node {
        Node {
            name: None,
            primitive: Some(id),
            ..node(uid, "prim", None, vec![])
        }
    }

    #[test]
    fn functions_outside_the_table_keep_their_id() {
        assert_eq!(label(&function(1, 1050)), "Add");
        assert_eq!(label(&function(1, 999_999)), "function 999999");
    }

    #[test]
    fn every_function_name_survives_every_format() {
        let names: Vec<_> = buzzard_rsrc::primitive_names().collect();
        let nodes = names
            .iter()
            .enumerate()
            .map(|(index, (id, _))| {
                let top = index as i16 * 40;
                Node {
                    bounds: Some(Rect {
                        top,
                        left: 0,
                        bottom: top + 32,
                        right: 32,
                    }),
                    ..function(index as i32 + 10, *id)
                }
            })
            .collect();
        let diagram = BlockDiagram {
            root: Diagram {
                uid: 1,
                nodes,
                wires: vec![],
                losses: Vec::new(),
            },
        };
        let svg = to_svg(&diagram);
        let dot = to_dot(&diagram);
        let mermaid = to_mermaid(&diagram);
        let markdown = crate::summary::diagram_to_markdown(&diagram);
        for (index, (_, entry)) in names.iter().enumerate() {
            let (uid, name) = (index + 10, entry.name);
            let escaped = name.replace('&', "&amp;");
            // The box shows a long name cut short; the tooltip has all of it.
            let shown = shorten(name, 34).replace('&', "&amp;");
            assert!(svg.contains(&format!(">{shown}</text>")), "{name}");
            assert!(svg.contains(&format!(
                "<title>built-in function `{escaped}` #{uid}</title>"
            )));
            assert!(dot.contains(&format!("n{uid} [label=\"{name} #{uid}\"];")));
            assert!(mermaid.contains(&format!("n{uid}[\"{name} #{uid}\"]")));
            assert!(markdown.contains(&format!("- built-in function `{name}` #{uid}\n")));
        }
        // Nothing in a name opened a tag, an entity or a second line.
        assert!(!svg.contains("&amp;amp;"));
        assert_eq!(svg.matches("<text ").count(), names.len());
        for piece in svg.split('&').skip(1) {
            assert!(
                ["amp;", "lt;", "gt;", "quot;"]
                    .iter()
                    .any(|entity| piece.starts_with(entity)),
                "bare ampersand before {:?}",
                &piece[..piece.len().min(20)]
            );
        }
        assert_eq!(mermaid.lines().count(), names.len() + 1);
        assert_eq!(
            markdown.lines().filter(|l| l.starts_with("- ")).count(),
            names.len()
        );
    }

    #[test]
    fn extreme_coordinates_and_missing_geometry_do_not_panic() {
        let far = Some(Rect {
            top: i16::MIN,
            left: i16::MIN,
            bottom: i16::MAX,
            right: i16::MAX,
        });
        let inner = Diagram {
            uid: 2,
            nodes: vec![node(3, "prim", None, vec![])],
            wires: vec![],
            losses: Vec::new(),
        };
        let diagram = BlockDiagram {
            root: Diagram {
                uid: 1,
                nodes: vec![
                    node(4, "whileLoop", far, vec![inner.clone(), inner]),
                    node(9, "iUse", None, vec![]),
                ],
                wires: vec![buzzard_rsrc::Wire {
                    uid: 8,
                    source: 1,
                    sinks: vec![2, 3],
                }],
                losses: Vec::new(),
            },
        };
        assert!(to_svg(&diagram).contains("</svg>"));
        assert!(to_dot(&diagram).ends_with("}\n"));
        assert!(to_mermaid(&diagram).starts_with("flowchart LR"));
    }
}
