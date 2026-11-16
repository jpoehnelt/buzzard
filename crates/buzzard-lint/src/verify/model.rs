//! The abstract state machine and the properties checked on it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;

/// Which states a state may hand over to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Successors {
    /// Exactly these state names can be written as the next state. A name
    /// that is not a state of the machine selects the default state; the
    /// extraction never produces one and uses [`State::unmatched`] for
    /// values without a case instead.
    Known(BTreeSet<String>),
    /// The next state comes from data the analysis cannot follow.
    Unknown,
}

/// Whether the loop stops after an iteration spent in a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    Never,
    Always,
    /// Depends on data, or could not be read.
    Maybe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct State {
    pub name: String,
    pub successors: Successors,
    /// Values this state can write as the next state that no case is
    /// labelled with, so the default state handles them. Kept apart from
    /// the successor names so that such a value can never be mistaken for
    /// a state whose name happens to read the same.
    pub unmatched: BTreeSet<String>,
    pub stop: Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Machine {
    /// Where the machine is on the diagram, e.g. `while loop #12`.
    pub location: String,
    /// States in case-frame order. Names are unique.
    pub states: Vec<State>,
    /// Possible first states; `None` when the initial value is not a
    /// constant the analysis could read.
    pub initial: Option<BTreeSet<String>>,
    /// The state that handles any value without a case of its own.
    pub default: Option<String>,
    /// File name of the type definition behind an enumeration selector,
    /// e.g. `State.ctl`. The states are then the items of the copy of the
    /// type that the file stored when it was saved; the type definition
    /// itself is not read, so a later change to it is not seen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_definition: Option<String>,
    /// Nodes on the block diagram that the model does not look into and
    /// that could end the program or abort the VI by other means than the
    /// loop's condition terminal: calls of subVIs, calls by reference,
    /// invoke and property nodes, calls into libraries and nodes of
    /// classes that are not known to be harmless. Sorted descriptions
    /// without repeats; a long list ends in a count of the rest. While
    /// there are any, a finding that the loop cannot stop is not labelled
    /// as proven.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub other_exits: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Finding {
    /// Proven: no execution reaches this state.
    Unreachable { state: String },
    /// Proven: once in this state the loop's condition terminal can never
    /// stop the loop.
    CannotTerminate { state: String },
    /// A value that has no case can be handed over as the next state; the
    /// default state would run for it. Usually a misspelled or deleted
    /// state. Reported once per machine and value, and only for values
    /// written by states that are not proven dead and that do not always
    /// stop the loop. It is a fact about the diagram: whether the path
    /// that carries the value ever runs is not determined.
    UndefinedTarget {
        target: String,
        handled_by: Option<String>,
    },
    /// Proven: the condition terminal never stops the loop from any
    /// starting state.
    NeverTerminates,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub machine: Machine,
    /// States the model can reach, in state order. `None` when the initial
    /// state is unknown, in which case nothing is claimed about
    /// reachability.
    pub reachable: Option<Vec<String>>,
    /// Resolved transitions `(from, to)`, sorted, for display and export.
    /// A state whose successors are [`Successors::Unknown`] may hand over
    /// to any state and has no entry here: listing those would take
    /// space that grows with the square of the number of states.
    pub transitions: Vec<(String, String)>,
    pub findings: Vec<Finding>,
    /// True when the initial state, every successor set and every stop
    /// condition was read from constants on the diagram, so that nothing
    /// in the model is a guess at data the analysis could not follow. The
    /// transitions are still "may" transitions: which of several constants
    /// a state writes can depend on data.
    pub exact: bool,
}

impl Machine {
    fn index(&self) -> BTreeMap<&str, usize> {
        self.states
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name.as_str(), i))
            .collect()
    }

    /// The state a written name selects: the state of that name, else the
    /// default state, else nothing (the value has no case at all).
    fn resolve(&self, index: &BTreeMap<&str, usize>, name: &str) -> Option<usize> {
        index
            .get(name)
            .copied()
            .or_else(|| index.get(self.default.as_deref()?).copied())
    }

    /// Successor state indices of each state in the model; `None` for a
    /// state that may hand over to any state. That case is kept implicit
    /// so that the work stays linear in the size of the machine.
    fn edges(&self, index: &BTreeMap<&str, usize>) -> Vec<Option<BTreeSet<usize>>> {
        self.states
            .iter()
            .map(|state| match &state.successors {
                Successors::Unknown => None,
                Successors::Known(names) => Some(
                    names
                        .iter()
                        .filter_map(|name| self.resolve(index, name))
                        .chain(
                            // An unmatched value runs the default state.
                            (!state.unmatched.is_empty())
                                .then(|| index.get(self.default.as_deref()?).copied())
                                .flatten(),
                        )
                        .collect(),
                ),
            })
            .collect()
    }

    /// Check the machine. The result is deterministic, and the time and
    /// space taken are linear in the number of states plus the number of
    /// known successors.
    pub fn check(&self) -> Report {
        let index = self.index();
        let edges = self.edges(&index);
        let count = self.states.len();

        // Forward reachability from the initial states.
        let reachable: Option<Vec<bool>> = self.initial.as_ref().map(|initial| {
            let mut seen = vec![false; count];
            let mut queue: VecDeque<usize> = initial
                .iter()
                .filter_map(|name| self.resolve(&index, name))
                .collect();
            let mut anywhere = false;
            while let Some(at) = queue.pop_front() {
                if std::mem::replace(&mut seen[at], true) {
                    continue;
                }
                match &edges[at] {
                    Some(next) => queue.extend(next.iter().copied().filter(|&next| !seen[next])),
                    // The first state that may go anywhere reaches every
                    // state; a second one adds nothing.
                    None if !anywhere => {
                        anywhere = true;
                        queue.extend((0..count).filter(|&next| !seen[next]));
                    }
                    None => {}
                }
            }
            seen
        });

        // Backward reachability from every state that may stop: the states
        // left over can never reach a stop.
        let mut can_stop = vec![false; count];
        let mut queue: VecDeque<usize> = (0..count)
            .filter(|&i| self.states[i].stop != Stop::Never)
            .collect();
        if !queue.is_empty() {
            // A state that may go anywhere reaches a state that may stop
            // as soon as there is one.
            queue.extend((0..count).filter(|&i| edges[i].is_none()));
        }
        let mut reverse: Vec<Vec<usize>> = vec![Vec::new(); count];
        for (from, successors) in edges.iter().enumerate() {
            for &to in successors.iter().flatten() {
                reverse[to].push(from);
            }
        }
        while let Some(at) = queue.pop_front() {
            if std::mem::replace(&mut can_stop[at], true) {
                continue;
            }
            queue.extend(reverse[at].iter().copied().filter(|&prev| !can_stop[prev]));
        }

        let mut findings = Vec::new();
        let live = |i: usize| reachable.as_ref().is_none_or(|seen| seen[i]);
        if let Some(seen) = &reachable {
            for (i, state) in self.states.iter().enumerate() {
                if !seen[i] {
                    findings.push(Finding::Unreachable {
                        state: state.name.clone(),
                    });
                }
            }
        }
        // Report non-termination once for the whole machine when no live
        // state can stop, otherwise per state.
        let trapped: Vec<usize> = (0..count).filter(|&i| live(i) && !can_stop[i]).collect();
        let any_live = (0..count).any(live);
        if any_live && trapped.len() == (0..count).filter(|&i| live(i)).count() {
            findings.push(Finding::NeverTerminates);
        } else {
            for i in trapped {
                findings.push(Finding::CannotTerminate {
                    state: self.states[i].name.clone(),
                });
            }
        }
        // Values without a case, once per machine and value.
        let mut undefined: BTreeSet<&String> = BTreeSet::new();
        for (i, state) in self.states.iter().enumerate() {
            let Successors::Known(names) = &state.successors else {
                continue;
            };
            // Say nothing about what a state writes when it is proven
            // never to run, or when it always stops the loop, so that the
            // value is never used to select a case.
            if !live(i) || state.stop == Stop::Always {
                continue;
            }
            undefined.extend(
                names
                    .iter()
                    .filter(|name| !index.contains_key(name.as_str()))
                    .chain(&state.unmatched),
            );
        }
        findings.extend(undefined.into_iter().map(|name| Finding::UndefinedTarget {
            target: name.clone(),
            handled_by: self.default.clone(),
        }));
        findings.sort();

        let mut transitions = BTreeSet::new();
        for (from, successors) in edges.iter().enumerate() {
            for &to in successors.iter().flatten() {
                transitions.insert((self.states[from].name.clone(), self.states[to].name.clone()));
            }
        }
        let exact = self.initial.is_some()
            && self
                .states
                .iter()
                .all(|s| s.successors != Successors::Unknown && s.stop != Stop::Maybe);
        Report {
            machine: self.clone(),
            reachable: reachable.map(|seen| {
                self.states
                    .iter()
                    .zip(seen)
                    .filter(|(_, hit)| *hit)
                    .map(|(s, _)| s.name.clone())
                    .collect()
            }),
            transitions: transitions.into_iter().collect(),
            findings,
            exact,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(names: &[&str]) -> Successors {
        Successors::Known(names.iter().map(|n| (*n).to_owned()).collect())
    }

    fn state(name: &str, successors: Successors, stop: Stop) -> State {
        State {
            name: name.to_owned(),
            successors,
            unmatched: BTreeSet::new(),
            stop,
        }
    }

    fn machine(states: Vec<State>, initial: Option<&[&str]>, default: Option<&str>) -> Machine {
        Machine {
            location: "while loop #1".to_owned(),
            states,
            initial: initial.map(|names| names.iter().map(|n| (*n).to_owned()).collect()),
            default: default.map(str::to_owned),
            ..Machine::default()
        }
    }

    fn unreachable(name: &str) -> Finding {
        Finding::Unreachable {
            state: name.to_owned(),
        }
    }

    #[test]
    fn healthy_machine_has_no_findings_and_is_exact() {
        let m = machine(
            vec![
                state("Init", known(&["Run"]), Stop::Never),
                state("Run", known(&["Run", "Exit"]), Stop::Never),
                state("Exit", known(&["Exit"]), Stop::Always),
            ],
            Some(&["Init"]),
            None,
        );
        let report = m.check();
        assert_eq!(report.findings, []);
        assert!(report.exact);
        assert_eq!(report.reachable.unwrap(), ["Init", "Run", "Exit"]);
        assert_eq!(
            report.transitions,
            [
                ("Exit", "Exit"),
                ("Init", "Run"),
                ("Run", "Exit"),
                ("Run", "Run")
            ]
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
        );
    }

    #[test]
    fn proves_states_unreachable() {
        let m = machine(
            vec![
                state("Init", known(&["Exit"]), Stop::Never),
                state("Orphan", known(&["Exit"]), Stop::Never),
                state("Exit", known(&[]), Stop::Always),
            ],
            Some(&["Init"]),
            None,
        );
        assert_eq!(m.check().findings, [unreachable("Orphan")]);
    }

    #[test]
    fn unknown_successor_blocks_unreachability_claims() {
        // "Init" might go anywhere, so "Orphan" can no longer be ruled out.
        let m = machine(
            vec![
                state("Init", Successors::Unknown, Stop::Never),
                state("Orphan", known(&["Exit"]), Stop::Never),
                state("Exit", known(&[]), Stop::Always),
            ],
            Some(&["Init"]),
            None,
        );
        let report = m.check();
        assert_eq!(report.findings, []);
        assert!(!report.exact);
    }

    #[test]
    fn unknown_initial_state_makes_no_reachability_claim() {
        let m = machine(
            vec![
                state("A", known(&["A"]), Stop::Always),
                state("B", known(&["A"]), Stop::Never),
            ],
            None,
            None,
        );
        let report = m.check();
        assert_eq!(report.reachable, None);
        assert_eq!(report.findings, []);
    }

    #[test]
    fn proves_a_trap_state_cannot_terminate() {
        let m = machine(
            vec![
                state("Init", known(&["Work", "Error"]), Stop::Never),
                state("Work", known(&["Done"]), Stop::Never),
                state("Done", known(&[]), Stop::Always),
                // Loops between two states that never stop.
                state("Error", known(&["Retry"]), Stop::Never),
                state("Retry", known(&["Error"]), Stop::Never),
            ],
            Some(&["Init"]),
            None,
        );
        assert_eq!(
            m.check().findings,
            [
                Finding::CannotTerminate {
                    state: "Error".to_owned()
                },
                Finding::CannotTerminate {
                    state: "Retry".to_owned()
                },
            ]
        );
    }

    #[test]
    fn a_stop_that_may_happen_is_not_a_trap() {
        let m = machine(
            vec![state("Idle", known(&["Idle"]), Stop::Maybe)],
            Some(&["Idle"]),
            None,
        );
        assert_eq!(m.check().findings, []);
    }

    #[test]
    fn machine_that_can_never_stop_is_reported_once() {
        let m = machine(
            vec![
                state("A", known(&["B"]), Stop::Never),
                state("B", known(&["A"]), Stop::Never),
                state("Dead", known(&["Dead"]), Stop::Always),
            ],
            Some(&["A"]),
            None,
        );
        assert_eq!(
            m.check().findings,
            [unreachable("Dead"), Finding::NeverTerminates]
        );
    }

    #[test]
    fn undefined_target_goes_to_the_default_state() {
        let m = machine(
            vec![
                state("Init", known(&["Rnu"]), Stop::Never),
                state("Run", known(&["Exit"]), Stop::Never),
                state("Exit", known(&[]), Stop::Always),
                state("Default", known(&["Exit"]), Stop::Never),
            ],
            Some(&["Init"]),
            Some("Default"),
        );
        let report = m.check();
        assert_eq!(
            report.findings,
            [
                unreachable("Run"),
                Finding::UndefinedTarget {
                    target: "Rnu".to_owned(),
                    handled_by: Some("Default".to_owned()),
                },
            ]
        );
        assert!(report
            .transitions
            .contains(&("Init".to_owned(), "Default".to_owned())));
    }

    #[test]
    fn undefined_target_without_default_has_no_successor() {
        let m = machine(
            vec![
                state("Init", known(&["Nowhere"]), Stop::Never),
                state("Exit", known(&[]), Stop::Always),
            ],
            Some(&["Init"]),
            None,
        );
        // Only "Init" ever runs and it can never stop, so the whole machine
        // is reported once.
        assert_eq!(
            m.check().findings,
            [
                unreachable("Exit"),
                Finding::UndefinedTarget {
                    target: "Nowhere".to_owned(),
                    handled_by: None,
                },
                Finding::NeverTerminates,
            ]
        );
    }

    #[test]
    fn an_unmatched_value_runs_the_default_state() {
        let with = |default: Option<&str>| {
            let mut init = state("Init", known(&[]), Stop::Never);
            init.unmatched = ["Run, Go".to_owned()].into();
            machine(
                vec![
                    init,
                    // Named exactly like the unmatched value: still not
                    // the target.
                    state("Run, Go", known(&["Exit"]), Stop::Never),
                    state("Exit", known(&[]), Stop::Always),
                    state("Other", known(&["Exit"]), Stop::Never),
                ],
                Some(&["Init"]),
                default,
            )
        };
        let undefined = |handled_by: Option<&str>| Finding::UndefinedTarget {
            target: "Run, Go".to_owned(),
            handled_by: handled_by.map(str::to_owned),
        };
        let report = with(Some("Other")).check();
        assert_eq!(
            report.findings,
            [unreachable("Run, Go"), undefined(Some("Other"))]
        );
        assert_eq!(report.reachable.unwrap(), ["Init", "Exit", "Other"]);
        // Without a default state the value has nowhere to go.
        assert_eq!(
            with(None).check().findings,
            [
                unreachable("Exit"),
                unreachable("Other"),
                unreachable("Run, Go"),
                undefined(None),
                Finding::NeverTerminates,
            ]
        );
        // A state that never runs is not blamed for what it writes.
        let mut dead = with(Some("Other"));
        dead.initial = Some(["Exit".to_owned()].into());
        assert!(dead
            .check()
            .findings
            .iter()
            .all(|f| matches!(f, Finding::Unreachable { .. })));
    }

    #[test]
    fn an_undefined_value_is_reported_once_and_never_for_a_stopping_state() {
        // Three states write the same undefined value; one of them always
        // stops the loop, so its value is never used to select a case.
        let m = machine(
            vec![
                state("A", known(&["B", "Typo"]), Stop::Never),
                state("B", known(&["C", "Typo"]), Stop::Maybe),
                state("C", known(&["Typo", "Gone"]), Stop::Always),
                state("Other", known(&["A"]), Stop::Never),
            ],
            Some(&["A"]),
            Some("Other"),
        );
        assert_eq!(
            m.check().findings,
            [Finding::UndefinedTarget {
                target: "Typo".to_owned(),
                handled_by: Some("Other".to_owned()),
            }]
        );
    }

    #[test]
    fn unknown_successors_cost_nothing_per_pair_of_states() {
        // Every state may go anywhere. Listing those pairs took space
        // that grew with the square of the number of states: 16,000,000
        // pairs of strings (3.4 GB) for 4,000 states. Here that would be
        // 40,000,000,000 pairs, so the test only ends when none is made.
        const STATES: usize = 200_000;
        let states = |last: Stop| -> Vec<State> {
            (0..STATES)
                .map(|i| {
                    let stop = if i + 1 == STATES { last } else { Stop::Never };
                    state(&format!("s{i}"), Successors::Unknown, stop)
                })
                .collect()
        };
        let started = std::time::Instant::now();
        // One state may stop: every state can reach it, nothing is found.
        let stoppable = machine(states(Stop::Maybe), Some(&["s0"]), None).check();
        assert_eq!(stoppable.findings, []);
        assert_eq!(stoppable.reachable.as_ref().map(Vec::len), Some(STATES));
        assert!(stoppable.transitions.is_empty());
        // No state may stop: reported once, not once per state.
        let endless = machine(states(Stop::Never), Some(&["s0"]), None).check();
        assert_eq!(endless.findings, [Finding::NeverTerminates]);
        assert!(endless.transitions.is_empty());
        // Generous: both checks take well under a second in a debug
        // build; the quadratic version would not end.
        assert!(started.elapsed() < std::time::Duration::from_secs(60));
    }

    #[test]
    fn unknown_and_known_successors_mix_as_before() {
        // "Hub" may go anywhere, so everything is reachable; "Trap" has a
        // known successor set that never leaves it and never stops.
        let m = machine(
            vec![
                state("Init", known(&["Hub"]), Stop::Never),
                state("Hub", Successors::Unknown, Stop::Never),
                state("Trap", known(&["Trap"]), Stop::Never),
                state("Exit", known(&[]), Stop::Always),
                state("Far", known(&["Exit"]), Stop::Never),
            ],
            Some(&["Init"]),
            None,
        );
        let report = m.check();
        assert_eq!(
            report.findings,
            [Finding::CannotTerminate {
                state: "Trap".to_owned()
            }]
        );
        assert_eq!(report.reachable.unwrap().len(), 5);
        // Only the known successors are listed.
        assert_eq!(
            report.transitions,
            [("Far", "Exit"), ("Init", "Hub"), ("Trap", "Trap")]
                .map(|(a, b)| (a.to_owned(), b.to_owned()))
        );
        // Without any state that may stop, the state that may go anywhere
        // cannot reach a stop either.
        let mut endless = m.clone();
        endless.states[3].stop = Stop::Never;
        assert_eq!(endless.check().findings, [Finding::NeverTerminates]);
    }

    #[test]
    fn empty_and_degenerate_machines_do_not_panic() {
        assert_eq!(
            machine(vec![], Some(&["X"]), Some("Y")).check().findings,
            []
        );
        let m = machine(
            vec![state("Only", Successors::Unknown, Stop::Maybe)],
            Some(&[]),
            None,
        );
        assert_eq!(m.check().findings, [unreachable("Only")]);
    }

    #[test]
    fn result_is_deterministic() {
        let m = machine(
            vec![
                state("C", known(&["A", "B", "Zed"]), Stop::Never),
                state("B", known(&["C"]), Stop::Maybe),
                state("A", known(&["C"]), Stop::Never),
            ],
            Some(&["C"]),
            Some("A"),
        );
        let first = m.check();
        for _ in 0..10 {
            assert_eq!(m.check(), first);
        }
    }
}
