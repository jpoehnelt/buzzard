//! Human-readable Markdown summary of one file, shared by the `inspect`
//! command and editor hovers, and the outline of a block diagram, which
//! is the diagram without its nodes, terminals, and wires.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

use buzzard_rsrc::{
    BlockDiagram, DataType, Diagram, Direction, FileKind, Loss, Node, Reentrancy, ViMetadata,
};
use serde::Serialize;

use crate::archive::ArchiveFile;
use crate::printable;
use crate::project::{ProjectFile, ProjectKind};

fn yes_no(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

/// One row of the metadata of a file: a label and its value, as plain
/// text that a renderer still has to print on one line and escape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataRow {
    /// What the row is about, such as `Saved version`.
    pub label: &'static str,
    /// The value, such as `21.0.0`.
    pub value: String,
    /// A code that goes with the value, such as the file type `LVIN` of
    /// the row `Kind`.
    pub code: Option<String>,
}

impl MetadataRow {
    fn new(label: &'static str, value: impl Into<String>) -> Self {
        Self {
            label,
            value: value.into(),
            code: None,
        }
    }
}

/// The rows of the metadata of a file, in the order the summaries print
/// them. `usage` is `(callers, callees)` inside the workspace, when a
/// dependency graph is available. [`to_markdown`] and the HTML report
/// both print these rows, so the two forms list the same facts.
pub fn metadata_rows(meta: &ViMetadata, usage: Option<(usize, usize)>) -> Vec<MetadataRow> {
    let kind = match meta.kind {
        FileKind::Vi => "VI",
        FileKind::Control => "Control",
        FileKind::Library => "LLB archive",
        FileKind::Other => "Resource file",
    };
    let mut rows = vec![MetadataRow {
        code: Some(meta.file_type.to_string()),
        ..MetadataRow::new("Kind", kind)
    }];
    if let Some(version) = meta.version {
        rows.push(MetadataRow::new("Saved version", version.to_string()));
    }
    if !meta.libraries.is_empty() {
        rows.push(MetadataRow::new("Library", meta.libraries.join(" > ")));
    }
    if let Some(flags) = &meta.flags {
        let reentrancy = match flags.reentrancy {
            Some(Reentrancy::NonReentrant) => "non-reentrant",
            Some(Reentrancy::SharedClones) => "shared clones",
            Some(Reentrancy::PreallocatedClones) => "preallocated clones",
            None => "unknown",
        };
        rows.extend([
            MetadataRow::new(
                "Compiled code separated",
                yes_no(flags.separate_compiled_code),
            ),
            MetadataRow::new(
                "Automatic error handling",
                yes_no(flags.auto_error_handling),
            ),
            MetadataRow::new("Debugging allowed", yes_no(flags.debuggable)),
            MetadataRow::new("Breakpoints", yes_no(flags.has_breakpoints)),
            MetadataRow::new("Suspend when called", yes_no(flags.suspend_when_called)),
            MetadataRow::new("Run when opened", yes_no(flags.run_on_open)),
            MetadataRow::new("Reentrancy", reentrancy),
            MetadataRow::new("Inlined", yes_no(flags.inline)),
            MetadataRow::new("Locked", yes_no(flags.locked)),
        ]);
    }
    if meta.password_protected.is_some() {
        rows.push(MetadataRow::new(
            "Password-protected",
            yes_no(meta.password_protected),
        ));
    }
    if let Some((callers, callees)) = usage {
        rows.push(MetadataRow::new(
            "Callers in workspace",
            callers.to_string(),
        ));
        rows.push(MetadataRow::new(
            "Callees in workspace",
            callees.to_string(),
        ));
    }
    rows
}

/// The targets of the links of a file, sorted and each once. A target is
/// data from the file, so a renderer prints it on one line.
pub fn dependencies(meta: &ViMetadata) -> Vec<String> {
    let mut targets: Vec<String> = meta.links.iter().map(|l| l.path.to_string()).collect();
    targets.sort();
    targets.dedup();
    targets
}

