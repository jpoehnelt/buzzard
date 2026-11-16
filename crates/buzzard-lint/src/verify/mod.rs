//! Verification of state machines found on block diagrams.
//!
//! A state machine here is the classic pattern: a while loop around a case
//! structure, with the case selected by a shift register that each case
//! writes its successor into. [`model`] holds the abstract machine and the
//! checks that are run on it. [`mod@extract`] finds the machines on a diagram
//! and builds a [`Machine`] for each; its documentation lists every
//! assumption it makes. [`report`] prints the results.
//!
//! # What "verified" means
//!
//! The model is an *over-approximation* of the real program: where a case
//! chooses its successor from data the analysis cannot follow, every state
//! is assumed possible, and a stop condition it cannot read is assumed to
//! be able to go either way. The real program's behaviours are therefore a
//! subset of the model's. That makes the two "cannot happen" results
//! proofs, not heuristics:
//!
//! * a state the model cannot reach is unreachable in the program;
//! * a state from which the model cannot reach a stop cannot terminate in
//!   the program.
//!
//! The reverse does not hold: a state the model *can* reach may still be
//! unreachable in practice. Each result says which kind it is.
//!
//! What the proofs rest on, beyond the algorithm:
//!
//! * the reading of the file format described in [`mod@extract`], which was
//!   checked against real files but never against a running editor;
//! * "terminate" means that the loop's own condition terminal ends the
//!   loop. A program can also be aborted from inside a subVI, by an error
//!   dialog or by the user, and none of that is modelled;
//! * a state is "unreachable" from the loop's first iteration onwards; the
//!   model says nothing about code outside the loop.
//!
//! A third result, a state name that no case handles, is a fact about the
//! diagram (the value is wired to the state register and no case is
//! labelled with it), not about which paths run.
//!
//! String/queue state machines in the style of the JKI State Machine are
//! recognised but not modelled; they get one heuristic check, labelled as
//! such. See [`mod@extract`].

pub mod extract;
pub mod model;
pub mod report;

use buzzard_rsrc::BlockDiagram;
use serde::Serialize;

pub use extract::{extract, Extraction, QueueMachine};
pub use model::{Finding, Machine, Report, State, Stop, Successors};

/// How much an [`Issue`] claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Follows from the over-approximating model; see the module docs for
    /// what the proof rests on.
    Proven,
    /// A pattern match on names. It can be wrong in both directions.
    Heuristic,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::Proven => "PROVEN",
            Confidence::Heuristic => "HEURISTIC",
        }
    }
}

/// What is wrong with a machine.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Problem {
    /// No execution reaches this state.
    UnreachableState { state: String },
    /// Once in this state the loop can never stop.
    TrapState { state: String },
    /// The loop never stops, whatever state it is in.
    NeverStops,
    /// A state writes a name no case handles; the default case runs.
    UndefinedState {
        from: String,
        target: String,
        handled_by: Option<String>,
    },
    /// A string/queue machine enqueues a name no case handles.
    UnhandledQueueState { name: String, handled_by: String },
}

/// One finding about one machine, ready to show.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Issue {
    /// Where the machine is on the diagram, e.g.
    /// `while loop #38, case structure #49`.
    pub location: String,
    #[serde(flatten)]
    pub problem: Problem,
}

impl Issue {
    /// The lint rule that reports this issue.
    pub fn code(&self) -> &'static str {
        match self.problem {
            Problem::UnreachableState { .. } => "BZ201",
            Problem::TrapState { .. } | Problem::NeverStops => "BZ202",
            Problem::UndefinedState { .. } | Problem::UnhandledQueueState { .. } => "BZ203",
        }
    }

    pub fn confidence(&self) -> Confidence {
        match self.problem {
            Problem::UnhandledQueueState { .. } => Confidence::Heuristic,
            _ => Confidence::Proven,
        }
    }

    /// What was found, in one line.
    pub fn message(&self) -> String {
        let default = |handled_by: &Option<String>| match handled_by {
            Some(state) => format!("the default case `{state}` runs instead"),
            None => "there is no default case to run instead".to_owned(),
        };
        match &self.problem {
            Problem::UnreachableState { state } => {
                format!("state `{state}` can never be reached")
            }
            Problem::TrapState { state } => {
                format!("once in state `{state}` the loop can never stop")
            }
            Problem::NeverStops => "the loop can never stop".to_owned(),
            Problem::UndefinedState {
                from,
                target,
                handled_by,
            } => format!(
                "state `{from}` writes the next state `{target}`, which no case handles; {}",
                default(handled_by)
            ),
            Problem::UnhandledQueueState { name, handled_by } => format!(
                "state `{name}` is enqueued but no case handles it; the default case \
                 `{handled_by}` runs instead"
            ),
        }
    }

    /// Why the finding holds, in one sentence.
    pub fn justification(&self) -> &'static str {
        match self.problem {
            Problem::UnreachableState { .. } => {
                "no chain of next-state values the diagram can write leads from the initial \
                 state to this one, and every value the analysis could not follow was assumed \
                 to lead anywhere"
            }
            Problem::TrapState { .. } => {
                "no state that can be reached from this one can make the loop's condition \
                 terminal stop the loop, and every condition the analysis could not read was \
                 assumed able to stop it"
            }
            Problem::NeverStops => {
                "every reachable state feeds the loop's condition terminal a constant that \
                 keeps the loop running"
            }
            Problem::UndefinedState { .. } => {
                "a constant with this value is wired to the state register from this state \
                 and no case is labelled with it, even ignoring letter case; whether that \
                 path runs is not determined"
            }
            Problem::UnhandledQueueState { .. } => {
                "the name appears in a string constant wired to the queue and matches no case \
                 label; names built at run time or enqueued elsewhere are not seen"
            }
        }
    }
}

/// Everything found on one diagram.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Verification {
    /// Classic machines with the result of checking each.
    pub machines: Vec<Report>,
    /// String/queue machines, which are not modelled.
    pub queue_machines: Vec<QueueMachine>,
}

impl Verification {
    pub fn is_empty(&self) -> bool {
        self.machines.is_empty() && self.queue_machines.is_empty()
    }

    /// Every finding, sorted.
    pub fn issues(&self) -> Vec<Issue> {
        let mut issues = Vec::new();
        for report in &self.machines {
            let location = &report.machine.location;
            issues.extend(report.findings.iter().map(|finding| Issue {
                location: location.clone(),
                problem: match finding.clone() {
                    Finding::Unreachable { state } => Problem::UnreachableState { state },
                    Finding::CannotTerminate { state } => Problem::TrapState { state },
                    Finding::NeverTerminates => Problem::NeverStops,
                    Finding::UndefinedTarget {
                        from,
                        target,
                        handled_by,
                    } => Problem::UndefinedState {
                        from,
                        target,
                        handled_by,
                    },
                },
            }));
        }
        for machine in &self.queue_machines {
            issues.extend(machine.unhandled.iter().map(|name| Issue {
                location: machine.location.clone(),
                problem: Problem::UnhandledQueueState {
                    name: name.clone(),
                    handled_by: machine.default.clone(),
                },
            }));
        }
        issues.sort();
        issues.dedup();
        issues
    }
}

/// Find and check every state machine on a diagram. The result is
/// deterministic.
pub fn verify(diagram: &BlockDiagram) -> Verification {
    let found = extract(diagram);
    Verification {
        machines: found.machines.iter().map(Machine::check).collect(),
        queue_machines: found.queue_machines,
    }
}
