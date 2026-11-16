//! The `BZxxx` lint rules.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use buzzard_rsrc::{RefKind, ViMetadata};
use rayon::prelude::*;

use crate::dataflow::Finding;
use crate::diagnostic::{Diagnostic, Severity};
use crate::graph::DependencyGraph;
use crate::project::ProjectFile;
use crate::verify::{Confidence, Problem};
use crate::workspace::{Analysis, FileState, Workspace};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub code: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub severity: Severity,
}

pub const RULES: &[Rule] = &[
    Rule {
        code: "BZ000",
        name: "unreadable-file",
        summary:
            "The file could not be read, or is not a valid RSRC container or project XML file.",
        severity: Severity::Error,
    },
    Rule {
        code: "BZ001",
        name: "compiled-code-not-separated",
        summary: "Compiled code is stored inside the source file, which makes every \
                  recompile show up as a change in version control.",
        severity: Severity::Warning,
    },
    Rule {
        code: "BZ002",
        name: "version-mismatch",
        summary: "The file was saved with a different editor version than most of the workspace.",
        severity: Severity::Warning,
    },
    Rule {
        code: "BZ003",
        name: "non-portable-path",
        summary: "A dependency is linked by a machine-specific absolute path, or a path \
                  exceeds the Windows MAX_PATH limit.",
        severity: Severity::Warning,
    },
    Rule {
        code: "BZ004",
        name: "leftover-debug-or-lock",
        summary: "The file still has breakpoints or suspend-when-called set, or its block \
                  diagram is password-protected.",
        severity: Severity::Warning,
    },
    Rule {
        code: "BZ005",
        name: "circular-dependency",
        summary: "The file is part of a circular dependency.",
        severity: Severity::Error,
    },
    Rule {
        code: "BZ006",
        name: "missing-member",
        summary: "A project, library or class lists a member whose file does not exist on disk.",
        severity: Severity::Warning,
    },
    Rule {
        code: "BZ101",
        name: "unwired-error-output",
        summary: "An error output on the block diagram is not wired to anything. With \
                  automatic error handling off, the error is silently discarded.",
        severity: Severity::Warning,
    },
    Rule {
        code: "BZ102",
        name: "reference-never-closed",
        summary: "A reference (queue, file, network connection, ...) is created and never \
                  reaches anything that could close it.",
        severity: Severity::Warning,
    },
    Rule {
        code: "BZ201",
        name: "unreachable-state",
        summary: "A state of a state machine (a case of a case structure selected by a \
                  shift register in a while loop) can never be reached. Proven on a model \
                  that over-approximates the diagram.",
        severity: Severity::Warning,
    },
    Rule {
        code: "BZ202",
        name: "state-machine-cannot-stop",
        summary: "A state machine has a state from which its loop can never stop (warning), \
                  or can never stop at all (info: a loop that runs until the program is \
                  aborted may be intended). Proven on a model that over-approximates the \
                  diagram.",
        severity: Severity::Warning,
    },
    Rule {
        code: "BZ203",
        name: "undefined-state",
        summary: "A state machine writes a next state that no case handles, so the default \
                  case runs. For a string/queue state machine this is a heuristic over the \
                  string constants that are enqueued, not a proof.",
        severity: Severity::Warning,
    },
];

pub fn rule(code: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.code == code)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintConfig {
    /// Longest allowed path, in characters (Windows MAX_PATH is 260).
    pub max_path_length: usize,
    /// Rule codes to skip.
    pub ignore: BTreeSet<String>,
}

impl Default for LintConfig {
    fn default() -> Self {
        Self {
            max_path_length: 260,
            ignore: BTreeSet::new(),
        }
    }
}