/// Render a summary. `usage` is `(callers, callees)` inside the workspace,
/// when a dependency graph is available. The title is usually a file name
/// and is printed on one line ([`crate::printable::text`]), here and in
/// the other summaries.
pub fn to_markdown(title: &str, meta: &ViMetadata, usage: Option<(usize, usize)>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "### {}\n", printable::text(title));
    let _ = writeln!(out, "| | |\n| --- | --- |");
    for row in metadata_rows(meta, usage) {
        let value = printable::text(&row.value);
        match row.code {
            Some(code) => {
                let _ = writeln!(
                    out,
                    "| {} | {value} (`{}`) |",
                    row.label,
                    printable::text(&code)
                );
            }
            None => {
                let _ = writeln!(out, "| {} | {value} |", row.label);
            }
        }
    }

    let targets = dependencies(meta);
    let _ = writeln!(out, "\n**Dependencies ({})**\n", targets.len());
    for target in &targets {
        // A link is data from the file and stays on its line.
        let _ = writeln!(out, "- `{}`", printable::text(target));
    }
    out
}

/// Graph facts about a project file, when a dependency graph is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectUsage {
    /// Members that are files in the analyzed workspace.
    pub members_in_workspace: usize,
    /// Projects and libraries that list this file as a member.
    pub owners: usize,
    /// Classes in the workspace that inherit from this class.
    pub child_classes: usize,
}

/// Render a summary of a project, library, or class.
pub fn project_to_markdown(
    title: &str,
    project: &ProjectFile,
    usage: Option<ProjectUsage>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "### {}\n", printable::text(title));
    let _ = writeln!(out, "| | |\n| --- | --- |");
    let _ = writeln!(out, "| Kind | {} |", project.kind.label());
    if let Some(version) = &project.lv_version {
        let _ = writeln!(out, "| Saved version | {} |", cell(version));
    }
    let dependencies = project.members.iter().filter(|m| m.dependency).count();
    let _ = writeln!(
        out,
        "| Members | {} |",
        project.members.len() - dependencies
    );
    if dependencies > 0 {
        let _ = writeln!(out, "| Dependencies | {dependencies} |");
    }
    match &project.parent_class {
        Some(parent) => {
            let _ = writeln!(
                out,
                "| Parent class | {} (`{}`) |",
                cell(&parent.qualified_name.join(":")),
                cell(&parent.link.to_string())
            );
        }
        None if project.kind == ProjectKind::Class => {
            let _ = writeln!(out, "| Parent class | none recorded |");
        }
        None => {}
    }
    if let Some(usage) = usage {
        let _ = writeln!(
            out,
            "| Members in workspace | {} |",
            usage.members_in_workspace
        );
        let _ = writeln!(out, "| Listed by | {} |", usage.owners);
        if project.kind == ProjectKind::Class {
            let _ = writeln!(
                out,
                "| Child classes in workspace | {} |",
                usage.child_classes
            );
        }
    }
    out
}

/// Render the directory of an `.llb` archive: one row per member, in the
/// order of the archive.
pub fn archive_to_markdown(title: &str, archive: &ArchiveFile) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "### {}\n", printable::text(title));
    let _ = writeln!(out, "| | |\n| --- | --- |");
    let _ = writeln!(out, "| Kind | LLB archive (`LVAR`) |");
    let _ = writeln!(out, "| Members | {} |", archive.members.len());
    if archive.unlisted > 0 {
        let _ = writeln!(
            out,
            "| Content not in the directory | {} |",
            archive.unlisted
        );
    }
    if !archive.members.is_empty() {
        let _ = writeln!(
            out,
            "\n| Member | Kind | Saved version | Stored | Bytes in archive | Bytes |\n\
             | --- | --- | --- | --- | ---: | ---: |"
        );
    }
    for member in &archive.members {
        let kind = match member.kind {
            FileKind::Vi => "VI",
            FileKind::Control => "Control",
            FileKind::Library => "LLB archive",
            FileKind::Other => "Other",
        };
        let version = member
            .version
            .map_or_else(|| "unknown".to_owned(), |v| v.to_string());
        let _ = writeln!(
            out,
            "| {} | {kind} (`{}`) | {version} | {} | {} | {} |",
            cell(&printable::text(&member.name)),
            cell(&member.file_type.to_string()),
            member.storage.as_str(),
            member.stored_size,
            member.size,
        );
    }
    for problem in &archive.problems {
        // The problem goes outside the table and is already on one line.
        let _ = writeln!(out, "\nNot opened: {problem}.");
    }
    out
}

