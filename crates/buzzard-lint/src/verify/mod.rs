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
//! * a state from which the model cannot reach a stop is one from which
//!   the loop's condition terminal can never stop the loop.
//!
//! The reverse does not hold: a state the model *can* reach may still be
//! unreachable in practice. Each result says which kind it is.
//!
//! What the proofs rest on, beyond the algorithm:
//!
//! * the reading of the file format described in [`mod@extract`], which was
//!   checked against real files but never against a running editor;
//! * a block diagram that decoded completely. The decoder skips what it
//!   cannot resolve, and a wire it skipped would look like a tunnel left
//!   unwired. A diagram with any recorded [`buzzard_rsrc::Loss`] is
//!   therefore not searched for classic machines at all;
//! * "stop" means that the loop's own condition terminal ends the loop,
//!   and nothing else. A program can also be ended from inside a subVI,
//!   through a call by reference, an invoke or property node or a call
//!   into a library, by an error dialog or by the user. None of that is
//!   modelled. The second result is labelled as proven only when the
//!   whole block diagram holds no node of those kinds (the conservative
//!   reading: such a node anywhere on the diagram may run in the same
//!   iteration or in parallel); otherwise it is reported one level lower,
//!   as [`Confidence::Structural`], and names the nodes;
//! * a state is "unreachable" from the loop's first iteration onwards; the
//!   model says nothing about code outside the loop;
//! * for an enumeration whose type is a type definition, the item list is
//!   the copy stored in the file when it was saved. The type definition
//!   file itself is not read or compared, so a proof is about the VI as
//!   saved; the findings say so.
//!
//! A third result, a state name that no case handles, is a fact about the
//! diagram (a constant with the value is wired toward the state register
//! and no case is labelled with it), not about which paths run. It is
//! labelled [`Confidence::Structural`], never as proven, and is reported
//! once per machine and value.
//!
//! String/queue state machines in the style of the JKI State Machine are
//! recognised but not modelled; they get one heuristic check, labelled as
//! such. See [`mod@extract`].

pub mod extract;
pub mod model;
pub mod report;

use buzzard_rsrc::BlockDiagram;
use serde::Serialize;

pub use extract::{extract, Extraction, QueueMachine, MAX_STATES};
pub use model::{Finding, Machine, Report, State, Stop, Successors};

/// How much an [`Issue`] claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Holds for every execution of the program, because it follows from
    /// the over-approximating model; see the module docs for what the
    /// proof rests on.
    Proven,
    /// A fact about what is wired on the diagram. It is not a statement
    /// about what the program does when it runs: the path in question may
    /// never execute, or a node the model does not look into may end the
    /// program first.
    Structural,
    /// A pattern match on names. It can be wrong in both directions.
    Heuristic,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::Proven => "PROVEN",
            Confidence::Structural => "STRUCTURAL",
            Confidence::Heuristic => "HEURISTIC",
        }
    }

    /// The words a one-line message ends in, e.g. in a lint diagnostic.
    pub fn suffix(self) -> &'static str {
        match self {
            Confidence::Proven => "proven",
            Confidence::Structural => "structural, not proven",
            Confidence::Heuristic => "heuristic, not proven",
        }
    }
}

