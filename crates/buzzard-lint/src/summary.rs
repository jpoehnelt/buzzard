//! Human-readable Markdown summary of one file, shared by the `inspect`
//! command and editor hovers.

use std::fmt::Write;

use buzzard_rsrc::{FileKind, Reentrancy, ViMetadata};

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

/// Keep file-supplied text from breaking out of a table cell.
fn cell(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '|' | '`' | '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}