/// Keep file-supplied text from breaking out of a table cell.
fn cell(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '|' | '`' | '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}

fn type_name(data_type: Option<DataType>) -> String {
    match data_type {
        None => "unknown type".to_owned(),
        Some(DataType::ErrorCluster) => "error cluster".to_owned(),
        Some(DataType::Reference(kind)) => format!("{kind:?} reference").to_lowercase(),
        Some(other) => format!("{other:?}").to_lowercase(),
    }
}

/// Render a block diagram as a nested Markdown outline that reads top to
/// bottom: each node with its inputs and outputs and what they connect to.
///
/// Terminals are identified by `#uid`, the same ids the JSON export uses.
pub fn diagram_to_markdown(diagram: &BlockDiagram) -> String {
    // Terminal uid -> description of the node that owns it.
    let mut owner: HashMap<i32, String> = HashMap::new();
    for node in diagram.nodes() {
        for terminal in &node.terminals {
            owner
                .entry(terminal.uid)
                .or_insert_with(|| format!("{} #{}", describe_node(node), node.uid));
        }
    }
    let mut sinks: HashMap<i32, &[i32]> = HashMap::new();
    let mut source: HashMap<i32, i32> = HashMap::new();
    for wire in diagram.wires() {
        sinks.entry(wire.source).or_insert(&wire.sinks);
        for sink in &wire.sinks {
            source.entry(*sink).or_insert(wire.source);
        }
    }
    let end = |uid: &i32| -> String {
        match owner.get(uid) {
            Some(node) => format!("{node} (terminal #{uid})"),
            None => format!("terminal #{uid}"),
        }
    };

    let mut out = String::new();
    let _ = writeln!(
        out,
        "## Block diagram\n\n{} nodes, {} wires, {} diagrams.\n",
        diagram.nodes().count(),
        diagram.wires().count(),
        diagram.diagrams().len(),
    );
    // An explicit stack, so that deeply nested structures cannot overflow
    // the call stack.
    let mut stack: Vec<(usize, Item<'_>)> = vec![(0, Item::Diagram(&diagram.root, None))];
    while let Some((depth, item)) = stack.pop() {
        let indent = "  ".repeat(depth);
        match item {
            Item::Diagram(inner, frame) => {
                if let Some((frame, label)) = &frame {
                    let label = match label.as_str() {
                        "" => String::new(),
                        label => format!(": {}", cell(label)),
                    };
                    let _ = writeln!(out, "{indent}- frame {frame}{label}");
                }
                let depth = depth + usize::from(frame.is_some());
                for node in inner.nodes.iter().rev() {
                    stack.push((depth, Item::Node(node)));
                }
            }
            Item::Node(node) => {
                let _ = writeln!(out, "{indent}- {} #{}", describe_node(node), node.uid);
                for terminal in &node.terminals {
                    // Unassigned connector-pane slots carry no information.
                    if !terminal.wired && terminal.data_type == Some(DataType::Void) {
                        continue;
                    }
                    let (word, link) = match terminal.direction {
                        Direction::Input => (
                            "in",
                            source.get(&terminal.uid).map(|s| format!(" <- {}", end(s))),
                        ),
                        Direction::Output => (
                            "out",
                            sinks.get(&terminal.uid).map(|all| {
                                let ends: Vec<String> = all.iter().map(&end).collect();
                                format!(" -> {}", ends.join(", "))
                            }),
                        ),
                    };
                    let constant = match &terminal.constant {
                        Some(value) => format!(" constant {}", cell(&value.to_string())),
                        None => String::new(),
                    };
                    let _ = writeln!(
                        out,
                        "{indent}  - {word} #{} {}{constant}{}",
                        terminal.uid,
                        type_name(terminal.data_type),
                        link.unwrap_or_else(|| " (unwired)".to_owned()),
                    );
                }
                for (index, inner) in node.diagrams.iter().enumerate().rev() {
                    let label = node
                        .selector
                        .as_ref()
                        .map(|selector| selector.label(index))
                        .unwrap_or_default();
                    stack.push((depth + 1, Item::Diagram(inner, Some((index, label)))));
                }
            }
        }
    }
    out
}

enum Item<'a> {
    /// A diagram and, for a frame, its index and case label.
    Diagram(&'a Diagram, Option<(usize, String)>),
    Node(&'a Node),
}

fn describe_node(node: &Node) -> String {
    match node.class.as_str() {
        "sRN" => "diagram boundary".to_owned(),
        "whileLoop" => "while loop".to_owned(),
        "forLoop" => "for loop".to_owned(),
        "select" => "case structure".to_owned(),
        "sequence" => "stacked sequence".to_owned(),
        "flatSequence" => "flat sequence".to_owned(),
        "eventStruct" => "event structure".to_owned(),
        _ => crate::dataflow::describe(node),
    }
}

/// A block diagram without its nodes, terminals, and wires: how many
/// there are, which classes of node, which subVIs, and the structures
/// with their frames.
///
/// The JSON of a whole diagram is often over a megabyte. An outline grows
/// with the number of structures and of distinct node classes and
/// callees, not with the number of nodes and wires. A reader looks at the
/// outline first and then asks for one frame ([`frame`]), for a range of
/// node ids ([`nodes_in_range`]), or for everything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagramOutline {
    /// Number of nodes in all diagrams.
    pub nodes: usize,
    /// Number of terminals of all nodes.
    pub terminals: usize,
    /// Number of wires in all diagrams.
    pub wires: usize,
    /// Number of diagrams: the top-level one and every frame.
    pub diagrams: usize,
    /// Whether every diagram decoded without a recorded loss
    /// ([`BlockDiagram::is_complete`]). Only then does the absence of a
    /// node or a wire mean that the file has none.
    pub complete: bool,
    /// The kinds of loss recorded in any diagram, sorted and without
    /// repeats.
    pub losses: Vec<Loss>,
    /// The smallest and the largest node id, for asking for a range of
    /// nodes. `None` for a diagram without nodes.
    pub node_ids: Option<(i32, i32)>,
    /// How many nodes there are of each class, by class name.
    pub node_classes: BTreeMap<String, usize>,
    /// The subVIs that are called, sorted by name and class.
    pub callees: Vec<CalleeOutline>,
    /// The structures (loops, case structures, sequences, and anything
    /// else that holds a diagram), outermost first, depth-first.
    pub structures: Vec<StructureOutline>,
}

/// One subVI and how often the diagram calls it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct CalleeOutline {
    /// The name the call node carries. It is data from the file.
    pub name: String,
    /// The class of the call node: `iUse`, `polyIUse`, or `dynIUse`.
    pub class: String,
    /// Number of call nodes with this name and class.
    pub calls: usize,
}

/// One structure of a block diagram.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StructureOutline {
    /// The id of the structure's node.
    pub uid: i32,
    /// The class of the node, such as `whileLoop` or `select`.
    pub class: String,
    /// The structure in words, such as `case structure`.
    pub description: String,
    /// How deep the structure is nested. A structure on the top-level
    /// diagram has depth 0.
    pub depth: usize,
    /// The id of the structure that holds this one. `None` on the
    /// top-level diagram.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<i32>,
    /// The frames, in order.
    pub frames: Vec<FrameOutline>,
}

