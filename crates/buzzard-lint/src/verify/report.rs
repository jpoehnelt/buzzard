//! Running the verification over files and printing what it found.
//!
//! Every output is deterministic: files are in path order, machines in
//! diagram order, and states, transitions and findings in the order the
//! model gives them.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Serialize;

use super::{verify, Confidence, Issue, QueueMachine, Report, Stop, Successors, Verification};
use crate::inspector::Inspection;
use crate::project::is_project_file;
use crate::workspace::discover;

/// One finding as it is shown: the issue plus its rule code, how much it
/// claims and why it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShownIssue {
    pub code: &'static str,
    pub confidence: Confidence,
    #[serde(flatten)]
    pub issue: Issue,
    pub message: String,
    pub justification: &'static str,
}

impl From<Issue> for ShownIssue {
    fn from(issue: Issue) -> Self {
        Self {
            code: issue.code(),
            confidence: issue.confidence(),
            message: issue.message(),
            justification: issue.justification(),
            issue,
        }
    }
}

/// The machines of one file. Only files that have any are listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileMachines {
    pub path: PathBuf,
    /// Classic state machines: the model, what was derived from it, and
    /// whether the model is exact.
    pub machines: Vec<Report>,
    /// String/queue machines, which are recognised but not modelled.
    pub queue_machines: Vec<QueueMachine>,
    pub findings: Vec<ShownIssue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Summary {
    /// Files with a block diagram that were analysed.
    pub files: usize,
    pub files_with_machines: usize,
    pub files_without_machines: usize,
    pub machines: usize,
    pub queue_machines: usize,
    pub findings: usize,
    pub proven: usize,
    pub heuristic: usize,
}

/// The result of verifying a set of files.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Verified {
    pub files: Vec<FileMachines>,
    /// Files that could not be read or parsed, with the reason.
    pub unreadable: Vec<(PathBuf, String)>,
    pub summary: Summary,
}

impl Verified {
    /// Collect per-file results. `results` holds every analysed file,
    /// whether it has a machine or not; `None` is a file without a block
    /// diagram.
    pub fn new(results: Vec<(PathBuf, Result<Option<Verification>, String>)>) -> Self {
        let mut out = Self::default();
        let mut results = results;
        results.sort_by(|a, b| a.0.cmp(&b.0));
        for (path, result) in results {
            let verification = match result {
                Ok(Some(verification)) => verification,
                Ok(None) => continue,
                Err(reason) => {
                    out.unreadable.push((path, reason));
                    continue;
                }
            };
            out.summary.files += 1;
            if verification.is_empty() {
                out.summary.files_without_machines += 1;
                continue;
            }
            let findings: Vec<ShownIssue> = verification
                .issues()
                .into_iter()
                .map(ShownIssue::from)
                .collect();
            let proven = findings
                .iter()
                .filter(|f| f.confidence == Confidence::Proven)
                .count();
            out.summary.files_with_machines += 1;
            out.summary.machines += verification.machines.len();
            out.summary.queue_machines += verification.queue_machines.len();
            out.summary.findings += findings.len();
            out.summary.proven += proven;
            out.summary.heuristic += findings.len() - proven;
            out.files.push(FileMachines {
                path,
                machines: verification.machines,
                queue_machines: verification.queue_machines,
                findings,
            });
        }
        out
    }

    /// Verify every binary file under the given files and directories.
    pub fn scan(roots: &[PathBuf]) -> Self {
        let paths: Vec<PathBuf> = discover(roots)
            .into_iter()
            .filter(|path| !is_project_file(path))
            .collect();
        Self::new(
            paths
                .into_par_iter()
                .map(|path| {
                    let result = verify_file(&path);
                    (path, result)
                })
                .collect(),
        )
    }

    pub fn has_findings(&self) -> bool {
        self.summary.findings > 0
    }
}

