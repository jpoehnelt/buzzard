//! Human-readable Markdown summary of one file, shared by the `inspect`
//! command and editor hovers.

use std::collections::HashMap;
use std::fmt::Write;

use buzzard_rsrc::{
    BlockDiagram, DataType, Diagram, Direction, FileKind, Node, Reentrancy, ViMetadata,
};

use crate::archive::ArchiveFile;
use crate::project::{ProjectFile, ProjectKind};

fn yes_no(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

/// Render a summary. `usage` is `(callers, callees)` inside the workspace,
/// when a dependency graph is available.
pub fn to_markdown(title: &str, meta: &ViMetadata, usage: Option<(usize, usize)>) -> String {
    let mut out = String::new();
    let kind = match meta.kind {
        FileKind::Vi => "VI",
        FileKind::Control => "Control",
        FileKind::Library => "LLB archive",
        FileKind::Other => "Resource file",
    };
    let _ = writeln!(out, "### {title}\n");
    let _ = writeln!(out, "| | |\n| --- | --- |");
    let _ = writeln!(out, "| Kind | {kind} (`{}`) |", meta.file_type);
    if let Some(version) = meta.version {
        let _ = writeln!(out, "| Saved version | {version} |");
    }
    if !meta.libraries.is_empty() {
        let _ = writeln!(out, "| Library | {} |", meta.libraries.join(" > "));
    }
    if let Some(flags) = &meta.flags {
        let reentrancy = match flags.reentrancy {
            Some(Reentrancy::NonReentrant) => "non-reentrant",
            Some(Reentrancy::SharedClones) => "shared clones",
            Some(Reentrancy::PreallocatedClones) => "preallocated clones",
            None => "unknown",
        };
        let _ = writeln!(
            out,
            "| Compiled code separated | {} |",
            yes_no(flags.separate_compiled_code)
        );
        let _ = writeln!(
            out,
            "| Automatic error handling | {} |",
            yes_no(flags.auto_error_handling)
        );
        let _ = writeln!(out, "| Debugging allowed | {} |", yes_no(flags.debuggable));
        let _ = writeln!(out, "| Breakpoints | {} |", yes_no(flags.has_breakpoints));
        let _ = writeln!(
            out,
            "| Suspend when called | {} |",
            yes_no(flags.suspend_when_called)
        );
        let _ = writeln!(out, "| Run when opened | {} |", yes_no(flags.run_on_open));
        let _ = writeln!(out, "| Reentrancy | {reentrancy} |");
        let _ = writeln!(out, "| Inlined | {} |", yes_no(flags.inline));
        let _ = writeln!(out, "| Locked | {} |", yes_no(flags.locked));
    }
    if meta.password_protected.is_some() {
        let _ = writeln!(
            out,
            "| Password-protected | {} |",
            yes_no(meta.password_protected)
        );
    }
    if let Some((callers, callees)) = usage {
        let _ = writeln!(out, "| Callers in workspace | {callers} |");
        let _ = writeln!(out, "| Callees in workspace | {callees} |");
    }

    let mut targets: Vec<String> = meta.links.iter().map(|l| l.path.to_string()).collect();
    targets.sort();
    targets.dedup();
    let _ = writeln!(out, "\n**Dependencies ({})**\n", targets.len());
    for target in &targets {
        let _ = writeln!(out, "- `{target}`");
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

/// Render a summary of a project, library or class.
pub fn project_to_markdown(
    title: &str,
    project: &ProjectFile,
    usage: Option<ProjectUsage>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "### {title}\n");
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
    let _ = writeln!(out, "### {title}\n");
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
            cell(&crate::archive::printable(&member.name)),
            cell(&member.file_type.to_string()),
            member.storage.as_str(),
            member.stored_size,
            member.size,
        );
    }
    for problem in &archive.problems {
        // Outside the table, and already on one line.
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
    // Explicit stack so that deeply nested structures cannot overflow.
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