/// One frame of a structure: a case, a sequence frame, or the body of a
/// loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FrameOutline {
    /// The position of the frame in its structure, from 0. [`frame`]
    /// takes it.
    pub index: usize,
    /// What selects the frame of a case structure, such as `"Idle"` or
    /// `Default`. Empty for any other structure.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Number of nodes directly in the frame.
    pub nodes: usize,
    /// Number of wires directly in the frame.
    pub wires: usize,
    /// Number of nodes in the frame and in every structure inside it. The
    /// JSON of the frame grows with this number, not with `nodes`.
    pub all_nodes: usize,
}

/// For each diagram under `root`, the number of nodes in it and in every
/// structure inside it. A diagram is named by its address, which is only
/// compared and never read. The work is linear in the size of the
/// diagram, and nesting of any depth cannot overflow the call stack.
fn nodes_below(root: &Diagram) -> HashMap<*const Diagram, usize> {
    // Parents come before their frames in `order`, so the reverse order
    // sees every frame before the diagram that holds it.
    let mut order: Vec<&Diagram> = vec![root];
    let mut next = 0;
    while let Some(diagram) = order.get(next).copied() {
        next += 1;
        order.extend(diagram.nodes.iter().flat_map(|node| node.diagrams.iter()));
    }
    let mut totals: HashMap<*const Diagram, usize> = HashMap::with_capacity(order.len());
    for diagram in order.into_iter().rev() {
        let inside: usize = diagram
            .nodes
            .iter()
            .flat_map(|node| node.diagrams.iter())
            .map(|frame| totals.get(&std::ptr::from_ref(frame)).copied().unwrap_or(0))
            .sum();
        totals.insert(
            std::ptr::from_ref(diagram),
            diagram.nodes.len().saturating_add(inside),
        );
    }
    totals
}