/// Verify one file. `Ok(None)` when it has no block diagram.
pub fn verify_file(path: &Path) -> Result<Option<Verification>, String> {
    let inspection = Inspection::from_path(path)?;
    Ok(inspection.diagram.as_ref().map(verify))
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn list(names: &[&str]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names
            .iter()
            .map(|name| clean(name))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Keep file-supplied text on one line.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// The plain-text report.
pub fn to_text(verified: &Verified) -> String {
    let mut out = String::new();
    for file in &verified.files {
        let _ = writeln!(out, "{}", file.path.display());
        for report in &file.machines {
            machine_text(&mut out, report);
            issues_text(&mut out, file, &report.machine.location);
        }
        for machine in &file.queue_machines {
            let _ = writeln!(
                out,
                "  string/queue state machine at {} (recognised, not modelled)",
                machine.location
            );
            let _ = writeln!(
                out,
                "    cases: {}, default case {}; {} found in constants",
                machine.states.len(),
                clean(&machine.default),
                plural(machine.enqueued.len(), "enqueued name", "enqueued names"),
            );
            issues_text(&mut out, file, &machine.location);
        }
        out.push('\n');
    }
    for (path, reason) in &verified.unreadable {
        let _ = writeln!(out, "{}: cannot analyze file: {reason}", path.display());
    }
    let s = &verified.summary;
    let _ = writeln!(
        out,
        "Checked {}: {} with a state machine ({}, {}), {} without.",
        plural(s.files, "file", "files"),
        s.files_with_machines,
        plural(s.machines, "state machine", "state machines"),
        plural(
            s.queue_machines,
            "string/queue machine",
            "string/queue machines"
        ),
        s.files_without_machines,
    );
    if !verified.unreadable.is_empty() {
        let _ = writeln!(
            out,
            "{} could not be read.",
            plural(verified.unreadable.len(), "file", "files")
        );
    }
    if s.findings == 0 {
        out.push_str("No findings.\n");
    } else {
        let _ = writeln!(
            out,
            "{}: {} proven, {} heuristic.",
            plural(s.findings, "finding", "findings"),
            s.proven,
            s.heuristic
        );
    }
    out
}

fn machine_text(out: &mut String, report: &Report) {
    let machine = &report.machine;
    let _ = writeln!(out, "  state machine at {}", machine.location);
    let names: Vec<&str> = machine.states.iter().map(|s| s.name.as_str()).collect();
    let _ = writeln!(out, "    states ({}): {}", names.len(), list(&names));
    let initial = match &machine.initial {
        Some(initial) => list(&initial.iter().map(String::as_str).collect::<Vec<_>>()),
        None => "unknown (the state register is not initialised with a constant)".to_owned(),
    };
    let _ = writeln!(out, "    initial state: {initial}");
    let _ = writeln!(
        out,
        "    default state: {}",
        machine.default.as_deref().map_or("none".to_owned(), clean)
    );
    let unknown = machine
        .states
        .iter()
        .filter(|s| s.successors == Successors::Unknown)
        .count();
    let maybe = machine
        .states
        .iter()
        .filter(|s| s.stop == Stop::Maybe)
        .count();
    if report.exact {
        out.push_str(
            "    model: exact (every next state and stop condition is a constant on the diagram)\n",
        );
    } else {
        let _ = writeln!(
            out,
            "    model: over-approximation ({} of {} states with a next state that could not \
             be followed, {} with a stop condition that could not be read{})",
            unknown,
            names.len(),
            maybe,
            if machine.initial.is_none() {
                ", initial state unknown"
            } else {
                ""
            }
        );
    }
    out.push_str("    transitions:\n");
    for state in &machine.states {
        let stop = match state.stop {
            Stop::Always => "stops the loop",
            Stop::Never => "never stops the loop",
            Stop::Maybe => "may stop the loop",
        };
        let targets = match &state.successors {
            Successors::Unknown => "? (any state)".to_owned(),
            Successors::Known(_) => {
                let targets: Vec<&str> = report
                    .transitions
                    .iter()
                    .filter(|(from, _)| *from == state.name)
                    .map(|(_, to)| to.as_str())
                    .collect();
                list(&targets)
            }
        };
        let _ = writeln!(out, "      {} -> {targets}; {stop}", clean(&state.name));
    }
}

fn issues_text(out: &mut String, file: &FileMachines, location: &str) {
    for finding in file
        .findings
        .iter()
        .filter(|f| f.issue.location == location)
    {
        let _ = writeln!(
            out,
            "    {} [{}] {}\n      why: {}",
            finding.confidence.as_str(),
            finding.code,
            clean(&finding.message),
            finding.justification
        );
    }
}

/// Text inside a double-quoted Graphviz string.
fn quote(text: &str) -> String {
    clean(text).replace('\\', "\\\\").replace('"', "\\\"")
}

/// The transition graphs as one Graphviz `digraph`, a cluster per
/// machine. A state that stops the loop has a double border, one proven
/// unreachable is dashed, and a next state that could not be followed is
/// a dashed edge to `?`. String/queue machines are not drawn.
pub fn to_dot(verified: &Verified) -> String {
    let mut out = String::from("digraph state_machines {\n  rankdir=LR;\n  node [shape=box];\n");
    let mut number = 0;
    for file in &verified.files {
        for report in &file.machines {
            let machine = &report.machine;
            let id = |state: &str| {
                let index = machine.states.iter().position(|s| s.name == state);
                format!("m{number}_s{}", index.unwrap_or(usize::MAX))
            };
            let _ = writeln!(
                out,
                "  subgraph cluster_{number} {{\n    label=\"{}: {}\";",
                quote(&file.path.display().to_string()),
                quote(&machine.location)
            );
            for state in &machine.states {
                let mut attributes = vec![format!("label=\"{}\"", quote(&state.name))];
                if state.stop == Stop::Always {
                    attributes.push("peripheries=2".to_owned());
                }
                let unreachable = report
                    .reachable
                    .as_ref()
                    .is_some_and(|reachable| !reachable.contains(&state.name));
                if unreachable {
                    attributes.push("style=dashed".to_owned());
                }
                let _ = writeln!(out, "    {} [{}];", id(&state.name), attributes.join(", "));
            }
            if let Some(initial) = &machine.initial {
                let _ = writeln!(out, "    m{number}_start [shape=point];");
                for state in initial {
                    let _ = writeln!(out, "    m{number}_start -> {};", id(state));
                }
            }
            for (from, to) in &report.transitions {
                let known = machine
                    .states
                    .iter()
                    .any(|s| s.name == *from && s.successors != Successors::Unknown);
                if known {
                    let _ = writeln!(out, "    {} -> {};", id(from), id(to));
                }
            }
            let unknown: Vec<&str> = machine
                .states
                .iter()
                .filter(|s| s.successors == Successors::Unknown)
                .map(|s| s.name.as_str())
                .collect();
            if !unknown.is_empty() {
                let _ = writeln!(out, "    m{number}_any [label=\"?\", shape=plaintext];");
                for state in unknown {
                    let _ = writeln!(out, "    {} -> m{number}_any [style=dashed];", id(state));
                }
            }
            out.push_str("  }\n");
            number += 1;
        }
    }
    out.push_str("}\n");
    out
}
