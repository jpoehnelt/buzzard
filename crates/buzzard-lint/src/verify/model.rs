//! The abstract state machine and the properties checked on it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;

/// Which states a state may hand over to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Successors {
    /// Exactly these state names can be written as the next state. A name
    /// that is not a state of the machine selects the default state.
    Known(BTreeSet<String>),
    /// The next state comes from data the analysis cannot follow.
    Unknown,
}

/// Whether the loop stops after an iteration spent in a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    /// The loop always runs another iteration.
    Never,
    /// The loop always stops.
    Always,
    /// Depends on data, or could not be read.
    Maybe,
}

/// One state: a case of the case structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct State {
    /// Name of the state: the value that selects its case.
    pub name: String,
    /// Which states it may hand over to.
    pub successors: Successors,
    /// Whether the loop stops after an iteration in this state.
    pub stop: Stop,
}

/// The abstract model of one state machine: a while loop around a case
/// structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
}

/// One result of [`Machine::check`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Finding {
    /// Proven: no execution reaches this state.
    Unreachable {
        /// Name of the state.
        state: String,
    },
    /// Proven: once in this state the loop can never stop.
    CannotTerminate {
        /// Name of the state.
        state: String,
    },
    /// A state hands over to a name that has no case; the default state
    /// runs instead. Usually a misspelled or deleted state.
    UndefinedTarget {
        /// The state that writes the name.
        from: String,
        /// The name without a case.
        target: String,
        /// The default state, or `None` when there is none and the name
        /// selects nothing.
        handled_by: Option<String>,
    },
    /// Proven: the loop never stops from any starting state.
    NeverTerminates,
}

/// What [`Machine::check`] found out about a machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// The machine that was checked.
    pub machine: Machine,
    /// States the model can reach, in state order. `None` when the initial
    /// state is unknown, in which case nothing is claimed about
    /// reachability.
    pub reachable: Option<Vec<String>>,
    /// Resolved transitions `(from, to)`, sorted, for display and export.
    pub transitions: Vec<(String, String)>,
    /// Proven unreachable or non-terminating states and undefined targets,
    /// sorted.
    pub findings: Vec<Finding>,
    /// True when every successor and every stop condition was read exactly,
    /// so the model is the program's state graph and not only a bound on it.
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

    /// Successor state indices of each state in the model.
    fn edges(&self, index: &BTreeMap<&str, usize>) -> Vec<BTreeSet<usize>> {
        self.states
            .iter()
            .map(|state| match &state.successors {
                Successors::Unknown => (0..self.states.len()).collect(),
                Successors::Known(names) => names
                    .iter()
                    .filter_map(|name| self.resolve(index, name))
                    .collect(),
            })
            .collect()
    }

    /// Check the machine. The result is deterministic.
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
            while let Some(at) = queue.pop_front() {
                if std::mem::replace(&mut seen[at], true) {
                    continue;
                }
                queue.extend(edges[at].iter().copied().filter(|&next| !seen[next]));
            }
            seen
        });

        // Backward reachability from every state that may stop: the states
        // left over can never reach a stop.
        let mut can_stop = vec![false; count];
        let mut queue: VecDeque<usize> = (0..count)
            .filter(|&i| self.states[i].stop != Stop::Never)
            .collect();
        let mut reverse: Vec<Vec<usize>> = vec![Vec::new(); count];
        for (from, successors) in edges.iter().enumerate() {
            for &to in successors {
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
        for (i, state) in self.states.iter().enumerate() {
            let Successors::Known(names) = &state.successors else {
                continue;
            };
            // A dead state's typo is still worth fixing, but say nothing
            // about states that are proven never to run.
            if !live(i) {
                continue;
            }
            for name in names {
                if !index.contains_key(name.as_str()) {
                    findings.push(Finding::UndefinedTarget {
                        from: state.name.clone(),
                        target: name.clone(),
                        handled_by: self.default.clone(),
                    });
                }
            }
        }
        findings.sort();

        let mut transitions = BTreeSet::new();
        for (from, successors) in edges.iter().enumerate() {
            for &to in successors {
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
            stop,
        }
    }

    fn machine(states: Vec<State>, initial: Option<&[&str]>, default: Option<&str>) -> Machine {
        Machine {
            location: "while loop #1".to_owned(),
            states,
            initial: initial.map(|names| names.iter().map(|n| (*n).to_owned()).collect()),
            default: default.map(str::to_owned),
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
                    from: "Init".to_owned(),
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
                    from: "Init".to_owned(),
                    target: "Nowhere".to_owned(),
                    handled_by: None,
                },
                Finding::NeverTerminates,
            ]
        );
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