/// The outline of `diagram`. The work is linear in the size of the
/// diagram, and nesting of any depth cannot overflow the call stack.
pub fn outline(diagram: &BlockDiagram) -> DiagramOutline {
    let diagrams = diagram.diagrams();
    let mut losses: Vec<Loss> = diagrams.iter().flat_map(|d| d.losses.clone()).collect();
    losses.sort_unstable();
    losses.dedup();

    let mut node_classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut calls: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    let mut node_ids: Option<(i32, i32)> = None;
    let (mut nodes, mut terminals) = (0_usize, 0_usize);
    for node in diagram.nodes() {
        nodes += 1;
        terminals += node.terminals.len();
        node_ids = Some(match node_ids {
            Some((low, high)) => (low.min(node.uid), high.max(node.uid)),
            None => (node.uid, node.uid),
        });
        match node_classes.get_mut(node.class.as_str()) {
            Some(count) => *count += 1,
            None => {
                node_classes.insert(node.class.clone(), 1);
            }
        }
        if let (Some(name), "iUse" | "polyIUse" | "dynIUse") = (&node.name, node.class.as_str()) {
            *calls.entry((name, &node.class)).or_default() += 1;
        }
    }
    let callees = calls
        .into_iter()
        .map(|((name, class), calls)| CalleeOutline {
            name: name.to_owned(),
            class: class.to_owned(),
            calls,
        })
        .collect();

    let totals = nodes_below(&diagram.root);
    let mut structures = Vec::new();
    // A diagram, how deep its structures are, and the structure it is a
    // frame of.
    let mut stack: Vec<(&Diagram, usize, Option<i32>)> = vec![(&diagram.root, 0, None)];
    while let Some((inner, depth, parent)) = stack.pop() {
        let mut below = Vec::new();
        for node in inner.nodes.iter().filter(|node| !node.diagrams.is_empty()) {
            structures.push(StructureOutline {
                uid: node.uid,
                class: node.class.clone(),
                description: describe_node(node),
                depth,
                parent,
                frames: node
                    .diagrams
                    .iter()
                    .enumerate()
                    .map(|(index, frame)| FrameOutline {
                        index,
                        label: node
                            .selector
                            .as_ref()
                            .map(|selector| selector.label(index))
                            .unwrap_or_default(),
                        nodes: frame.nodes.len(),
                        wires: frame.wires.len(),
                        all_nodes: totals
                            .get(&std::ptr::from_ref(frame))
                            .copied()
                            .unwrap_or(frame.nodes.len()),
                    })
                    .collect(),
            });
            below.extend(
                node.diagrams
                    .iter()
                    .map(|frame| (frame, depth + 1, Some(node.uid))),
            );
        }
        // The stack gives back the last entry first.
        stack.extend(below.into_iter().rev());
    }
    DiagramOutline {
        nodes,
        terminals,
        wires: diagram.wires().count(),
        diagrams: diagrams.len(),
        complete: diagram.is_complete(),
        losses,
        node_ids,
        node_classes,
        callees,
        structures,
    }
}

/// The frame `index` of the structure whose node has the id `structure`,
/// as a [`StructureOutline`] names them. `None` when no structure has
/// that id or the structure has no such frame. When a damaged file gives
/// one id to two structures, the first in the order of the outline is
/// meant.
pub fn frame(diagram: &BlockDiagram, structure: i32, index: usize) -> Option<&Diagram> {
    diagram
        .nodes()
        .find(|node| node.uid == structure && !node.diagrams.is_empty())?
        .diagrams
        .get(index)
}

/// The nodes whose id is from `first` to `last`, both included, in the
/// order of [`BlockDiagram::nodes`]. A structure in the range comes with
/// everything inside it, so a node inside such a structure is not listed a
/// second time on its own: every node of the diagram is in the result at
/// most once, and the result is never larger than the diagram. A node in
/// the range that is inside a structure outside the range is listed. The
/// list is empty when `first` is greater than `last`.
pub fn nodes_in_range(diagram: &BlockDiagram, first: i32, last: i32) -> Vec<&Node> {
    let mut found = Vec::new();
    // The stack gives back the last entry first.
    let mut stack: Vec<&Node> = diagram.root.nodes.iter().rev().collect();
    while let Some(node) = stack.pop() {
        if (first..=last).contains(&node.uid) {
            found.push(node);
        } else {
            stack.extend(
                node.diagrams
                    .iter()
                    .rev()
                    .flat_map(|frame| frame.nodes.iter().rev()),
            );
        }
    }
    found
}

/// A name or label from the file, on one line and inside a code span.
fn code(text: &str) -> String {
    format!("`{}`", printable::text(text).replace('`', "'"))
}

