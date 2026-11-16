//! State machine extraction and verification.
//!
//! Real files: `tests/fixtures/verify` (its README records sources and
//! licences). The classic enumeration machines the extraction was
//! developed against carry no licence, so they are rebuilt here as
//! diagrams with the same shape, and
//! `real_machines_outside_the_repository` checks the real files where
//! they are available.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use buzzard_lint::verify::{
    extract, verify, Confidence, Finding, Machine, Problem, State, Stop, Successors,
};
use buzzard_lint::Inspection;
use buzzard_rsrc::{BlockDiagram, DataType, NumericType, SelectorKind, TunnelMode, Value};

#[path = "verify/build.rs"]
mod build;

use build::{
    boolean, integer, label, one, range, select, text, with_node, with_terminals, Expr, Spec, Wrap,
    ADD, CASE, CONDITION, EQUAL, LEFT, LOOP, STOP,
};

// ------------------------------------------------------------- helpers

fn fixture(name: &str) -> BlockDiagram {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/verify")
        .join(name);
    Inspection::from_path(&path).unwrap().diagram.unwrap()
}

fn names(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn state(name: &str, successors: &[&str], stop: Stop) -> State {
    State {
        name: name.to_owned(),
        successors: Successors::Known(names(successors)),
        unmatched: BTreeSet::new(),
        stop,
    }
}

fn opaque(name: &str, stop: Stop) -> State {
    State {
        successors: Successors::Unknown,
        ..state(name, &[], stop)
    }
}

fn machine(states: Vec<State>, initial: Option<&[&str]>, default: Option<&str>) -> Machine {
    Machine {
        location: String::new(),
        states,
        initial: initial.map(names),
        default: default.map(str::to_owned),
    }
}

/// The one machine of a diagram, without its location.
fn only(diagram: &BlockDiagram) -> Machine {
    let found = extract(diagram);
    assert!(found.queue_machines.is_empty());
    let [machine] = &found.machines[..] else {
        panic!("expected one machine, found {}", found.machines.len());
    };
    Machine {
        location: String::new(),
        ..machine.clone()
    }
}

fn unreachable(name: &str) -> Finding {
    Finding::Unreachable {
        state: name.to_owned(),
    }
}

const NEVER: Stop = Stop::Never;
const ALWAYS: Stop = Stop::Always;
const MAYBE: Stop = Stop::Maybe;

/// Whether any finding claims that something cannot happen.
fn claims_impossibility(findings: &[Finding]) -> bool {
    findings.iter().any(|finding| {
        matches!(
            finding,
            Finding::Unreachable { .. }
                | Finding::CannotTerminate { .. }
                | Finding::NeverTerminates
        )
    })
}

// ------------------------------------- the real machines, and their twins
//
// Each pair is the machine expected from a real file and a diagram built
// to the same shape. The expectations were read off the decoded diagrams
// (`buzzard inspect --format markdown`) and agree with an independent
// reference extraction made during the format research.

/// `lallers/StateMachine`, `StateMachineP7.vi` (version 13). The stop
/// terminal is wired to a front-panel control.
fn lallers() -> (Spec, Machine) {
    let spec = Spec::enumeration(&["Acquire", "Analyze", "Alarm"]);
    let (alarm, acquire) = (spec.item("Alarm"), spec.item("Acquire"));
    let mut spec = spec
        .default(0)
        .starts_in("Acquire")
        .goes(0, "Analyze")
        .goes(1, "Acquire")
        .next(2, select(alarm, acquire));
    // The frames are not in item order: "Alarm" is the second frame.
    spec.frames[1].matches = vec![one(2)];
    spec.frames[2].matches = vec![one(1)];
    spec.loop_stop = Some(Expr::Control);
    let expected = machine(
        vec![
            state("Acquire", &["Analyze"], MAYBE),
            state("Alarm", &["Acquire"], MAYBE),
            state("Analyze", &["Acquire", "Alarm"], MAYBE),
        ],
        Some(&["Acquire"]),
        Some("Acquire"),
    );
    (spec, expected)
}

/// `AhmedDMoHassan/Simple-State-machine`, `Statemachine/Main.vi` (version
/// 25.1). The stop tunnel is wired only in "Shutdown".
fn ahmed() -> (Spec, Machine) {
    let spec = Spec::enumeration(&["Start", "Process 1", "Process 2", "End", "Shutdown"]);
    let (shutdown, again) = (spec.item("Shutdown"), spec.item("Process 1"));
    let mut spec = spec
        .default(0)
        .starts_in("Start")
        .goes(0, "Process 1")
        .goes(1, "Process 2")
        .goes(2, "End")
        .next(3, select(shutdown, again))
        .goes(4, "Shutdown")
        .stop(4, boolean(true));
    for frame in &mut spec.frames[..4] {
        frame.stop = Expr::Unwired;
    }
    let expected = machine(
        vec![
            state("Start", &["Process 1"], NEVER),
            state("Process 1", &["Process 2"], NEVER),
            state("Process 2", &["End"], NEVER),
            state("End", &["Process 1", "Shutdown"], NEVER),
            state("Shutdown", &["Shutdown"], ALWAYS),
        ],
        Some(&["Start"]),
        Some("Start"),
    );
    (spec, expected)
}

/// `frank1641/Multiple-consumer-State-machine`, `Queue State Machine
/// Lib/Main.vi` (version 20). Two states leave the next-state tunnel
/// unwired, which sends them to the first item; nothing leads to them.
fn frank() -> (Spec, Machine) {
    let spec = Spec::enumeration(&[
        "Initialize",
        "Wait for Event",
        "User State 1",
        "User State 2",
        "Exit",
    ])
    .starts_in("Initialize")
    .goes(0, "Wait for Event")
    .goes(1, "Exit")
    .next(2, Expr::Unwired)
    .next(3, Expr::Unwired)
    .goes(4, "Exit")
    .stop(4, boolean(true));
    let expected = machine(
        vec![
            state("Initialize", &["Wait for Event"], NEVER),
            state("Wait for Event", &["Exit"], NEVER),
            state("User State 1", &["Initialize"], NEVER),
            state("User State 2", &["Initialize"], NEVER),
            state("Exit", &["Exit"], ALWAYS),
        ],
        Some(&["Initialize"]),
        None,
    );
    (spec, expected)
}

/// `Gluvertski1/StateMachine_LabVIEW`, `StateMachine.vi` (version 17).
/// One state splits by event case; the loop stops on a comparison.
fn gluvertski() -> (Spec, Machine) {
    let spec = Spec::enumeration(&[
        "Initialize",
        "Stop",
        "Check for UI Events",
        "Acquire Data",
        "Analyze Data",
    ]);
    let events = Expr::Event {
        frames: vec![
            spec.item("Acquire Data"),
            spec.item("Analyze Data"),
            spec.item("Stop"),
        ],
        default_if_unwired: false,
        timeout_wired: false,
    };
    let mut spec = spec
        .starts_in("Initialize")
        .goes(0, "Check for UI Events")
        .goes(1, "Check for UI Events")
        .next(2, events)
        .goes(3, "Check for UI Events")
        .goes(4, "Check for UI Events");
    spec.loop_stop = Some(Expr::Function(EQUAL, vec![Expr::Current, Expr::Control]));
    let expected = machine(
        vec![
            state("Initialize", &["Check for UI Events"], MAYBE),
            state("Stop", &["Check for UI Events"], MAYBE),
            state(
                "Check for UI Events",
                &["Acquire Data", "Analyze Data", "Stop"],
                MAYBE,
            ),
            state("Acquire Data", &["Check for UI Events"], MAYBE),
            state("Analyze Data", &["Check for UI Events"], MAYBE),
        ],
        Some(&["Initialize"]),
        None,
    );
    (spec, expected)
}

/// `GaborDernoczi/LabVIEW_State_Machine`, `State_Machine.vi` (version
/// 20). The waiting state holds an event structure whose first two cases
/// leave the next state unwired (the first item, the waiting state
/// itself) and whose stop tunnel is unwired, True and False.
fn gabor() -> (Spec, Machine) {
    let spec = Spec::enumeration(&[
        "Waiting for Event",
        "Initialization",
        "Calculation",
        "Representation",
    ]);
    let next = Expr::Event {
        frames: vec![Expr::Unwired, Expr::Unwired, spec.item("Initialization")],
        default_if_unwired: true,
        timeout_wired: false,
    };
    let stop = Expr::Event {
        frames: vec![Expr::Unwired, boolean(true), boolean(false)],
        default_if_unwired: true,
        timeout_wired: false,
    };
    let spec = spec
        .default(1)
        .starts_in("Waiting for Event")
        .next(0, next)
        .stop(0, stop)
        .goes(1, "Calculation")
        .goes(2, "Representation")
        .goes(3, "Waiting for Event");
    let expected = machine(
        vec![
            state(
                "Waiting for Event",
                &["Initialization", "Waiting for Event"],
                MAYBE,
            ),
            state("Initialization", &["Calculation"], NEVER),
            state("Calculation", &["Representation"], NEVER),
            state("Representation", &["Waiting for Event"], NEVER),
        ],
        Some(&["Waiting for Event"]),
        Some("Initialization"),
    );
    (spec, expected)
}

/// `TomsLabVIEWAdventure/Guessing-Game`, `Main.vi` (version 13).
fn toms() -> (Spec, Machine) {
    let spec = Spec::enumeration(&[
        "Initialise",
        "Make Guess",
        "Wait for event",
        "Sucess",
        "Fail",
        "End",
    ]);
    let item = |name: &str| spec.item(name);
    let events = Expr::Event {
        frames: vec![item("Make Guess"), item("End")],
        default_if_unwired: false,
        timeout_wired: false,
    };
    let (retry, verdict) = (
        select(item("End"), item("Make Guess")),
        select(item("Sucess"), item("Fail")),
    );
    let mut spec = spec
        .default(0)
        .starts_in("Initialise")
        .goes(0, "Wait for event")
        .next(1, events)
        .goes(2, "End")
        .next(3, retry)
        .goes(4, "End")
        .next(5, verdict)
        .stop(4, boolean(true));
    // Frame order in the file: items 0, 2, 3, 4, 5, 1.
    for (frame, value) in spec.frames.iter_mut().zip([0, 2, 3, 4, 5, 1]) {
        frame.matches = vec![one(value)];
    }
    for index in [0, 1, 2, 3, 5] {
        spec.frames[index].stop = Expr::Unwired;
    }
    let expected = machine(
        vec![
            state("Initialise", &["Wait for event"], NEVER),
            state("Wait for event", &["End", "Make Guess"], NEVER),
            state("Sucess", &["End"], NEVER),
            state("Fail", &["End", "Make Guess"], NEVER),
            state("End", &["End"], ALWAYS),
            state("Make Guess", &["Fail", "Sucess"], NEVER),
        ],
        Some(&["Initialise"]),
        Some("Initialise"),
    );
    (spec, expected)
}

/// `CandleScience/Labview-State-Machine-Template`,
/// `SimpleStateMachineTemplate.vi` (version 20). The case structure sits
/// in the timeout case of an event structure whose other cases write the
/// next state themselves; six items, four frames.
fn candle() -> (Spec, Machine) {
    let spec = Spec::enumeration(&["Cooling", "Heating", "Start", "Stop", "Idle", "Init"]);
    let item = |name: &str| spec.item(name);
    let others = vec![
        (item("Idle"), boolean(true)),
        (item("Start"), Expr::Unwired),
        (item("Stop"), Expr::Unwired),
    ];
    let mut spec = spec.default(0).starts_in("Init");
    spec.frames.truncate(4);
    let frames = [(4, "Idle"), (3, "Idle"), (2, "Start"), (5, "Idle")];
    for (frame, (value, next)) in spec.frames.iter_mut().zip(frames) {
        frame.matches = vec![one(value)];
        frame.stop = Expr::Unwired;
        frame.next = Expr::Const(Value::Enum {
            value: ["Cooling", "Heating", "Start", "Stop", "Idle", "Init"]
                .iter()
                .position(|item| *item == next)
                .unwrap() as u32,
            name: Some(next.to_owned()),
        });
    }
    spec.wrap = Some(Wrap {
        others,
        timeout_wired: true,
    });
    let all = &["Idle", "Start", "Stop"];
    let expected = machine(
        vec![
            state("Idle", all, MAYBE),
            state("Stop", all, MAYBE),
            state("Start", all, MAYBE),
            state("Init", all, MAYBE),
        ],
        Some(&["Init"]),
        Some("Idle"),
    );
    (spec, expected)
}

type Twin = fn() -> (Spec, Machine);

/// Where each real machine is, relative to a directory of clones named
/// `<owner>_<repository>`, and the location it is expected at.
const REAL: [(&str, &str, Twin); 8] = [
    (
        "lallers_StateMachine/StateMachineP7.vi",
        "while loop #38, case structure #49",
        lallers,
    ),
    (
        "lallers_StateMachine/StatemMachineLV7/Main.vi",
        "while loop #38, case structure #49",
        lallers,
    ),
    (
        "AhmedDMoHassan_Simple-State-machine/Statemachine/Main.vi",
        "while loop #160, case structure #193",
        ahmed,
    ),
    (
        "frank1641_Multiple-consumer-State-machine/Queue State Machine Lib/Main.vi",
        "while loop #38, case structure #49",
        frank,
    ),
    (
        "Gluvertski1_StateMachine_LabVIEW/StateMachine.vi",
        "while loop #637, case structure #684",
        gluvertski,
    ),
    (
        "GaborDernoczi_LabVIEW_State_Machine/State_Machine.vi",
        "while loop #70, case structure #117",
        gabor,
    ),
    (
        "TomsLabVIEWAdventure_Guessing-Game/Main.vi",
        "while loop #601, case structure #627",
        toms,
    ),
    (
        "CandleScience_Labview-State-Machine-Template/SimpleStateMachineTemplate.vi",
        "while loop #105, case structure #217",
        candle,
    ),
];

#[test]
fn twins_of_the_real_machines_extract_exactly() {
    for twin in [lallers, ahmed, frank, gluvertski, gabor, toms, candle] {
        let (spec, expected) = twin();
        let diagram = spec.build();
        assert_eq!(only(&diagram), expected);
        // The result does not depend on the run.
        assert_eq!(extract(&diagram), extract(&diagram));
    }
    // What the checks make of them: only the machine with two orphaned
    // states has findings, and they are proofs.
    let findings = |twin: Twin| twin().1.check().findings;
    for clean in [lallers, ahmed, gluvertski, gabor, toms, candle] {
        assert_eq!(findings(clean), []);
    }
    assert_eq!(
        findings(frank),
        [unreachable("User State 1"), unreachable("User State 2")]
    );
    assert!(ahmed().1.check().exact && toms().1.check().exact);
    assert!(!lallers().1.check().exact && !candle().1.check().exact);
}

/// The seven classic enumeration machines the extraction was developed
/// against have no licence and are not in the repository. Clone them (the
/// fixture README lists them) into one directory and run
/// `BUZZARD_FSM_SAMPLES=<dir> cargo test -p buzzard-lint --test verify -- --ignored`.
#[test]
#[ignore = "needs BUZZARD_FSM_SAMPLES to point at clones of the sample repositories"]
fn real_machines_outside_the_repository() {
    let root = PathBuf::from(
        std::env::var_os("BUZZARD_FSM_SAMPLES").expect("BUZZARD_FSM_SAMPLES is not set"),
    );
    for (path, location, twin) in REAL {
        let file = root.join(path);
        let diagram = Inspection::from_path(&file)
            .unwrap_or_else(|e| panic!("{}: {e}", file.display()))
            .diagram
            .unwrap();
        let found = extract(&diagram);
        assert!(found.queue_machines.is_empty(), "{path}");
        let expected = Machine {
            location: location.to_owned(),
            ..twin().1
        };
        assert_eq!(found.machines, [expected], "{path}");
    }
}

/// The one real classic machine that may be redistributed: a string
/// machine with the cases `"Init"` and `"Idle", Default` (version 13).
///
/// "Init" leaves the next-state tunnel unwired, which is the empty string
/// and so the default case "Idle". "Idle" holds an event structure with a
/// wired timeout whose cases leave the tunnel unwired or, through nested
/// enumeration case structures, write "Init" or "Idle". The stop tunnel
/// is unwired in "Init" and reaches a compound arithmetic node in "Idle",
/// but the diagram also calls the built-in Stop function, so no state is
/// claimed to keep the loop running.
#[test]
fn real_string_machine_extracts_exactly() {
    let diagram = fixture("jki-smo/process.vi");
    let found = extract(&diagram);
    assert!(found.queue_machines.is_empty());
    let expected = Machine {
        location: "while loop #3379, case structure #3023".to_owned(),
        ..machine(
            vec![
                state("Init", &["Idle"], MAYBE),
                state("Idle", &["Idle", "Init"], MAYBE),
            ],
            Some(&["Init"]),
            Some("Idle"),
        )
    };
    assert_eq!(found.machines, [expected]);
    let checked = verify(&diagram);
    assert_eq!(checked.issues(), []);
    assert!(!checked.machines[0].exact);
    assert_eq!(
        checked.machines[0].transitions,
        [("Idle", "Idle"), ("Idle", "Init"), ("Init", "Idle")]
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
    );
}

// ------------------------------------------------- string/queue machines

#[test]
fn queue_machines_are_recognised_and_checked_heuristically() {
    // The library template: every enqueued name has a case.
    let template = verify(&fixture(
        "jki-state-machine/queued-state-machine-template.vi",
    ));
    assert!(template.machines.is_empty());
    let [queue] = &template.queue_machines[..] else {
        panic!("expected one queue machine");
    };
    assert_eq!(queue.location, "while loop #117, case structure #5");
    assert_eq!(queue.default, "Default");
    assert_eq!(queue.states.len(), 21);
    assert_eq!(
        queue.enqueued,
        [
            "Data: Cleanup",
            "Data: Initialize",
            "Exit",
            "Initialize Core Data",
            "Macro: Exit",
            "Macro: Initialize",
            "UI: Front Panel State",
            "UI: Initialize",
            "UI: Uninitialize",
        ]
    );
    assert_eq!(queue.unhandled, [] as [&str; 0]);
    assert_eq!(template.issues(), []);

    // A documentation example enqueues three names that have no case. The
    // case structures on the state's arguments, nested in the one on the
    // state, are not machines of their own.
    let example = verify(&fixture("jki-state-machine/new-support-vis-examples.vi"));
    assert!(example.machines.is_empty());
    assert_eq!(example.queue_machines.len(), 1);
    let issues = example.issues();
    let unhandled: Vec<&str> = issues
        .iter()
        .map(|issue| match &issue.problem {
            Problem::UnhandledQueueState { name, handled_by } => {
                assert_eq!(handled_by, "Default");
                name.as_str()
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        unhandled,
        [
            "Action: Clean up after doing work",
            "Action: Step 1",
            "Action: Step 2"
        ]
    );
    for issue in &issues {
        assert_eq!(issue.location, "while loop #70, case structure #177");
        assert_eq!(issue.code(), "BZ203");
        assert_eq!(issue.confidence(), Confidence::Heuristic);
    }
}

#[test]
fn queue_strings_are_split_as_the_real_files_use_them() {
    use buzzard_lint::verify::extract::queued_names;
    // Lines, arguments after `>>`, and both comment forms, as found in the
    // real machines.
    assert_eq!(
        queued_names("Data: Initialize\nInitialize Core Data\nUI: Front Panel State >> Open"),
        [
            "Data: Initialize",
            "Initialize Core Data",
            "UI: Front Panel State"
        ]
    );
    assert_eq!(
        queued_names("Action: Step 1\r\nEvent Structure // check the Abort button\n\n  Exit  "),
        ["Action: Step 1", "Event Structure", "Exit"]
    );
    assert_eq!(queued_names("//Macro: Exit"), [] as [&str; 0]);
    assert_eq!(
        queued_names("//Test: Start //Only for development."),
        [] as [&str; 0]
    );
    assert_eq!(queued_names("/* Post Error States"), [] as [&str; 0]);
    assert_eq!(queued_names("A /* x\ny */ B\nC"), ["A  B", "C"]);
    assert_eq!(queued_names("Open >> http://example.test/a"), ["Open"]);
    assert_eq!(queued_names(""), [] as [&str; 0]);
}

// ------------------------------------------------------ not state machines

#[test]
fn real_loops_with_case_structures_are_not_machines() {
    // An error cluster from a shift register selects the case, and the
    // loop counter does.
    for name in [
        "lv-mqtt-broker/test-simple-connection.vi",
        "jki-state-machine/parse-state-queue-performance-test.vi",
    ] {
        let diagram = fixture(name);
        let loops = diagram.nodes().filter(|n| n.class == "whileLoop").count();
        let cases = diagram.nodes().filter(|n| n.class == "select").count();
        assert!(loops >= 1 && cases >= 1, "{name}");
        assert!(verify(&diagram).is_empty(), "{name}");
    }
}

/// A healthy three-state machine to derive the negative cases from.
fn base() -> Spec {
    Spec::enumeration(&["Init", "Run", "Exit"])
        .starts_in("Init")
        .goes(0, "Run")
        .goes(1, "Exit")
        .stop(2, boolean(true))
}

#[test]
fn the_base_machine_is_found() {
    let expected = machine(
        vec![
            state("Init", &["Run"], NEVER),
            state("Run", &["Exit"], NEVER),
            state("Exit", &["Exit"], ALWAYS),
        ],
        Some(&["Init"]),
        None,
    );
    assert_eq!(only(&base().build()), expected);
    let found = extract(&base().build());
    assert_eq!(
        found.machines[0].location,
        format!("while loop #{LOOP}, case structure #{CASE}")
    );
}

#[test]
fn other_selectors_and_loops_are_not_machines() {
    let none = |diagram: &BlockDiagram| assert!(verify(diagram).is_empty());

    // A Boolean and an error cluster selector, even from a register.
    for kind in [SelectorKind::Boolean, SelectorKind::ErrorCluster] {
        let mut diagram = base().build();
        with_node(&mut diagram, CASE, |case| {
            case.selector.as_mut().unwrap().kind = kind;
        });
        none(&diagram);
    }
    // A selector table that was not decoded.
    let mut diagram = base().build();
    with_node(&mut diagram, CASE, |case| case.selector = None);
    none(&diagram);
    // A for loop, and a loop class the decoder gives no loop facts for.
    for class in ["forLoop", "timedLoop"] {
        let mut diagram = base().build();
        with_node(&mut diagram, LOOP, |looping| {
            looping.class = class.to_owned()
        });
        none(&diagram);
    }
    // The selector is not the register: its wire comes from a subVI, a
    // function, a control or the loop counter instead.
    for class in ["iUseDCO", "parm", "fPDCO", "lCnt"] {
        let mut diagram = base().build();
        let inner = |t: &buzzard_rsrc::Terminal| {
            t.dco == Some(LEFT) && t.direction == buzzard_rsrc::Direction::Output
        };
        let changed = with_terminals(&mut diagram, inner, |t| {
            t.dco_class = Some(class.to_owned());
        });
        assert_eq!(changed, 1);
        none(&diagram);
    }
    // A stacked register: two left terminals.
    let mut diagram = base().build();
    with_node(&mut diagram, LOOP, |looping| {
        let info = looping.loop_info.as_mut().unwrap();
        info.shift_registers[0].left.push(77);
    });
    none(&diagram);
    // The loop does not list the register at all.
    let mut diagram = base().build();
    with_node(&mut diagram, LOOP, |looping| {
        looping.loop_info.as_mut().unwrap().shift_registers.clear();
    });
    none(&diagram);
    // A case structure with a single frame.
    let mut spec = base();
    spec.frames.truncate(1);
    none(&spec.default(0).build());
    // The selector passes through an indexing tunnel on its way in.
    let mut diagram = base().build();
    let state_in =
        |t: &buzzard_rsrc::Terminal| t.dco_class.as_deref() == Some("caseSel") && t.wired;
    assert_eq!(
        with_terminals(&mut diagram, state_in, |t| t.wired = false),
        1
    );
    diagram.root.nodes[1].diagrams[0].wires.remove(0);
    none(&diagram);
}

#[test]
fn an_integer_register_that_is_only_computed_is_not_a_machine() {
    // A counter: every frame adds to the register.
    let count = Expr::Function(ADD, vec![Expr::Current, integer(1)]);
    let counter = Spec::integers(vec![vec![one(0)], vec![], vec![one(1)]])
        .default(1)
        .starts(integer(0))
        .next(0, count.clone())
        .next(1, count.clone())
        .next(2, count);
    assert!(verify(&counter.clone().build()).is_empty());
    // One frame that writes a constant makes it a machine.
    let expected = machine(
        vec![
            state("0", &["1"], NEVER),
            opaque("Default", NEVER),
            opaque("1", NEVER),
        ],
        Some(&["0"]),
        Some("Default"),
    );
    assert_eq!(only(&counter.next(0, integer(1)).build()), expected);
}

// ----------------------------------------------------------- soundness

/// A machine in which every "cannot happen" finding fires when the
/// diagram is read exactly: "Orphan" and "Exit" are unreachable and the
/// rest can never stop.
fn doomed() -> Spec {
    Spec::enumeration(&["Init", "Run", "Orphan", "Exit"])
        .starts_in("Init")
        .goes(0, "Run")
        .goes(2, "Exit")
        .stop(3, boolean(true))
}

#[test]
fn exact_reading_proves_what_is_wrong() {
    let report = only(&doomed().build()).check();
    assert!(report.exact);
    assert_eq!(
        report.findings,
        [
            unreachable("Exit"),
            unreachable("Orphan"),
            Finding::NeverTerminates
        ]
    );
}

#[test]
fn an_unknowable_next_state_is_unknown_and_claims_nothing() {
    let item = |name: &str| doomed().item(name);
    let unknowable: Vec<(&str, Expr)> = vec![
        ("a subVI", Expr::Call),
        ("arithmetic", Expr::Function(ADD, vec![Expr::Current])),
        ("a control", Expr::Control),
        (
            "Select with a subVI on one side",
            select(item("Run"), Expr::Call),
        ),
        (
            "Select with an unwired input",
            select(item("Run"), Expr::Unwired),
        ),
        (
            "a case structure with a subVI in one frame",
            Expr::Case {
                frames: vec![item("Run"), Expr::Call],
                default_if_unwired: false,
            },
        ),
        (
            "an unwired frame without use-default",
            Expr::Case {
                frames: vec![item("Run"), Expr::Unwired],
                default_if_unwired: false,
            },
        ),
        ("a for loop", Expr::For(Box::new(item("Run")))),
        (
            "an indexing tunnel",
            Expr::AutoIndexed(Box::new(item("Run"))),
        ),
        (
            "a sequence with two writers",
            Expr::Sequence(vec![item("Run"), item("Exit")]),
        ),
        (
            "a constant of another numeric type",
            Expr::Typed(
                Value::Enum {
                    value: 1,
                    name: Some("Run".to_owned()),
                },
                NumericType::Enum8,
            ),
        ),
        (
            "an integer where an enumeration is expected",
            Expr::Typed(Value::Integer(1), NumericType::Enum16),
        ),
        (
            "an item whose name differs from the selector's",
            Expr::Const(Value::Enum {
                value: 1,
                name: Some("Walk".to_owned()),
            }),
        ),
        (
            "a value outside the item list",
            Expr::Const(Value::Enum {
                value: 9,
                name: None,
            }),
        ),
        ("an unwired next-state terminal", Expr::Unwired),
    ];
    for (what, next) in unknowable {
        let mut spec = doomed().next(1, next);
        if matches!(spec.frames[1].next, Expr::Unwired) {
            // Unwired at the top level: feed the register from nothing.
            spec.loop_next = Some(Expr::Unwired);
        }
        let found = only(&spec.build());
        let unknown = if spec.loop_next.is_some() { 4 } else { 1 };
        let count = found
            .states
            .iter()
            .filter(|s| s.successors == Successors::Unknown)
            .count();
        assert_eq!(count, unknown, "{what}");
        assert_eq!(found.states[1].successors, Successors::Unknown, "{what}");
        let report = found.check();
        assert!(!report.exact, "{what}");
        assert!(!claims_impossibility(&report.findings), "{what}");
    }
}

#[test]
fn a_wire_that_cannot_be_trusted_is_unknown() {
    let run_is_unknown = |diagram: &BlockDiagram, what: &str| {
        let found = only(diagram);
        assert_eq!(found.states[1].successors, Successors::Unknown, "{what}");
        assert!(!claims_impossibility(&found.check().findings), "{what}");
    };
    fn frame(diagram: &mut BlockDiagram) -> &mut buzzard_rsrc::Diagram {
        let case = &mut diagram.root.nodes[1].diagrams[0].nodes[1];
        assert_eq!(case.uid, CASE);
        &mut case.diagrams[1]
    }
    let spec = doomed().goes(1, "Run");
    // The next-state constant of the frame.
    fn is_item(terminal: &buzzard_rsrc::Terminal) -> bool {
        matches!(terminal.constant, Some(Value::Enum { .. }))
    }

    // Listed backwards: the constant appears as the sink.
    let mut diagram = spec.build();
    let wire = &mut frame(&mut diagram).wires[0];
    std::mem::swap(&mut wire.source, &mut wire.sinks[0]);
    run_is_unknown(&diagram, "reversed wire");

    // Two wires into one terminal.
    let mut diagram = spec.build();
    let inner = frame(&mut diagram);
    let mut second = inner.wires[0].clone();
    second.source = inner.wires[1].source;
    inner.wires.push(second);
    run_is_unknown(&diagram, "two sources");

    // The source terminal exists twice.
    let mut diagram = spec.build();
    let inner = frame(&mut diagram);
    let copy = inner.nodes[0].terminals.iter().find(|t| is_item(t));
    let copy = copy.unwrap().clone();
    inner.nodes[0].terminals.push(copy);
    run_is_unknown(&diagram, "duplicate id");

    // The wire's source is not a terminal of the diagram.
    let mut diagram = spec.build();
    frame(&mut diagram).wires[0].source = 123_456;
    run_is_unknown(&diagram, "dangling source");

    // The constant's value was not decoded.
    let mut diagram = spec.build();
    let terminals = &mut frame(&mut diagram).nodes[0].terminals;
    terminals.iter_mut().find(|t| is_item(t)).unwrap().constant = None;
    run_is_unknown(&diagram, "undecoded constant");

    // The tunnel out of the case structure is of a kind that changes
    // values, or its two sides disagree.
    for change in [
        (|t: &mut buzzard_rsrc::Terminal| t.tunnel.as_mut().unwrap().mode = TunnelMode::Other)
            as fn(&mut buzzard_rsrc::Terminal),
        |t| t.tunnel.as_mut().unwrap().conditional = true,
        |t| t.tunnel.as_mut().unwrap().output = false,
        |t| t.tunnel = None,
    ] {
        let mut diagram = spec.build();
        with_node(&mut diagram, CASE, |case| change(&mut case.terminals[2]));
        let found = only(&diagram);
        assert!(found
            .states
            .iter()
            .all(|s| s.successors == Successors::Unknown));
        assert!(!claims_impossibility(&found.check().findings));
    }
}

#[test]
fn a_selector_and_register_of_different_types_resolve_nothing() {
    // The register is a 32-bit integer and the selector an enumeration: a
    // conversion sits between them, so written values are not resolved
    // and the initial state is unknown.
    let mut diagram = doomed().build();
    let register =
        |t: &buzzard_rsrc::Terminal| matches!(t.dco_class.as_deref(), Some("lSR" | "rSR"));
    with_terminals(&mut diagram, register, |t| {
        t.numeric = Some(NumericType::I32);
    });
    let found = only(&diagram);
    assert!(found
        .states
        .iter()
        .all(|s| s.successors == Successors::Unknown));
    assert_eq!(found.initial, None);
    assert_eq!(found.check().findings, []);

    // A 64-bit selector, and an unsigned one whose table holds a negative
    // number, are not resolved either.
    let mut diagram = doomed().build();
    with_terminals(
        &mut diagram,
        |t| t.numeric == Some(NumericType::Enum16),
        |t| t.numeric = Some(NumericType::U64),
    );
    with_node(&mut diagram, CASE, |case| {
        case.selector.as_mut().unwrap().kind = SelectorKind::Integer;
    });
    assert!(verify(&diagram).is_empty());

    let wide = Spec {
        state: build::Ty {
            data: DataType::Numeric,
            numeric: Some(NumericType::U32),
        },
        ..Spec::integers(vec![vec![one(0)], vec![one(-1)]])
    }
    .starts(integer(0))
    .next(0, integer(0));
    assert!(verify(&wide.build()).is_empty());
}

#[test]
fn a_stop_that_is_not_a_constant_is_maybe() {
    let unknowable: Vec<(&str, Expr)> = vec![
        ("a control", Expr::Control),
        ("a subVI", Expr::Call),
        ("a comparison", Expr::Function(EQUAL, vec![Expr::Current])),
        ("either constant", select(boolean(true), boolean(false))),
        (
            "an unwired frame without use-default",
            Expr::Case {
                frames: vec![boolean(false), Expr::Unwired],
                default_if_unwired: false,
            },
        ),
    ];
    for (what, stop) in unknowable {
        // In one frame only.
        let found = only(&doomed().stop(1, stop.clone()).build());
        assert_eq!(found.states[1].stop, MAYBE, "{what}");
        assert_eq!(found.states[0].stop, NEVER, "{what}");
        let findings = found.check().findings;
        assert_eq!(
            findings,
            [unreachable("Exit"), unreachable("Orphan")],
            "{what}"
        );
        // For the whole loop.
        let mut spec = doomed();
        spec.loop_stop = Some(stop);
        let found = only(&spec.build());
        assert!(found.states.iter().all(|s| s.stop == MAYBE), "{what}");
    }
    // The condition terminal is not a Boolean (an error cluster stops the
    // loop on an error), is missing, or is not wired at all.
    let mut diagram = doomed().build();
    with_terminals(
        &mut diagram,
        |t| t.dco == Some(CONDITION),
        |t| t.data_type = Some(DataType::ErrorCluster),
    );
    assert!(only(&diagram).states.iter().all(|s| s.stop == MAYBE));
    let mut diagram = doomed().build();
    with_node(&mut diagram, LOOP, |looping| {
        looping.loop_info.as_mut().unwrap().stop = None;
    });
    assert!(only(&diagram).states.iter().all(|s| s.stop == MAYBE));
    let mut spec = doomed();
    spec.loop_stop = Some(Expr::Unwired);
    assert!(only(&spec.build()).states.iter().all(|s| s.stop == MAYBE));
}

#[test]
fn a_stop_function_anywhere_makes_every_stop_maybe() {
    let mut diagram = doomed().build();
    diagram.root.nodes.push(buzzard_rsrc::Node {
        uid: 70_000,
        class: "prim".to_owned(),
        primitive: Some(STOP),
        ..buzzard_rsrc::Node::default()
    });
    let found = only(&diagram);
    assert!(found.states.iter().all(|s| s.stop == MAYBE));
    assert_eq!(
        found.check().findings,
        [unreachable("Exit"), unreachable("Orphan")]
    );
}

#[test]
fn stop_polarity_is_respected() {
    // "Continue if True": a True constant keeps the loop running.
    let mut spec = base()
        .stop(0, boolean(true))
        .stop(1, boolean(true))
        .stop(2, boolean(false));
    spec.stop_if_true = false;
    let found = only(&spec.build());
    let stops: Vec<Stop> = found.states.iter().map(|s| s.stop).collect();
    assert_eq!(stops, [NEVER, NEVER, ALWAYS]);
    assert_eq!(found.check().findings, []);

    // An unwired stop tunnel is False, which stops a "continue if true"
    // loop and keeps a "stop if true" loop running.
    let unwired = |stop_if_true: bool| {
        let mut spec = base().stop(0, Expr::Unwired);
        spec.stop_if_true = stop_if_true;
        only(&spec.build()).states[0].stop
    };
    assert_eq!(unwired(true), NEVER);
    assert_eq!(unwired(false), ALWAYS);
}

#[test]
fn an_uninitialised_register_has_no_known_first_state() {
    // The register keeps its value between calls of the VI.
    let found = only(&doomed().starts(Expr::Unwired).build());
    assert_eq!(found.initial, None);
    let report = found.check();
    assert_eq!(report.reachable, None);
    // Nothing is claimed about reachability; that no state can stop is
    // true wherever the loop starts... except in "Exit" itself, so only
    // the states that cannot reach it are reported.
    assert_eq!(
        report.findings,
        [
            Finding::CannotTerminate {
                state: "Init".to_owned()
            },
            Finding::CannotTerminate {
                state: "Run".to_owned()
            },
        ]
    );
    // An initial value that is not a constant is unknown as well.
    for initial in [Expr::Control, Expr::Call] {
        assert_eq!(only(&doomed().starts(initial).build()).initial, None);
    }
    // One of two constants: both are possible first states.
    let either = select(doomed().item("Init"), doomed().item("Orphan"));
    let found = only(&doomed().starts(either).build());
    assert_eq!(found.initial, Some(names(&["Init", "Orphan"])));
    assert_eq!(
        found.check().findings,
        [
            Finding::CannotTerminate {
                state: "Init".to_owned()
            },
            Finding::CannotTerminate {
                state: "Run".to_owned()
            }
        ]
    );
}

// ------------------------------------------------ values, ranges and names

#[test]
fn a_frame_with_several_values_is_one_state() {
    // Frames: `..-1`, `0`, `1, 3..5`, `6..` and a default for 2.
    let spec = Spec::integers(vec![
        vec![range(None, Some(-1))],
        vec![one(0)],
        vec![one(1), range(Some(3), Some(5))],
        vec![range(Some(6), None)],
        vec![],
    ])
    .default(4)
    .starts(integer(0))
    .next(0, integer(-7)) // stays in `..-1`
    .next(1, select(integer(4), integer(1))) // both in `1, 3..5`
    .next(2, integer(2)) // no frame lists 2: the default
    .next(3, integer(1_000_000))
    .next(4, select(integer(i64::from(i32::MIN)), integer(6)));
    let expected = machine(
        vec![
            state("..-1", &["..-1"], NEVER),
            state("0", &["1, 3..5"], NEVER),
            state("1, 3..5", &["Default"], NEVER),
            state("6..", &["6.."], NEVER),
            state("Default", &["..-1", "6.."], NEVER),
        ],
        Some(&["0"]),
        Some("Default"),
    );
    assert_eq!(only(&spec.build()), expected);

    // A value beyond 32 bits is not resolved.
    let found = only(&spec.clone().next(3, integer(1 << 40)).build());
    assert_eq!(found.states[3].successors, Successors::Unknown);
    // Without a default frame, a value no frame lists has no case at all,
    // which a runnable diagram cannot have: unknown.
    let mut partial = spec;
    partial.default = None;
    partial.frames[4].matches = vec![one(99)];
    let found = only(&partial.build());
    assert_eq!(found.states[2].successors, Successors::Unknown);
    assert_eq!(found.states[4].name, "99");
}

#[test]
fn enumeration_values_resolve_by_value_not_by_label() {
    // Frames: `"A", "C"`, then `"B".."B"`... as a range `"D".."E"`, then
    // a default that also lists "B".
    let mut spec = Spec::enumeration(&["A", "B", "C", "D", "E", "F"]).default(2);
    spec.frames.truncate(3);
    spec.frames[0].matches = vec![one(0), one(2)];
    spec.frames[1].matches = vec![range(Some(3), Some(4))];
    spec.frames[2].matches = vec![one(1)];
    let spec = spec
        .starts_in("C")
        .goes(0, "E")
        .goes(1, "F") // no frame lists "F": the default
        .goes(2, "A");
    let expected = machine(
        vec![
            state("A, C", &["D..E"], NEVER),
            state("D..E", &["B"], NEVER),
            state("B", &["A, C"], NEVER),
        ],
        Some(&["A, C"]),
        Some("B"),
    );
    assert_eq!(only(&spec.build()), expected);
    assert_eq!(
        only(&spec.build()).check().findings,
        [Finding::NeverTerminates]
    );
}

#[test]
fn frames_with_the_same_label_stay_separate_states() {
    let mut spec = Spec::strings(&["Run", "Run", "Stop"]).default(2);
    spec.frames[1].matches = vec![label("Run"), label("Go")];
    spec.frames[0].matches = vec![label("Run, Go")];
    let found = only(&spec.build());
    let labels: Vec<&str> = found.states.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(labels, ["Run, Go (frame 0)", "Run, Go (frame 1)", "Stop"]);
}

#[test]
fn strings_match_exactly_and_fall_to_the_default() {
    let spec = Spec::strings(&["Init", "Run", "Idle"])
        .default(2)
        .starts(text("Init"))
        .next(0, text("Run"))
        .next(1, select(text("Rnu"), text("")))
        .next(2, Expr::Unwired);
    let found = only(&spec.build());
    let mut run = state("Run", &["Idle"], NEVER);
    run.unmatched = names(&["Rnu"]);
    let expected = machine(
        vec![
            state("Init", &["Run"], NEVER),
            run,
            // Unwired is the empty string, which no case lists.
            state("Idle", &["Idle"], NEVER),
        ],
        Some(&["Init"]),
        Some("Idle"),
    );
    assert_eq!(found, expected);
    let report = found.check();
    assert_eq!(
        report.findings,
        [
            Finding::UndefinedTarget {
                from: "Run".to_owned(),
                target: "Rnu".to_owned(),
                handled_by: Some("Idle".to_owned()),
            },
            Finding::NeverTerminates,
        ]
    );
    assert!(report
        .transitions
        .contains(&("Run".to_owned(), "Idle".to_owned())));
}

#[test]
fn an_unmatched_string_is_never_mistaken_for_a_state_name() {
    // The state of the first frame is named `Run, Go`; a constant with
    // that very text matches neither label and runs the default case.
    let mut spec = Spec::strings(&["Run", "Idle"]).default(1);
    spec.frames[0].matches = vec![label("Run"), label("Go")];
    let found = only(&spec.starts(text("Go")).next(1, text("Run, Go")).build());
    assert_eq!(found.states[0].name, "Run, Go");
    assert_eq!(found.states[1].successors, Successors::Known(names(&[])));
    assert_eq!(found.states[1].unmatched, names(&["Run, Go"]));
    assert_eq!(found.initial, Some(names(&["Run, Go"])));
    assert_eq!(
        found.check().transitions,
        [
            ("Idle".to_owned(), "Idle".to_owned()),
            ("Run, Go".to_owned(), "Run, Go".to_owned())
        ]
    );
}

#[test]
fn letter_case_and_other_alphabets_are_handled_conservatively() {
    let spec = Spec::strings(&["Init", "Run", "Idle"])
        .default(2)
        .starts(text("Init"));
    // Whether the case structure ignores case is not read: "run" selects
    // "Run" or the default, and is not reported as undefined.
    let found = only(&spec.clone().next(0, text("run")).build());
    assert_eq!(found.states[0], state("Init", &["Idle", "Run"], NEVER));
    assert_eq!(found.check().findings, [Finding::NeverTerminates]);
    // A string that is not ASCII is not compared at all.
    let found = only(&spec.clone().next(0, text("Rün")).build());
    assert_eq!(found.states[0].successors, Successors::Unknown);
    // Without a default case an unlisted string has nowhere to go.
    let mut strict = spec.next(0, text("Rnu"));
    strict.default = None;
    assert_eq!(
        only(&strict.build()).states[0].successors,
        Successors::Unknown
    );
}

// ------------------------------------------------------------ structures

#[test]
fn values_are_followed_through_structures() {
    let item = |name: &str| doomed().item(name);
    let next = |expr: Expr| only(&doomed().next(1, expr).build()).states[1].clone();
    let goes = |targets: &[&str]| state("Run", targets, NEVER);

    // A nested case structure contributes every frame; an unwired frame
    // with "use default" is the first item.
    let nested = Expr::Case {
        frames: vec![item("Exit"), Expr::Unwired, Expr::Current],
        default_if_unwired: true,
    };
    assert_eq!(next(nested), goes(&["Exit", "Init", "Run"]));
    // An event structure that waits forever runs exactly one frame.
    let waiting = |timeout_wired| Expr::Event {
        frames: vec![item("Exit"), item("Orphan")],
        default_if_unwired: false,
        timeout_wired,
    };
    assert_eq!(next(waiting(false)), goes(&["Exit", "Orphan"]));
    // With a wired timeout it may run none, leaving the type's default.
    assert_eq!(next(waiting(true)), goes(&["Exit", "Init", "Orphan"]));
    // A disable structure: whichever frame is enabled.
    let disabled = Expr::Disable(vec![item("Exit"), item("Orphan")]);
    assert_eq!(next(disabled), goes(&["Exit", "Orphan"]));
    // A stacked sequence: the one frame that writes the tunnel.
    let sequence = Expr::Sequence(vec![Expr::Unwired, item("Exit"), Expr::Unwired]);
    assert_eq!(next(sequence), goes(&["Exit"]));
    // A nested while loop runs at least once.
    assert_eq!(next(Expr::While(Box::new(item("Exit")))), goes(&["Exit"]));
    // Holding the state through a nested structure is the state itself.
    let held = Expr::Case {
        frames: vec![Expr::Current, select(Expr::Current, item("Exit"))],
        default_if_unwired: false,
    };
    assert_eq!(next(held), goes(&["Exit", "Run"]));
}

#[test]
fn the_case_structure_may_sit_inside_other_structures() {
    let (spec, expected) = candle();
    let diagram = spec.build();
    assert_eq!(only(&diagram), expected);
    // Without the wired timeout the default no longer leaks in, which
    // changes nothing here: "Cooling" has no frame and runs "Idle".
    let mut waiting = spec.clone();
    waiting.wrap.as_mut().unwrap().timeout_wired = false;
    assert_eq!(only(&waiting.build()), expected);
    // An event case that hands the state to a subVI makes every state's
    // successor unknown, since any state can be current when it runs.
    let mut opaque_event = spec;
    opaque_event.wrap.as_mut().unwrap().others[0].0 = Expr::Call;
    let found = only(&opaque_event.build());
    assert!(found
        .states
        .iter()
        .all(|s| s.successors == Successors::Unknown));
}

#[test]
fn two_case_structures_on_one_register_are_two_machines() {
    let mut diagram = base().build();
    let inside = &mut diagram.root.nodes[1].diagrams[0];
    let mut second = inside.nodes[1].clone();
    assert_eq!(second.uid, CASE);
    // A copy with ids of its own, selected by the same register but not
    // wired to it: it handles the states without choosing the next one.
    second.uid = 80_000;
    let renumber = |node: &mut buzzard_rsrc::Node| {
        for terminal in &mut node.terminals {
            terminal.uid += 200_000;
            terminal.dco = terminal.dco.map(|dco| dco + 200_000);
        }
    };
    renumber(&mut second);
    for frame in &mut second.diagrams {
        frame.uid += 200_000;
        frame.wires.clear();
        build::visit(frame, &mut |node| {
            node.uid += 200_000;
            renumber(node);
        });
    }
    let source = inside.wires[0].source;
    let selected_by = second.terminals[0].uid;
    inside.wires.push(buzzard_rsrc::Wire {
        uid: 80_001,
        source,
        sinks: vec![selected_by],
    });
    inside.nodes.push(second);
    build::mark_wired(&mut diagram);

    let found = extract(&diagram);
    assert_eq!(found.machines.len(), 2);
    assert_eq!(
        found.machines[1].location,
        format!("while loop #{LOOP}, case structure #80000")
    );
    // The first is unchanged. In the second, the register is written by
    // the first structure, of which any frame may run as far as the
    // second can tell: its frames are the same states, each of which may
    // be followed by any state the first structure writes.
    assert_eq!(
        Machine {
            location: String::new(),
            ..found.machines[0].clone()
        },
        only(&base().build())
    );
    // ("Exit" holds the state in the first structure, which for the
    // second is whichever state is current.)
    let expected = machine(
        vec![
            state("Init", &["Exit", "Init", "Run"], MAYBE),
            state("Run", &["Exit", "Run"], MAYBE),
            state("Exit", &["Exit", "Run"], MAYBE),
        ],
        Some(&["Init"]),
        None,
    );
    assert_eq!(
        Machine {
            location: String::new(),
            ..found.machines[1].clone()
        },
        expected
    );
}

// ------------------------------------------------------- injected bugs

#[test]
fn injected_bugs_in_a_real_machine_are_found() {
    let diagram = fixture("jki-smo/process.vi");
    let real = extract(&diagram).machines.remove(0);
    assert_eq!(real.check().findings, []);

    // Retarget "Init" from "Idle" to itself: "Idle" becomes unreachable.
    let mut bug = real.clone();
    bug.states[0].successors = Successors::Known(names(&["Init"]));
    assert_eq!(bug.check().findings, [unreachable("Idle")]);

    // Remove the only transition out of "Init", and say it never stops
    // (the diagram leaves its stop tunnel unwired).
    let mut bug = real.clone();
    bug.states[0].successors = Successors::Known(names(&[]));
    assert_eq!(bug.check().findings, [unreachable("Idle")]);
    bug.states[0].stop = NEVER;
    assert_eq!(
        bug.check().findings,
        [unreachable("Idle"), Finding::NeverTerminates]
    );

    // Misspell the target: the default case runs, and the finding says so.
    let mut bug = real.clone();
    bug.states[0].successors = Successors::Known(names(&[]));
    bug.states[0].unmatched = names(&["Idel"]);
    assert_eq!(
        bug.check().findings,
        [Finding::UndefinedTarget {
            from: "Init".to_owned(),
            target: "Idel".to_owned(),
            handled_by: Some("Idle".to_owned()),
        }]
    );

    // Make "Init" a trap: it holds itself and never stops.
    let mut bug = real.clone();
    bug.states[0].stop = NEVER;
    bug.states[0].successors = Successors::Known(names(&["Init"]));
    bug.initial = Some(names(&["Idle"]));
    bug.states[1].successors = Successors::Known(names(&["Idle", "Init"]));
    assert_eq!(
        bug.check().findings,
        [Finding::CannotTerminate {
            state: "Init".to_owned()
        }]
    );
    // Make no state stop: the loop can never stop.
    let mut bug = real;
    for state in &mut bug.states {
        state.stop = NEVER;
    }
    assert_eq!(bug.check().findings, [Finding::NeverTerminates]);
}

#[test]
fn injected_bugs_in_the_twins_are_found() {
    let (spec, healthy) = ahmed();
    assert_eq!(healthy.check().findings, []);

    // Remove "End" -> "Shutdown" in the diagram itself.
    let again = spec.item("Process 1");
    let found = only(&spec.clone().next(3, again).build());
    assert_eq!(
        found.check().findings,
        [unreachable("Shutdown"), Finding::NeverTerminates]
    );
    // Retarget "Process 2" from "End" to "Shutdown".
    let found = only(&spec.clone().goes(2, "Shutdown").build());
    assert_eq!(found.check().findings, [unreachable("End")]);
    // Forget to wire the stop in "Shutdown".
    let found = only(&spec.clone().stop(4, Expr::Unwired).build());
    assert_eq!(found.check().findings, [Finding::NeverTerminates]);
    // Add a state that loops on itself and send "End" there too.
    let mut trap = spec;
    trap.items.push("Stuck");
    trap.frames.push(build::Frame {
        matches: vec![one(5)],
        next: Expr::Current,
        stop: Expr::Unwired,
    });
    let stuck = select(trap.item("Stuck"), trap.item("Shutdown"));
    let found = only(&trap.next(3, stuck).build());
    assert_eq!(
        found.check().findings,
        [Finding::CannotTerminate {
            state: "Stuck".to_owned()
        }]
    );
}

// ------------------------------------------------------ hostile diagrams

#[test]
fn damaged_diagrams_do_not_panic() {
    // Structural damage to a real diagram and to a built one: ids
    // duplicated or redirected, wires reversed and cut, frames dropped,
    // tunnel facts and selector tables changed.
    let mut subjects = vec![fixture("jki-smo/process.vi"), candle().0.build()];
    subjects.push(fixture("jki-state-machine/new-support-vis-examples.vi"));
    for original in subjects {
        let total = original.nodes().map(|n| n.terminals.len()).sum::<usize>();
        let step = (total / 60).max(1);
        for target in (0..total).step_by(step) {
            for damage in 0..8 {
                let mut diagram = original.clone();
                let mut seen = 0;
                build::visit(&mut diagram.root, &mut |node| {
                    for index in 0..node.terminals.len() {
                        if seen == target {
                            let terminal = &mut node.terminals[index];
                            match damage {
                                0 => terminal.uid = 1,
                                1 => terminal.dco = Some(LEFT),
                                2 => terminal.dco = None,
                                3 => terminal.tunnel = None,
                                4 => terminal.dco_class = Some("lSR".to_owned()),
                                5 => terminal.constant = Some(Value::Integer(i64::MIN)),
                                6 => terminal.param = Some(0),
                                _ => {
                                    let copy = terminal.clone();
                                    node.terminals.push(copy);
                                }
                            }
                        }
                        seen += 1;
                    }
                });
                let _ = verify(&diagram).issues();
            }
        }
        let nodes = original.nodes().count();
        for target in (0..nodes).step_by((nodes / 40).max(1)) {
            for damage in 0..6 {
                let mut diagram = original.clone();
                let mut seen = 0;
                build::visit(&mut diagram.root, &mut |node| {
                    if seen == target {
                        match damage {
                            0 => node.diagrams.truncate(1),
                            1 => node.diagrams.clear(),
                            2 => node.terminals.clear(),
                            3 => node.class = "whileLoop".to_owned(),
                            4 => {
                                if let Some(selector) = &mut node.selector {
                                    selector.frames.pop();
                                    selector.default_frame = Some(u32::MAX);
                                    selector.enum_items.clear();
                                }
                            }
                            _ => {
                                for frame in &mut node.diagrams {
                                    for wire in &mut frame.wires {
                                        std::mem::swap(&mut wire.source, &mut wire.sinks[0]);
                                    }
                                    frame.nodes.reverse();
                                }
                            }
                        }
                    }
                    seen += 1;
                });
                let _ = verify(&diagram).issues();
            }
        }
    }
}

#[test]
fn a_walk_that_branches_without_end_gives_up() {
    // Each Select feeds both inputs from the next one, so the number of
    // paths doubles at every level.
    let mut diagram = doomed().goes(1, "Exit").build();
    let frame = {
        let case = &mut diagram.root.nodes[1].diagrams[0].nodes[1];
        &mut case.diagrams[1]
    };
    let constant = frame.wires[0].source;
    let sink = frame.wires[0].sinks[0];
    frame.wires.clear();
    let mut uid = 300_000;
    let mut feed = sink;
    let ty = build::ENUM16;
    for _ in 0..40 {
        let terminal = |uid: i32, param: u32| buzzard_rsrc::Terminal {
            uid,
            param: Some(param),
            dco: Some(uid + 1),
            dco_class: Some("parm".to_owned()),
            data_type: Some(ty.data),
            numeric: ty.numeric,
            wired: true,
            ..buzzard_rsrc::Terminal::default()
        };
        let node = buzzard_rsrc::Node {
            uid,
            class: "prim".to_owned(),
            primitive: Some(build::SELECT),
            terminals: (0..4)
                .map(|p| terminal(uid + 10 + 2 * p, p as u32))
                .collect(),
            ..buzzard_rsrc::Node::default()
        };
        frame.wires.push(buzzard_rsrc::Wire {
            uid: uid + 1,
            source: uid + 10,
            sinks: vec![feed],
        });
        if feed != sink {
            // Both value inputs of the previous Select hang on this one.
            frame.wires.last_mut().unwrap().sinks.push(feed + 4);
        }
        feed = uid + 12;
        frame.nodes.push(node);
        uid += 100;
    }
    frame.wires.push(buzzard_rsrc::Wire {
        uid,
        source: constant,
        sinks: vec![feed, feed + 4],
    });
    let started = std::time::Instant::now();
    let found = only(&diagram);
    assert_eq!(found.states[1].successors, Successors::Unknown);
    assert!(started.elapsed().as_secs() < 20);
}