/// Run every enabled rule. The result is sorted by path, then code, then
/// message, and is identical across runs.
pub fn lint(
    workspace: &Workspace,
    graph: &DependencyGraph,
    config: &LintConfig,
) -> Vec<Diagnostic> {
    let majority = majority_version(workspace);
    let entries: Vec<(&std::path::PathBuf, &FileState)> = workspace.files().iter().collect();
    let mut out: Vec<Diagnostic> = entries
        .par_iter()
        .flat_map_iter(|(path, state)| {
            let mut found = Vec::new();
            let mut emit = |code: &'static str, message: String| {
                let severity = rule(code).expect("rule is registered").severity;
                found.push(Diagnostic {
                    path: (*path).clone(),
                    code,
                    severity,
                    message,
                    related: Vec::new(),
                });
            };
            match state {
                FileState::Failed(reason) => {
                    emit("BZ000", format!("cannot analyze file: {reason}"))
                }
                FileState::Parsed(analysis) => {
                    let meta = &analysis.metadata;
                    check_separate_code(meta, &mut emit);
                    check_version(meta, majority.as_ref(), &mut emit);
                    check_paths(&workspace.root_relative(path), meta, config, &mut emit);
                    check_debug_and_lock(meta, &mut emit);
                    found.extend(check_dataflow(path, analysis));
                    found.extend(check_state_machines(path, analysis));
                }
                FileState::Project(project) => check_members(workspace, project, &mut emit),
            }
            found
        })
        .collect();

    for cycle in graph.cycles() {
        let mut names: Vec<String> = cycle.example.iter().map(|p| display_name(p)).collect();
        names.push(names[0].clone());
        let chain = names.join(" -> ");
        for member in &cycle.members {
            out.push(Diagnostic {
                path: member.clone(),
                code: "BZ005",
                severity: rule("BZ005").expect("rule is registered").severity,
                message: format!("part of a circular dependency: {chain}"),
                related: cycle
                    .members
                    .iter()
                    .filter(|m| *m != member)
                    .cloned()
                    .collect(),
            });
        }
    }

    out.retain(|d| !config.ignore.contains(d.code));
    out.sort();
    out
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn check_separate_code(meta: &ViMetadata, emit: &mut impl FnMut(&'static str, String)) {
    if meta.flags.as_ref().and_then(|f| f.separate_compiled_code) == Some(false) {
        emit(
            "BZ001",
            "compiled code is not separated from the source file; enable \
             \"Separate compiled code from source file\""
                .to_owned(),
        );
    }
}

/// `(major, minor)` saved by more files than any other version.
struct Majority {
    version: (u8, u8),
    count: usize,
}

fn majority_version(workspace: &Workspace) -> Option<Majority> {
    let mut counts: BTreeMap<(u8, u8), usize> = BTreeMap::new();
    for meta in workspace.files().values().filter_map(FileState::metadata) {
        if let Some(v) = meta.version {
            *counts.entry((v.major, v.minor)).or_default() += 1;
        }
    }
    // On a tie no version is the one the workspace "mostly uses", and
    // picking one would flag half the files against the other half.
    let top = counts.values().copied().max()?;
    let mut leaders = counts.into_iter().filter(|&(_, count)| count == top);
    let (version, count) = leaders.next()?;
    leaders
        .next()
        .is_none()
        .then_some(Majority { version, count })
}

fn check_version(
    meta: &ViMetadata,
    majority: Option<&Majority>,
    emit: &mut impl FnMut(&'static str, String),
) {
    let (Some(version), Some(majority)) = (meta.version, majority) else {
        return;
    };
    if (version.major, version.minor) != majority.version {
        emit(
            "BZ002",
            format!(
                "saved with version {}.{} but the workspace mostly uses {}.{} ({} file{})",
                version.major,
                version.minor,
                majority.version.0,
                majority.version.1,
                majority.count,
                if majority.count == 1 { "" } else { "s" },
            ),
        );
    }
}

/// `own` is the file's path from its scan root. The length of the absolute
/// path would depend on where the workspace happens to be checked out, so
/// the same tree would give different results on different machines.
fn check_paths(
    own: &Path,
    meta: &ViMetadata,
    config: &LintConfig,
    emit: &mut impl FnMut(&'static str, String),
) {
    let limit = config.max_path_length;
    let own_len = own.to_string_lossy().chars().count();
    if own_len > limit {
        emit(
            "BZ003",
            format!(
                "file path is {own_len} characters long from the scan root, \
                 over the limit of {limit}"
            ),
        );
    }

    // The same target is usually linked from several places in one file.
    let targets: BTreeSet<String> = meta.links.iter().map(|l| l.path.to_string()).collect();
    let absolute: BTreeSet<String> = meta
        .links
        .iter()
        .filter(|l| l.path.is_machine_absolute())
        .map(|l| l.path.to_string())
        .collect();
    for target in &targets {
        if absolute.contains(target) {
            emit(
                "BZ003",
                format!("dependency is linked by absolute path `{target}`"),
            );
        }
        let len = target.chars().count();
        if len > limit {
            emit(
                "BZ003",
                format!("dependency path `{target}` is {len} characters long, over the limit of {limit}"),
            );
        }
    }
}

/// Members under a project's `Dependencies` node are skipped: the editor
/// computes that list, so a stale entry there is not something the user
/// listed. URLs that do not resolve to a path (installation paths, items
/// embedded in the XML file) cannot be checked.
fn check_members(
    workspace: &Workspace,
    project: &ProjectFile,
    emit: &mut impl FnMut(&'static str, String),
) {
    let mut missing: BTreeSet<(&str, &str)> = BTreeSet::new();
    for member in project.members.iter().filter(|m| !m.dependency) {
        let Some(path) = &member.path else { continue };
        // A file the workspace has loaded exists; only stat the others.
        if workspace.get(path).is_none() && !path.exists() {
            missing.insert((&member.url, &member.name));
        }
    }
    for (url, name) in missing {
        emit(
            "BZ006",
            format!("member `{name}` does not exist on disk (`{url}`)"),
        );
    }
}

fn check_dataflow(path: &Path, analysis: &Analysis) -> Vec<Diagnostic> {
    // An unwired error output raises a dialog at run time when automatic
    // error handling is on, so the error is at least seen. When it is off
    // the error vanishes, which is the case worth a warning.
    let auto_handled = analysis
        .metadata
        .flags
        .as_ref()
        .and_then(|f| f.auto_error_handling)
        == Some(true);
    analysis
        .dataflow
        .iter()
        .map(|finding| {
            let (code, severity, message) = match finding {
                Finding::UnwiredError { node, places } => {
                    let count = match places {
                        1 => String::new(),
                        n => format!(" in {n} places"),
                    };
                    let (severity, effect) = if auto_handled {
                        (Severity::Info, "it relies on automatic error handling")
                    } else {
                        (
                            Severity::Warning,
                            "automatic error handling is off, so errors are silently discarded",
                        )
                    };
                    (
                        "BZ101",
                        severity,
                        format!("error output of {node} is not wired{count}; {effect}"),
                    )
                }
                Finding::UnclosedReference { node, kind } => (
                    "BZ102",
                    Severity::Warning,
                    format!(
                        "{} reference created by {node} never reaches anything that could close it",
                        reference_name(*kind)
                    ),
                ),
            };
            Diagnostic {
                path: path.to_path_buf(),
                code,
                severity,
                message,
                related: Vec::new(),
            }
        })
        .collect()
}

/// The findings of the state machine verification, computed when the
/// file was loaded. Each message names the machine and says whether the
/// finding is proven or a heuristic.
fn check_state_machines(path: &Path, analysis: &Analysis) -> Vec<Diagnostic> {
    analysis
        .state_machines
        .iter()
        .map(|issue| {
            let code = issue.code();
            // A loop that never stops is often a daemon that is meant to
            // run until the program is aborted.
            let severity = match issue.problem {
                Problem::NeverStops => Severity::Info,
                _ => rule(code).expect("rule is registered").severity,
            };
            let (what, how) = match issue.confidence() {
                Confidence::Proven => ("state machine", "proven"),
                Confidence::Heuristic => ("string/queue state machine", "heuristic, not proven"),
            };
            Diagnostic {
                path: path.to_path_buf(),
                code,
                severity,
                message: format!("{what} at {}: {} ({how})", issue.location, issue.message()),
                related: Vec::new(),
            }
        })
        .collect()
}

fn reference_name(kind: RefKind) -> &'static str {
    match kind {
        RefKind::DataLog => "datalog file",
        RefKind::File => "file",
        RefKind::Tcp => "TCP connection",
        RefKind::Udp => "UDP connection",
        RefKind::Bluetooth => "Bluetooth connection",
        RefKind::Irda => "IrDA connection",
        RefKind::Notifier => "notifier",
        RefKind::Queue => "queue",
        RefKind::UserEvent => "user event",
        RefKind::DataValue => "data value",
        RefKind::Tdms => "TDMS file",
        RefKind::DataSocket => "DataSocket",
        RefKind::Fifo => "FIFO",
        RefKind::Automation => "automation",
        RefKind::DotNet => ".NET",
        _ => "resource",
    }
}

fn check_debug_and_lock(meta: &ViMetadata, emit: &mut impl FnMut(&'static str, String)) {
    if let Some(flags) = &meta.flags {
        if flags.has_breakpoints == Some(true) {
            let message = match flags.breakpoint_count {
                Some(n) if n > 0 => format!(
                    "{n} breakpoint{} left in the block diagram",
                    if n == 1 { "" } else { "s" }
                ),
                _ => "breakpoints left in the block diagram".to_owned(),
            };
            emit("BZ004", message);
        }
        if flags.suspend_when_called == Some(true) {
            emit("BZ004", "\"Suspend when called\" is enabled".to_owned());
        }
    }
    if meta.password_protected == Some(true) {
        emit(
            "BZ004",
            "block diagram is password-protected and cannot be reviewed".to_owned(),
        );
    }
}
