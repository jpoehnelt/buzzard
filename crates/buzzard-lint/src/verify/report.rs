//! Running the verification over files and printing what it found.
//!
//! Every output is deterministic. Files are in path order, machines in
//! diagram order, and states, transitions, and findings in the order the
//! model gives them.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Serialize;

use super::{verify, Confidence, Issue, QueueMachine, Report, Stop, Successors, Verification};
use crate::archive;
use crate::inspector::Inspection;
use crate::project::is_project_file;
use crate::workspace::discover;

/// One finding as it is shown: the issue plus its rule code, how much it
/// claims, and why it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShownIssue {
    /// The lint rule that reports the issue, for example `BZ201`.
    pub code: &'static str,
    /// How much the finding claims.
    pub confidence: Confidence,
    /// The finding itself.
    #[serde(flatten)]
    pub issue: Issue,
    /// The finding in one sentence.
    pub message: String,
    /// Why the finding holds and what it does not say.
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
    /// Path of the file.
    pub path: PathBuf,
    /// Classic state machines: the model, what was derived from it, and
    /// whether the model is exact.
    pub machines: Vec<Report>,
    /// String/queue machines, which are recognized but not modeled.
    pub queue_machines: Vec<QueueMachine>,
    /// Every finding about the machines of the file, sorted.
    pub findings: Vec<ShownIssue>,
}

/// Counts over every file of a [`Verified`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Summary {
    /// Files with a block diagram that were analyzed.
    pub files: usize,
    /// Files with at least one state machine of either kind.
    pub files_with_machines: usize,
    /// Analyzed files in which no state machine was found.
    pub files_without_machines: usize,
    /// Classic state machines found.
    pub machines: usize,
    /// String/queue machines found.
    pub queue_machines: usize,
    /// Files whose block diagram did not decode completely and was
    /// therefore not searched for classic state machines.
    pub incomplete: usize,
    /// Findings of every confidence.
    pub findings: usize,
    /// Findings labeled [`Confidence::Proven`].
    pub proven: usize,
    /// Findings labeled [`Confidence::Structural`].
    pub structural: usize,
    /// Findings labeled [`Confidence::Heuristic`].
    pub heuristic: usize,
}

/// The result of verifying a set of files.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Verified {
    /// The files that have a state machine, sorted by path.
    pub files: Vec<FileMachines>,
    /// Files that could not be read or parsed, with the reason.
    pub unreadable: Vec<(PathBuf, String)>,
    /// Files whose block diagram did not decode completely. Nothing is
    /// claimed about classic state machines on them.
    pub incomplete: Vec<PathBuf>,
    /// Counts over all files.
    pub summary: Summary,
}

impl Verified {
    /// Collect per-file results. `results` holds every analyzed file,
    /// whether it has a machine or not. `None` is a file without a block
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
            if verification.incomplete {
                out.summary.incomplete += 1;
                out.incomplete.push(path.clone());
            }
            if verification.is_empty() {
                out.summary.files_without_machines += 1;
                continue;
            }
            let findings: Vec<ShownIssue> = verification
                .issues()
                .into_iter()
                .map(ShownIssue::from)
                .collect();
            let count = |confidence: Confidence| {
                findings
                    .iter()
                    .filter(|f| f.confidence == confidence)
                    .count()
            };
            out.summary.files_with_machines += 1;
            out.summary.machines += verification.machines.len();
            out.summary.queue_machines += verification.queue_machines.len();
            out.summary.findings += findings.len();
            out.summary.proven += count(Confidence::Proven);
            out.summary.structural += count(Confidence::Structural);
            out.summary.heuristic += count(Confidence::Heuristic);
            out.files.push(FileMachines {
                path,
                machines: verification.machines,
                queue_machines: verification.queue_machines,
                findings,
            });
        }
        out
    }

    /// Verify every binary file under the given files and directories,
    /// and every VI inside an `.llb` archive among them. A member is
    /// listed under the path of its archive followed by its name. An
    /// archive that cannot be opened, or that has a member that cannot
    /// be, is listed as unreadable.
    pub fn scan(roots: &[PathBuf]) -> Self {
        let paths: Vec<PathBuf> = discover(roots)
            .into_iter()
            .filter(|path| !is_project_file(path))
            .collect();
        let results: Vec<Vec<_>> = paths
            .into_par_iter()
            .map(|path| {
                if !archive::is_archive(&path) || archive::split(&path).is_some() {
                    let result = verify_file(&path);
                    return vec![(path, result)];
                }
                // Project files among the members have no diagram.
                let verified = archive::open(&path, |member, bytes| {
                    (!is_project_file(member)).then(|| {
                        let inspection = Inspection::from_bytes(&bytes?)?;
                        Ok(inspection.diagram.as_ref().map(verify))
                    })
                });
                match verified {
                    Err(reason) => vec![(path, Err(reason))],
                    // An archive without a directory is a resource file.
                    Ok(None) => {
                        let result = verify_file(&path);
                        vec![(path, result)]
                    }
                    Ok(Some(opened)) => {
                        let mut out = Vec::new();
                        if !opened.file.problems.is_empty() {
                            out.push((path, Err(opened.file.problems.join("; "))));
                        }
                        let members = opened.members.into_iter();
                        out.extend(members.filter_map(|(path, result)| Some((path, result?))));
                        out
                    }
                }
            })
            .collect();
        // A member that was also given as a root of its own is one file.
        let mut results: Vec<_> = results.into_iter().flatten().collect();
        results.sort_by(|a, b| a.0.cmp(&b.0));
        results.dedup_by(|later, first| later.0 == first.0);
        Self::new(results)
    }

    /// Whether any file has a finding of any confidence.
    pub fn has_findings(&self) -> bool {
        self.summary.findings > 0
    }
}