/// The outline of a block diagram as Markdown: what a reader gets in
/// place of the list of every node. The command line and the MCP server
/// print this one text. Every name from the file is on one line.
pub fn outline_to_markdown(outline: &DiagramOutline) -> String {
    let plural = |count: usize| if count == 1 { "" } else { "s" };
    let mut out = String::from("## Block diagram\n\n");
    let _ = writeln!(
        out,
        "{} nodes, {} terminals, {} wires, {} diagrams.",
        outline.nodes, outline.terminals, outline.wires, outline.diagrams
    );
    if let Some((first, last)) = outline.node_ids {
        let _ = writeln!(out, "Node ids {first} to {last}.");
    }
    if !outline.complete {
        let losses: Vec<String> = outline
            .losses
            .iter()
            .map(|loss| match serde_json::to_value(loss) {
                Ok(serde_json::Value::String(name)) => name,
                _ => format!("{loss:?}"),
            })
            .collect();
        let _ = writeln!(
            out,
            "\nThe diagram did not decode completely, so a missing node or wire proves \
             nothing. Losses: {}.",
            losses.join(", ")
        );
    }
    if !outline.node_classes.is_empty() {
        out.push_str("\n### Node classes\n\n");
        for (class, count) in &outline.node_classes {
            let _ = writeln!(out, "- {}: {count}", code(class));
        }
    }
    if !outline.callees.is_empty() {
        out.push_str("\n### Callees\n\n");
        for callee in &outline.callees {
            let _ = writeln!(
                out,
                "- {} ({}): {} call{}",
                code(&callee.name),
                code(&callee.class),
                callee.calls,
                plural(callee.calls)
            );
        }
    }
    if !outline.structures.is_empty() {
        out.push_str("\n### Structures\n\n");
        for structure in &outline.structures {
            // A damaged file may nest deeper than a list can show.
            let indent = "  ".repeat(structure.depth.min(16));
            let _ = writeln!(
                out,
                "{indent}- {} #{} ({}), {} frame{}",
                printable::text(&structure.description),
                structure.uid,
                code(&structure.class),
                structure.frames.len(),
                plural(structure.frames.len())
            );
            for frame in &structure.frames {
                let label = if frame.label.is_empty() {
                    String::new()
                } else {
                    format!(" {}", code(&frame.label))
                };
                // The size of a frame follows the nodes of everything
                // inside it.
                let inside = if frame.all_nodes == frame.nodes {
                    String::new()
                } else {
                    format!(" ({} with the structures inside)", frame.all_nodes)
                };
                let _ = writeln!(
                    out,
                    "{indent}  - frame {}{label}: {} nodes{inside}, {} wires",
                    frame.index, frame.nodes, frame.wires
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_outline_puts_every_name_from_the_file_on_one_line() {
        let outline = DiagramOutline {
            nodes: 3,
            terminals: 4,
            wires: 2,
            diagrams: 2,
            complete: true,
            losses: Vec::new(),
            node_ids: Some((7, 90)),
            node_classes: [("prim".to_owned(), 2), ("a\nb".to_owned(), 1)].into(),
            callees: vec![CalleeOutline {
                name: "Init.vi\n## Forged".to_owned(),
                class: "iUse".to_owned(),
                calls: 1,
            }],
            structures: vec![StructureOutline {
                uid: 12,
                class: "select".to_owned(),
                description: "case structure".to_owned(),
                depth: 1,
                parent: Some(3),
                frames: vec![
                    FrameOutline {
                        index: 0,
                        label: "\"Idle\"\u{1b}[31m`".to_owned(),
                        nodes: 2,
                        wires: 1,
                        all_nodes: 2,
                    },
                    FrameOutline {
                        index: 1,
                        label: String::new(),
                        nodes: 3,
                        wires: 8,
                        all_nodes: 97,
                    },
                ],
            }],
        };
        assert_eq!(
            outline_to_markdown(&outline),
            "## Block diagram\n\n\
             3 nodes, 4 terminals, 2 wires, 2 diagrams.\n\
             Node ids 7 to 90.\n\
             \n### Node classes\n\n\
             - `a\\nb`: 1\n\
             - `prim`: 2\n\
             \n### Callees\n\n\
             - `Init.vi\\n## Forged` (`iUse`): 1 call\n\
             \n### Structures\n\n\
             \x20 - case structure #12 (`select`), 2 frames\n\
             \x20   - frame 0 `\"Idle\"\\u{1b}[31m'`: 2 nodes, 1 wires\n\
             \x20   - frame 1: 3 nodes (97 with the structures inside), 8 wires\n"
        );
    }
}