/// What is wrong with a machine.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Problem {
    /// No execution reaches this state.
    UnreachableState { state: String },
    /// Once in this state the loop's condition terminal can never stop
    /// the loop.
    TrapState { state: String },
    /// The loop's condition terminal can never stop the loop, whatever
    /// state it is in.
    NeverStops,
    /// A constant that no case handles can reach the state register; the
    /// default case would run for it.
    UndefinedState {
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
    /// For a finding that the loop cannot stop: the nodes on the block
    /// diagram that could end the program some other way; see
    /// [`Machine::other_exits`]. While there are any the finding is not
    /// labelled as proven.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub other_exits: Vec<String>,
    /// For a proof about an enumeration machine: the type definition
    /// whose saved copy the proof is about; see
    /// [`Machine::type_definition`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_definition: Option<String>,
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
            Problem::UnreachableState { .. } => Confidence::Proven,
            Problem::TrapState { .. } | Problem::NeverStops if self.other_exits.is_empty() => {
                Confidence::Proven
            }
            Problem::TrapState { .. } | Problem::NeverStops | Problem::UndefinedState { .. } => {
                Confidence::Structural
            }
            Problem::UnhandledQueueState { .. } => Confidence::Heuristic,
        }
    }

    /// What was found, in one line.
    pub fn message(&self) -> String {
        // What else could end the program, for a loop that cannot stop.
        let exits = if self.other_exits.is_empty() {
            String::new()
        } else {
            format!(
                "; the program could still be ended by: {}",
                self.other_exits.join(", ")
            )
        };
        // Which copy of the item list a proof is about.
        let saved = match &self.type_definition {
            Some(file) => format!(
                " (going by the items of the type definition `{file}` as saved in this file)"
            ),
            None => String::new(),
        };
        match &self.problem {
            Problem::UnreachableState { state } => {
                format!("state `{state}` can never be reached{saved}")
            }
            Problem::TrapState { state } => format!(
                "once in state `{state}` the loop's condition terminal can never stop the \
                 loop{exits}{saved}"
            ),
            Problem::NeverStops => {
                format!("the loop's condition terminal can never stop the loop{exits}{saved}")
            }
            Problem::UndefinedState { target, handled_by } => format!(
                "a constant `{target}` that no case handles can reach the state register; {}",
                match handled_by {
                    Some(state) => format!("the default case `{state}` would run for it"),
                    None => "there is no default case to run for it".to_owned(),
                }
            ),
            Problem::UnhandledQueueState { name, handled_by } => format!(
                "state `{name}` is enqueued but no case handles it; the default case \
                 `{handled_by}` runs instead"
            ),
        }
    }

    /// Why the finding holds, in one sentence.
    pub fn justification(&self) -> &'static str {
        let proven = self.confidence() == Confidence::Proven;
        match self.problem {
            Problem::UnreachableState { .. } => {
                "no chain of next-state values the diagram can write leads from the initial \
                 state to this one, and every value the analysis could not follow was assumed \
                 to lead anywhere"
            }
            Problem::TrapState { .. } if proven => {
                "no state that can be reached from this one can make the loop's condition \
                 terminal stop the loop, every condition the analysis could not read was \
                 assumed able to stop it, and the block diagram holds no subVI call or other \
                 node that could end the program some other way"
            }
            Problem::TrapState { .. } => {
                "no state that can be reached from this one can make the loop's condition \
                 terminal stop the loop, and every condition the analysis could not read was \
                 assumed able to stop it; but the nodes named are not looked into and could \
                 end the program or abort the VI, so this does not prove that the loop runs \
                 forever"
            }
            Problem::NeverStops if proven => {
                "every reachable state feeds the loop's condition terminal a constant that \
                 keeps the loop running, and the block diagram holds no subVI call or other \
                 node that could end the program some other way"
            }
            Problem::NeverStops => {
                "every reachable state feeds the loop's condition terminal a constant that \
                 keeps the loop running; but the nodes named are not looked into and could \
                 end the program or abort the VI, so this does not prove that the loop runs \
                 forever"
            }
            Problem::UndefinedState { .. } => {
                "a constant with this value is wired toward the state register and no case is \
                 labelled with it, even ignoring letter case; whether a path that carries it \
                 ever runs is not determined"
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
    /// The block diagram did not decode completely, so it was not
    /// searched for classic machines; see [`Extraction::incomplete`].
    pub incomplete: bool,
}

impl Verification {
    pub fn is_empty(&self) -> bool {
        self.machines.is_empty() && self.queue_machines.is_empty()
    }

    /// Every finding, sorted.
    pub fn issues(&self) -> Vec<Issue> {
        let mut issues = Vec::new();
        for report in &self.machines {
            let machine = &report.machine;
            issues.extend(report.findings.iter().map(|finding| {
                let problem = match finding.clone() {
                    Finding::Unreachable { state } => Problem::UnreachableState { state },
                    Finding::CannotTerminate { state } => Problem::TrapState { state },
                    Finding::NeverTerminates => Problem::NeverStops,
                    Finding::UndefinedTarget { target, handled_by } => {
                        Problem::UndefinedState { target, handled_by }
                    }
                };
                let stops = matches!(problem, Problem::TrapState { .. } | Problem::NeverStops);
                let proof = !matches!(problem, Problem::UndefinedState { .. });
                Issue {
                    location: machine.location.clone(),
                    problem,
                    other_exits: match stops {
                        true => machine.other_exits.clone(),
                        false => Vec::new(),
                    },
                    type_definition: machine.type_definition.clone().filter(|_| proof),
                }
            }));
        }
        for machine in &self.queue_machines {
            issues.extend(machine.unhandled.iter().map(|name| Issue {
                location: machine.location.clone(),
                problem: Problem::UnhandledQueueState {
                    name: name.clone(),
                    handled_by: machine.default.clone(),
                },
                other_exits: Vec::new(),
                type_definition: None,
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
        incomplete: found.incomplete,
    }
}
