//! The `BZxxx` lint rules.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use buzzard_rsrc::{RefKind, ViMetadata};
use rayon::prelude::*;

use crate::archive::Archives;
use crate::dataflow::Finding;
use crate::diagnostic::{Diagnostic, Severity};
use crate::graph::DependencyGraph;
use crate::project::{Member, ProjectFile};
use crate::verify::{Confidence, Problem};
use crate::workspace::{Analysis, FileState, Workspace};

/// One lint rule, as listed in [`RULES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    /// Stable identifier, such as `BZ001`, used in output and with
    /// `--ignore`.
    pub code: &'static str,
    /// Short kebab-case name, such as `missing-member`.
    pub name: &'static str,
    /// One or two sentences on what the rule reports.
    pub summary: &'static str,
    /// Default severity of its findings.
    pub severity: Severity,
}

/// Every rule, in code order. SARIF output refers to a rule by its index
/// here.
pub const RULES: &[Rule] = &[
    Rule {
        code: "BZ000",
        name: "unreadable-file",
        summary: "The file could not be read, or is not a valid RSRC container, project XML \
                  file, or LLB archive. A member of an archive that cannot be opened is \
                  reported too.",
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
        summary: "A project, library or class lists a member whose file does not exist on disk \
                  or in its archive.",
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
        summary: "A reference, such as a queue, a file, or a network connection, is created \
                  and never reaches anything that could close it.",
        severity: Severity::Warning,
    },
    Rule {
        code: "BZ201",
        name: "unreachable-state",
        summary: "A state of a state machine (a case of a case structure selected by a \
                  shift register in a while loop) can never be reached. Proven on a model \
                  that over-approximates the diagram as saved in the file.",
        severity: Severity::Warning,
    },
    Rule {
        code: "BZ202",
        name: "state-machine-cannot-stop",
        summary: "A state machine has a state from which the condition terminal of its loop \
                  can never stop the loop (warning), or the terminal can never stop the \
                  loop at all (info, because a loop that runs until the program is aborted \
                  may be intended). Proven on a model that over-approximates the diagram. \
                  When the diagram holds a subVI call or another node that could end the \
                  program some other way, the finding names those nodes and is structural, \
                  not proven.",
        severity: Severity::Warning,
    },
    Rule {
        code: "BZ203",
        name: "undefined-state",
        summary: "A constant that no case handles can reach the state register of a state \
                  machine, so the default case would run for it. This is a fact about the \
                  wiring (structural), not a proof that the path runs. For a string/queue \
                  state machine it is a heuristic over the string constants that are \
                  enqueued.",
        severity: Severity::Warning,
    },
];

/// The rule with this code, if there is one.
pub fn rule(code: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.code == code)
}

/// Settings for [`lint`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintConfig {
    /// Longest allowed path, in characters (Windows `MAX_PATH` is 260).
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

/// Run every enabled rule. The result is sorted by path, code, severity,
/// and message, and is identical across runs.
#[expect(
    clippy::missing_panics_doc,
    reason = "the `expect`s look up rule codes that are all in `RULES`"
)]
pub fn lint(
    workspace: &Workspace,
    graph: &DependencyGraph,
    config: &LintConfig,
) -> Vec<Diagnostic> {
    let majority = majority_version(workspace);
    let archives = Archives::default();
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
                    emit("BZ000", format!("cannot analyze file: {reason}"));
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
                FileState::Project(project) => {
                    check_members(workspace, &archives, project, &mut emit);
                }
                FileState::Archive(archive) => {
                    for problem in &archive.problems {
                        emit("BZ000", format!("cannot open {problem}"));
                    }
                }
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
/// path would depend on where the workspace is checked out, so the same
/// tree would give different results on different machines.
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

/// Members under a project's `Dependencies` node are skipped, because the
/// editor computes that list and the user did not write a stale entry
/// there. URLs that do not resolve to a path (installation paths, items
/// embedded in the XML file) cannot be checked.
fn check_members(
    workspace: &Workspace,
    archives: &Archives,
    project: &ProjectFile,
    emit: &mut impl FnMut(&'static str, String),
) {
    let mut missing: BTreeSet<(&str, &str)> = BTreeSet::new();
    for member in project.members.iter().filter(|m| !m.dependency) {
        if member_is_missing(workspace, archives, member) {
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

/// Whether a member's URL resolves to a path that holds no file, in the
/// loaded workspace or on disk. A URL that leads into an `.llb` archive is
/// missing when the archive on disk does not hold that member. It is not
/// missing when the archive cannot be read, because nothing is known then.
/// A URL that does not resolve to a path is never missing. `BZ006` and the
/// dependency report (`crate::deps`) both decide with this function.
pub(crate) fn member_is_missing(
    workspace: &Workspace,
    archives: &Archives,
    member: &Member,
) -> bool {
    // A file that the workspace has loaded exists. Only the others are
    // looked for on disk.
    member.path.as_ref().is_some_and(|path| {
        workspace.get(path).is_none() && archives.present(path, false) == Some(false)
    })
}

fn check_dataflow(path: &Path, analysis: &Analysis) -> Vec<Diagnostic> {
    // An unwired error output raises a dialog at run time when automatic
    // error handling is on, so the user sees the error. When it is off,
    // the error is discarded, and that case gets a warning.
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
/// file was loaded. Each message names the machine and ends in the
/// confidence of the finding: proven, structural, or heuristic.
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
            let confidence = issue.confidence();
            let what = match confidence {
                Confidence::Proven | Confidence::Structural => "state machine",
                Confidence::Heuristic => "string/queue state machine",
            };
            let how = confidence.suffix();
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