/// Verify one file. `Ok(None)` when it has no block diagram.
///
/// # Errors
///
/// Returns the reason as text when the file cannot be read or parsed.
pub fn verify_file(path: &Path) -> Result<Option<Verification>, String> {
    let inspection = Inspection::from_path(path)?;
    Ok(inspection.diagram.as_ref().map(verify))
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Names separated by commas. A name that holds a comma itself (a case
/// that handles several values is named `A, B`) is put in backticks, so
/// that it cannot be read as two names.
fn list(names: &[&str]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names
            .iter()
            .map(|name| {
                if name.contains(',') {
                    format!("`{}`", clean(name))
                } else {
                    clean(name)
                }
            })
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
                "  string/queue state machine at {} (recognized, not modeled)",
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
    for path in &verified.incomplete {
        let _ = writeln!(
            out,
            "{}: the block diagram did not decode completely; it was not searched for state \
             machines, because nothing could be proven about one",
            path.display()
        );
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
            "{}: {} proven, {} structural, {} heuristic.",
            plural(s.findings, "finding", "findings"),
            s.proven,
            s.structural,
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
        None => "unknown (the state register is not initialized with a constant)".to_owned(),
    };
    let _ = writeln!(out, "    initial state: {initial}");
    let _ = writeln!(
        out,
        "    default state: {}",
        machine.default.as_deref().map_or("none".to_owned(), clean)
    );
    if let Some(file) = &machine.type_definition {
        let _ = writeln!(
            out,
            "    states are the items of the type definition {} as saved in this file; the type \
             definition itself was not read",
            clean(file)
        );
    }
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
    let mut successors: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (from, to) in &report.transitions {
        successors.entry(from).or_default().push(to);
    }
    for state in &machine.states {
        let stop = match state.stop {
            Stop::Always => "stops the loop",
            Stop::Never => "never stops the loop",
            Stop::Maybe => "may stop the loop",
        };
        let targets = match &state.successors {
            Successors::Unknown => "? (any state)".to_owned(),
            Successors::Known(_) => list(
                successors
                    .get(state.name.as_str())
                    .map_or(&[][..], Vec::as_slice),
            ),
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
///
/// The work is linear in the number of states and transitions.
pub fn to_dot(verified: &Verified) -> String {
    let mut out = String::from("digraph state_machines {\n  rankdir=LR;\n  node [shape=box];\n");
    let mut number = 0;
    for file in &verified.files {
        for report in &file.machines {
            let machine = &report.machine;
            let position: BTreeMap<&str, usize> = machine
                .states
                .iter()
                .enumerate()
                .map(|(index, state)| (state.name.as_str(), index))
                .collect();
            let id = |state: &str| {
                let index = position.get(state).copied();
                format!("m{number}_s{}", index.unwrap_or(usize::MAX))
            };
            let reachable: Option<std::collections::BTreeSet<&str>> = report
                .reachable
                .as_ref()
                .map(|names| names.iter().map(String::as_str).collect());
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
                let unreachable = reachable
                    .as_ref()
                    .is_some_and(|reachable| !reachable.contains(state.name.as_str()));
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
            // Only known successors are listed as transitions.
            for (from, to) in &report.transitions {
                let _ = writeln!(out, "    {} -> {};", id(from), id(to));
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
