//! Human-readable Markdown summary of one file, shared by the `inspect`
//! command and editor hovers.

use std::fmt::Write;

use buzzard_rsrc::{FileKind, Reentrancy, ViMetadata};

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
