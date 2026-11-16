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

use buzzard_lint::verify::report::{to_text, Verified};
use buzzard_lint::verify::{
    extract, verify, Confidence, Finding, Machine, Problem, State, Stop, Successors, MAX_STATES,
};
use buzzard_lint::Inspection;
use buzzard_rsrc::{BlockDiagram, DataType, Loss, NumericType, SelectorKind, TunnelMode, Value};

#[path = "verify/build.rs"]
mod build;

use build::{
    boolean, integer, label, mark_wired, one, range, select, text, with_node, with_terminals, Expr,
    Spec, Wrap, ADD, CASE, CONDITION, EQUAL, LEFT, LOOP, STOP,
};

// ------------------------------------------------------------- helpers

fn fixture(name: &str) -> BlockDiagram {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/verify")
        .join(name);
    Inspection::from_path(&path)
        .expect("parse the fixture")
        .diagram
        .expect("the fixture has a block diagram")
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
        ..Machine::default()
    }
}

/// The one machine of a diagram, without its location and without the
/// list of nodes that could end the program some other way, which
/// `other_ways_out_keep_a_loop_that_cannot_stop_from_being_proven` covers.
fn only(diagram: &BlockDiagram) -> Machine {
    let found = extract(diagram);
    assert_eq!(found.queue_machines, []);
    let [machine] = &found.machines[..] else {
        panic!("expected one machine, found {}", found.machines.len());
    };
    Machine {
        location: String::new(),
        other_exits: Vec::new(),
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
            value: (0..)
                .zip(["Cooling", "Heating", "Start", "Stop", "Idle", "Init"])
                .find_map(|(value, item)| (item == next).then_some(value))
                .expect("the next state is one of the items"),
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
/// `<owner>_<repository>`, the location it is expected at, and the type
/// definition its selector is an instance of.
const REAL: [(&str, &str, &str, Twin); 8] = [
    (
        "lallers_StateMachine/StateMachineP7.vi",
        "while loop #38, case structure #49",
        "State.ctl",
        lallers,
    ),
    (
        "lallers_StateMachine/StatemMachineLV7/Main.vi",
        "while loop #38, case structure #49",
        "State.ctl",
        lallers,
    ),
    (
        "AhmedDMoHassan_Simple-State-machine/Statemachine/Main.vi",
        "while loop #160, case structure #193",
        "Selector.ctl",
        ahmed,
    ),
    (
        "frank1641_Multiple-consumer-State-machine/Queue State Machine Lib/Main.vi",
        "while loop #38, case structure #49",
        "State.ctl",
        frank,
    ),
    (
        "Gluvertski1_StateMachine_LabVIEW/StateMachine.vi",
        "while loop #637, case structure #684",
        "StateMachineController.ctl",
        gluvertski,
    ),
    (
        "GaborDernoczi_LabVIEW_State_Machine/State_Machine.vi",
        "while loop #70, case structure #117",
        "Enum_Type_Def.ctl",
        gabor,
    ),
    (
        "TomsLabVIEWAdventure_Guessing-Game/Main.vi",
        "while loop #601, case structure #627",
        "State.ctl",
        toms,
    ),
    (
        "CandleScience_Labview-State-Machine-Template/SimpleStateMachineTemplate.vi",
        "while loop #105, case structure #217",
        "state.ctl",
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
    for (path, location, type_definition, twin) in REAL {
        let file = root.join(path);
        let diagram = Inspection::from_path(&file)
            .unwrap_or_else(|e| panic!("{}: {e}", file.display()))
            .diagram
            .unwrap();
        assert!(diagram.is_complete(), "{path}");
        let found = extract(&diagram);
        assert!(found.queue_machines.is_empty(), "{path}");
        let expected = Machine {
            location: location.to_owned(),
            type_definition: Some(type_definition.to_owned()),
            ..twin().1
        };
        let [machine] = &found.machines[..] else {
            panic!("{path}: {} machines", found.machines.len());
        };
        let compared = Machine {
            other_exits: Vec::new(),
            ..machine.clone()
        };
        assert_eq!(compared, expected, "{path}");
    }
}

/// The single changed bytes of real files that were reported to give
/// false proofs, as `(file, offset in the inflated heap, old byte, new
/// byte)`.
///
/// * The first renumbers the inner terminal of the next-state tunnel in
///   one frame, so the wire into it ends nowhere.
/// * The second makes that wire's signal object undecodable.
///   Either way the frame looked unwired, the tunnel's default was taken
///   for what the frame writes, and three states were "proven"
///   unreachable.
/// * The third turns the frame index of a selector table entry into a
///   second copy of another field. The absent index read as frame 0, a
///   state `frame 1` was made up for the frame left without a value, and
///   five states were "proven" unreachable.
///
/// Each is now a recorded loss, so the diagram is not searched at all.
const REPORTED_DAMAGE: [(&str, usize, u8, u8); 3] = [
    (
        "GaborDernoczi_LabVIEW_State_Machine/State_Machine.vi",
        4899,
        0xf0,
        0x00,
    ),
    (
        "GaborDernoczi_LabVIEW_State_Machine/State_Machine.vi",
        5197,
        0xfe,
        0x00,
    ),
    (
        "TomsLabVIEWAdventure_Guessing-Game/Main.vi",
        24731,
        0x23,
        0x21,
    ),
];

/// The invariant of `a_damaged_heap_never_adds_a_proven_finding` on the
/// real enumeration machines, which cannot be redistributed: first the
/// three reported bytes, then a sample of every heap.
#[test]
#[ignore = "needs BUZZARD_FSM_SAMPLES to point at clones of the sample repositories"]
fn damage_to_the_real_machines_proves_nothing_new() {
    use buzzard_rsrc::blocks::zlib;
    use buzzard_rsrc::RsrcFile;

    let root = PathBuf::from(
        std::env::var_os("BUZZARD_FSM_SAMPLES").expect("BUZZARD_FSM_SAMPLES is not set"),
    );
    for (path, at, old, new) in REPORTED_DAMAGE {
        let bytes = std::fs::read(root.join(path)).unwrap();
        let file = RsrcFile::parse(&bytes).unwrap();
        let mut heap = zlib::decompress(file.first(b"BDHb").unwrap()).unwrap();
        let pristine = BlockDiagram::from_inflated_heap(&file, &heap)
            .unwrap()
            .unwrap();
        assert_eq!(extract(&pristine).machines.len(), 1, "{path}");
        assert_eq!(heap[at], old, "{path} byte {at}");
        heap[at] = new;
        let damaged = BlockDiagram::from_inflated_heap(&file, &heap)
            .unwrap()
            .unwrap();
        assert!(!damaged.is_complete(), "{path} byte {at}");
        assert_eq!(extract(&damaged).machines, [], "{path} byte {at}");
        assert_eq!(verify(&damaged).issues(), [], "{path} byte {at}");
    }

    let step = damage_step(53);
    let mut total = Damage::default();
    for (path, ..) in REAL {
        let found = damage(&std::fs::read(root.join(path)).unwrap(), step);
        assert_eq!(found.added, [""; 0], "{path}");
        assert_eq!(found.vanished, [""; 0], "{path}");
        total.trials += found.trials;
        total.decoded += found.decoded;
        total.incomplete += found.incomplete;
        total.machines += found.machines;
        total.revalued.extend(found.revalued);
    }
    println!(
        "{} damaged heaps, {} decoded, {} of them with a recorded loss, {} still with a machine",
        total.trials, total.decoded, total.incomplete, total.machines
    );
    println!(
        "{} proofs about a diagram that differs in a value only: {:#?}",
        total.revalued.len(),
        total.revalued
    );
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
    assert_eq!(found.queue_machines, []);
    let expected = Machine {
        location: "while loop #3379, case structure #3023".to_owned(),
        // The diagram calls 17 different subVIs, any of which could end
        // the program; the first eight are named.
        other_exits: [
            "a subVI call",
            "subVI `Add Registered Process.vi`",
            "subVI `CleanDependencies.vi`",
            "subVI `Clear Errors.vi`",
            "subVI `Compute Shared Key.vi`",
            "subVI `Destroy.vi`",
            "subVI `FindDependency(byKey).vi`",
            "subVI `Get CallerWatchdog Queue.vi`",
            "9 more of these kinds",
        ]
        .map(str::to_owned)
        .to_vec(),
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
    assert_eq!(template.machines, []);
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
    assert_eq!(queue.unhandled, [""; 0]);
    assert_eq!(template.issues(), []);

    // A documentation example enqueues three names that have no case. The
    // case structures on the state's arguments, nested in the one on the
    // state, are not machines of their own.
    let example = verify(&fixture("jki-state-machine/new-support-vis-examples.vi"));
    assert_eq!(example.machines, []);
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
    assert_eq!(queued_names("//Macro: Exit"), [""; 0]);
    assert_eq!(
        queued_names("//Test: Start //Only for development."),
        [""; 0]
    );
    assert_eq!(queued_names("/* Post Error States"), [""; 0]);
    assert_eq!(queued_names("A /* x\ny */ B\nC"), ["A  B", "C"]);
    assert_eq!(queued_names("Open >> http://example.test/a"), ["Open"]);
    assert_eq!(queued_names(""), [""; 0]);
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
            looping.class = class.to_owned();
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
    fn frame(diagram: &mut BlockDiagram) -> &mut buzzard_rsrc::Diagram {
        let case = &mut diagram.root.nodes[1].diagrams[0].nodes[1];
        assert_eq!(case.uid, CASE);
        &mut case.diagrams[1]
    }
    // The next-state constant of the frame.
    fn is_item(terminal: &buzzard_rsrc::Terminal) -> bool {
        matches!(terminal.constant, Some(Value::Enum { .. }))
    }
    let run_is_unknown = |diagram: &BlockDiagram, what: &str| {
        let found = only(diagram);
        assert_eq!(found.states[1].successors, Successors::Unknown, "{what}");
        assert!(!claims_impossibility(&found.check().findings), "{what}");
    };
    let spec = doomed().goes(1, "Run");

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
    let changes: [fn(&mut buzzard_rsrc::Terminal); 4] = [
        |t| t.tunnel.as_mut().unwrap().mode = TunnelMode::Other,
        |t| t.tunnel.as_mut().unwrap().conditional = true,
        |t| t.tunnel.as_mut().unwrap().output = false,
        |t| t.tunnel = None,
    ];
    for change in changes {
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
    mark_wired(&mut diagram);

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
                                    selector.enum_items = [].into();
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
            terminals: (0u8..4)
                .map(|p| terminal(uid + 10 + 2 * i32::from(p), u32::from(p)))
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

// ------------------------------------------------------------ lint rules

/// The diagnostics `lint` gives for a file with the given diagram.
fn lint_diagram(diagram: BlockDiagram) -> Vec<(String, &'static str, String)> {
    use buzzard_lint::{lint, DependencyGraph, FileState, LintConfig, Workspace};

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify/jki-smo");
    let mut inspection =
        Inspection::from_path(&path.join("process.vi")).expect("parse the fixture");
    inspection.diagram = Some(diagram);
    let mut workspace = Workspace::default();
    workspace.insert(
        Path::new("Main.vi"),
        FileState::from_inspection(Ok(inspection)),
    );
    let graph = DependencyGraph::build(&workspace);
    lint(&workspace, &graph, &LintConfig::default())
        .into_iter()
        .filter(|d| d.code.starts_with("BZ2"))
        .map(|d| (d.severity.as_str().to_owned(), d.code, d.message))
        .collect()
}

#[test]
fn lint_reports_the_findings_under_three_rules() {
    let at = format!("state machine at while loop #{LOOP}, case structure #{CASE}");
    let line = |severity: &str, code: &'static str, text: &str| {
        (severity.to_owned(), code, format!("{at}: {text} (proven)"))
    };
    let structural = |severity: &str, code: &'static str, text: &str| {
        let how = "(structural, not proven)";
        (severity.to_owned(), code, format!("{at}: {text} {how}"))
    };

    // Unreachable states are warnings; a loop that never stops at all is
    // informational, since a daemon is a legitimate design.
    assert_eq!(
        lint_diagram(doomed().build()),
        [
            line("warning", "BZ201", "state `Exit` can never be reached"),
            line("warning", "BZ201", "state `Orphan` can never be reached"),
            line(
                "info",
                "BZ202",
                "the loop's condition terminal can never stop the loop"
            ),
        ]
    );
    // A trap state among states that can stop is a warning.
    let trap = Spec::enumeration(&["Init", "Run", "Exit", "Stuck"])
        .starts_in("Init")
        .goes(1, "Exit")
        .stop(2, boolean(true));
    let either = select(trap.item("Stuck"), trap.item("Run"));
    assert_eq!(
        lint_diagram(trap.next(0, either).build()),
        [line(
            "warning",
            "BZ202",
            "once in state `Stuck` the loop's condition terminal can never stop the loop"
        )]
    );
    // A misspelled state name that the default case swallows.
    let typo = Spec::strings(&["Init", "Run", "Idle"])
        .default(2)
        .starts(text("Init"))
        .next(0, text("Rnu"))
        .stop(2, boolean(true));
    assert_eq!(
        lint_diagram(typo.build()),
        [
            line("warning", "BZ201", "state `Run` can never be reached"),
            structural(
                "warning",
                "BZ203",
                "a constant `Rnu` that no case handles can reach the state register; the \
                 default case `Idle` would run for it"
            ),
        ]
    );
    // A healthy machine, and a machine whose wiring cannot be followed,
    // report nothing.
    assert_eq!(lint_diagram(base().build()), []);
    assert_eq!(lint_diagram(doomed().next(1, Expr::Call).build()), []);

    for code in ["BZ201", "BZ202", "BZ203"] {
        let rule = buzzard_lint::rules::rule(code).unwrap();
        assert_eq!(rule.severity, buzzard_lint::Severity::Warning);
        assert!(!rule.summary.is_empty() && !rule.name.is_empty());
    }
}

#[test]
fn issues_say_how_much_they_claim() {
    let report = verify(&doomed().build());
    let issues = report.issues();
    assert_eq!(issues.len(), 3);
    for issue in &issues {
        assert_eq!(issue.confidence(), Confidence::Proven);
        assert!(issue.justification().ends_with(char::is_alphabetic));
        assert!(!issue.justification().contains('\n'));
    }
    let codes: Vec<&str> = issues
        .iter()
        .map(buzzard_lint::verify::Issue::code)
        .collect();
    assert_eq!(codes, ["BZ201", "BZ201", "BZ202"]);
    assert_eq!(Confidence::Proven.as_str(), "PROVEN");
    assert_eq!(Confidence::Structural.as_str(), "STRUCTURAL");
    assert_eq!(Confidence::Heuristic.as_str(), "HEURISTIC");
}

#[test]
fn a_selector_table_that_contradicts_itself_is_tolerated() {
    // A default frame that does not exist, as no decoder would give it.
    let mut diagram = Spec::strings(&["Init", "Run"])
        .default(1)
        .starts(text("Nowhere"))
        .next(0, text("Zzz"))
        .build();
    with_node(&mut diagram, CASE, |case| {
        case.selector.as_mut().unwrap().default_frame = Some(99);
    });
    let found = only(&diagram);
    assert_eq!(found.initial, None);
    assert_eq!(found.default, None);
    assert_eq!(found.states[0].unmatched, names(&["Zzz"]));
    let _ = found.check();
    let _ = verify(&diagram).issues();
}

// ------------------------------------------- what a finding may claim

/// The text report `buzzard verify` prints for one diagram.
fn text_of(diagram: &BlockDiagram) -> String {
    let result = Ok(Some(verify(diagram)));
    to_text(&Verified::new(vec![(PathBuf::from("Main.vi"), result)]))
}

/// The messages of every issue with their confidence.
fn claims(diagram: &BlockDiagram) -> Vec<(Confidence, &'static str, String)> {
    verify(diagram)
        .issues()
        .iter()
        .map(|issue| (issue.confidence(), issue.code(), issue.message()))
        .collect()
}

/// Add a node of the given class next to the loop.
fn with_extra_node(mut diagram: BlockDiagram, class: &str, name: Option<&str>) -> BlockDiagram {
    diagram.root.nodes.push(buzzard_rsrc::Node {
        uid: 70_000 + i32::try_from(diagram.root.nodes.len()).expect("few nodes"),
        class: class.to_owned(),
        name: name.map(str::to_owned),
        ..buzzard_rsrc::Node::default()
    });
    diagram
}

#[test]
fn a_diagram_with_a_recorded_loss_is_not_searched() {
    // The shape of the real file this was found on: one frame writes a
    // constant through a wire and another leaves the tunnel unwired, so
    // the tunnel is marked as using the default, which is "Wait".
    let spec = Spec::enumeration(&["Wait", "Init", "Work"])
        .starts_in("Wait")
        .goes(0, "Init")
        .goes(1, "Work")
        .next(2, Expr::Unwired)
        .stop(2, boolean(true));
    let whole = spec.build();
    assert!(whole.is_complete());
    assert_eq!(verify(&whole).issues(), []);

    // The wire from the constant "Init" is gone, as when the decoder
    // skips a signal it cannot resolve or a wire end it cannot find.
    let mut damaged = whole.clone();
    with_node(&mut damaged, CASE, |case| {
        case.diagrams[0].wires.remove(0);
    });
    mark_wired(&mut damaged);
    // In a diagram that decoded completely a missing wire means that the
    // tunnel is unwired, and two states are then rightly unreachable.
    assert_eq!(
        only(&damaged).check().findings,
        [
            unreachable("Init"),
            unreachable("Work"),
            Finding::NeverTerminates
        ]
    );

    // With the loss on record nothing is claimed: no machine at all.
    let lossy = |place: fn(&mut BlockDiagram) -> &mut buzzard_rsrc::Diagram, loss: Loss| {
        let mut diagram = damaged.clone();
        place(&mut diagram).losses = vec![loss];
        assert!(!diagram.is_complete());
        diagram
    };
    let in_the_frame = lossy(
        |diagram| {
            let inside = &mut diagram.root.nodes[1].diagrams[0];
            let case = inside.nodes.iter_mut().find(|n| n.uid == CASE).unwrap();
            &mut case.diagrams[0]
        },
        Loss::Wire,
    );
    let elsewhere = lossy(|diagram| &mut diagram.root, Loss::RepeatedObject);
    for diagram in [in_the_frame, elsewhere] {
        let found = extract(&diagram);
        assert!(found.incomplete);
        assert_eq!(found.machines, []);
        let checked = verify(&diagram);
        assert!(checked.incomplete);
        assert_eq!(checked.issues(), []);
        assert_eq!(lint_diagram(diagram.clone()), []);
        let report = Verified::new(vec![(PathBuf::from("Main.vi"), Ok(Some(checked)))]);
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["incomplete"], serde_json::json!(["Main.vi"]));
        assert_eq!(json["summary"]["incomplete"], 1);
        assert_eq!(json["files"], serde_json::json!([]));
        assert_eq!(
            text_of(&diagram),
            "Main.vi: the block diagram did not decode completely; it was not searched for \
             state machines, because nothing could be proven about one\n\
             Checked 1 file: 0 with a state machine (0 state machines, 0 string/queue \
             machines), 1 without.\nNo findings.\n"
        );
    }
    // Any loss on the undamaged diagram refuses it as well.
    let mut diagram = whole;
    diagram.root.losses = vec![Loss::WireEnd];
    assert_eq!(extract(&diagram).machines, []);
}

#[test]
fn a_frame_that_no_value_selects_gives_no_machine() {
    // "Exit" has no entry and is not the default: the table contradicts
    // the frame list. A state "frame 2" used to be made up for it.
    let mut spec = base();
    spec.frames[2].matches.clear();
    let found = extract(&spec.build());
    assert_eq!(found.machines, []);
    assert!(!found.incomplete);
    assert_eq!(verify(&spec.build()).issues(), []);
    // Only the default frame may be without a value.
    let found = only(&spec.default(2).build());
    assert_eq!(found.states[2].name, "Default");
    assert_eq!(found.default.as_deref(), Some("Default"));
}

#[test]
fn a_state_named_after_several_values_reads_as_one_state() {
    let mut spec = Spec::enumeration(&["A", "B", "C"])
        .starts_in("A")
        .goes(2, "A");
    spec.frames.remove(1);
    spec.frames[0].matches = vec![one(0), one(1)];
    let text = text_of(&spec.build());
    assert!(text.contains("    states (2): `A, B`, C\n"), "{text}");
    assert!(text.contains("    initial state: `A, B`\n"), "{text}");
    assert!(
        text.contains("      C -> `A, B`; never stops the loop\n"),
        "{text}"
    );
}

#[test]
fn a_constant_without_a_case_is_structural_and_reported_once() {
    // The constant sits in a frame of a disable structure that may be
    // the disabled one, so it may never be written.
    let disabled = Spec::strings(&["Init", "Idle", "Exit"])
        .default(1)
        .starts(text("Init"))
        .next(0, Expr::Disable(vec![text("Idle"), text("Bogus")]))
        .stop(1, Expr::Control)
        .stop(2, boolean(true));
    let message = "a constant `Bogus` that no case handles can reach the state register; the \
                   default case `Idle` would run for it";
    assert_eq!(
        claims(&disabled.build()),
        [
            (
                Confidence::Proven,
                "BZ201",
                "state `Exit` can never be reached".to_owned()
            ),
            (Confidence::Structural, "BZ203", message.to_owned()),
        ]
    );
    let report = text_of(&disabled.build());
    assert!(
        report.contains(&format!(
            "    STRUCTURAL [BZ203] {message}\n      why: a constant"
        )),
        "{report}"
    );
    assert!(!report.contains("PROVEN [BZ203]"));
    assert!(report.ends_with("2 findings: 1 proven, 1 structural, 0 heuristic.\n"));
    let lint = lint_diagram(disabled.build());
    assert!(lint[1]
        .2
        .ends_with(&format!("{message} (structural, not proven)")));
    let issues = verify(&disabled.build()).issues();
    assert!(issues[1].justification().contains("is not determined"));
    assert!(!issues[1].message().contains("writes"));

    // The case structure sits in one frame of an event structure and
    // another event frame writes the constant: one finding for the
    // machine, where every state used to be blamed for writing it.
    let mut wrapped = Spec::strings(&["Init", "Run", "Exit"])
        .default(0)
        .starts(text("Init"))
        .next(0, text("Run"))
        .next(1, text("Exit"));
    wrapped.wrap = Some(Wrap {
        others: vec![(text("Shutdown"), boolean(false))],
        timeout_wired: false,
    });
    let found = claims(&wrapped.build());
    let about: Vec<&(Confidence, &str, String)> = found
        .iter()
        .filter(|(_, code, _)| *code == "BZ203")
        .collect();
    assert_eq!(about.len(), 1, "{found:?}");
    assert_eq!(about[0].0, Confidence::Structural);
    assert!(about[0]
        .2
        .starts_with("a constant `Shutdown` that no case handles"));

    // A state that always stops the loop writes a value that is never
    // used to select a case: nothing is reported.
    let stopping = Spec::strings(&["Init", "Exit"])
        .default(0)
        .starts(text("Init"))
        .next(0, text("Exit"))
        .next(1, text("done"))
        .stop(1, boolean(true));
    assert_eq!(
        only(&stopping.build()).states[1].unmatched,
        names(&["done"])
    );
    assert_eq!(claims(&stopping.build()), []);
}

#[test]
fn other_ways_out_keep_a_loop_that_cannot_stop_from_being_proven() {
    let never = "the loop's condition terminal can never stop the loop";
    // Nothing but the loop on the diagram: proven.
    let alone = doomed().build();
    assert_eq!(extract(&alone).machines[0].other_exits, [""; 0]);
    let found = claims(&alone);
    assert_eq!(found[2], (Confidence::Proven, "BZ202", never.to_owned()));
    // A built-in function that only computes changes nothing.
    let computing = with_extra_node(doomed().build(), "prim", None);
    assert_eq!(claims(&computing), found);

    // Anything that could end the program or abort the VI: the finding
    // is not proven and names what was found.
    for (class, name, shown) in [
        ("iUse", Some("Quit App.vi"), "subVI `Quit App.vi`"),
        ("iUse", None, "a subVI call"),
        ("polyIUse", Some("Close.vi"), "subVI `Close.vi`"),
        ("dynIUse", Some("Stop.vi"), "subVI `Stop.vi`"),
        ("callByRefNode", None, "a call by reference"),
        ("invokeNode", None, "an invoke node"),
        ("propNode", None, "a property node"),
        ("extFunc", None, "a call into a library"),
        ("xNode", None, "an XNode"),
        ("neverSeenBefore", None, "a node of class `neverSeenBefore`"),
    ] {
        let diagram = with_extra_node(doomed().build(), class, name);
        assert_eq!(extract(&diagram).machines[0].other_exits, [shown]);
        let found = claims(&diagram);
        // Reachability does not depend on it.
        assert_eq!(found[0].0, Confidence::Proven);
        assert_eq!(found[1].0, Confidence::Proven);
        let expected = format!("{never}; the program could still be ended by: {shown}");
        assert_eq!(
            found[2],
            (Confidence::Structural, "BZ202", expected),
            "{class}"
        );
        let report = text_of(&diagram);
        assert!(report.contains("    STRUCTURAL [BZ202] "), "{report}");
        assert!(!report.contains("PROVEN [BZ202]"), "{report}");
        assert!(report.contains("so this does not prove that the loop runs forever"));
        let lint = lint_diagram(diagram);
        assert!(lint[2].2.ends_with("(structural, not proven)"), "{lint:?}");
    }

    // The same for a single trap state, and the node may sit anywhere,
    // here inside another frame than the trap's.
    let trap = Spec::enumeration(&["Init", "Run", "Exit", "Stuck"])
        .starts_in("Init")
        .goes(1, "Exit")
        .stop(2, boolean(true));
    let either = select(trap.item("Stuck"), trap.item("Run"));
    let trap = trap.next(0, either);
    let stuck = "once in state `Stuck` the loop's condition terminal can never stop the loop";
    assert_eq!(
        claims(&trap.build()),
        [(Confidence::Proven, "BZ202", stuck.to_owned())]
    );
    let mut diagram = trap.build();
    with_node(&mut diagram, CASE, |case| {
        case.diagrams[1].nodes.push(buzzard_rsrc::Node {
            uid: 70_001,
            class: "invokeNode".to_owned(),
            ..buzzard_rsrc::Node::default()
        });
    });
    assert_eq!(
        claims(&diagram),
        [(
            Confidence::Structural,
            "BZ202",
            format!("{stuck}; the program could still be ended by: an invoke node")
        )]
    );

    // A long list is cut short.
    let mut crowded = doomed().build();
    for index in 0..40 {
        crowded = with_extra_node(crowded, "iUse", Some(&format!("Sub {index:02}.vi")));
    }
    let exits = &extract(&crowded).machines[0].other_exits;
    assert_eq!(exits.len(), 9);
    assert_eq!(exits[0], "subVI `Sub 00.vi`");
    assert_eq!(exits[8], "32 more of these kinds");
}

#[test]
fn only_selector_types_whose_table_was_seen_are_resolved() {
    let ty = |numeric| build::Ty {
        data: DataType::Numeric,
        numeric: Some(numeric),
    };
    // Three frames `0`, `1`, `2` on a register of the given type, each
    // writing the next number as a constant of that type.
    let counter = |numeric, last: i32| {
        let mut spec = Spec::integers(vec![vec![one(0)], vec![one(1)], vec![one(last)]])
            .default(0)
            .starts(integer(0))
            .next(0, integer(1))
            .next(1, integer(i64::from(last)))
            .next(2, integer(0));
        spec.state = ty(numeric);
        spec.build()
    };
    let resolved = |diagram: &BlockDiagram| {
        let found = only(diagram);
        assert_eq!(found.states[0].successors, Successors::Known(names(&["1"])));
        assert_eq!(found.initial, Some(names(&["0"])));
    };
    // An integer register of which no successor resolves is not reported
    // as a machine at all.
    let unresolved = |diagram: &BlockDiagram| assert_eq!(extract(diagram).machines, []);

    // 32-bit signed tables were seen with negative numbers.
    resolved(&counter(NumericType::I32, 2));
    resolved(&counter(NumericType::I32, -1));
    resolved(&counter(NumericType::I32, i32::MIN));
    // U8 and I16 tables were seen with numbers that read the same
    // whether a narrow value is sign-extended or zero-extended.
    resolved(&counter(NumericType::U8, 255));
    resolved(&counter(NumericType::I16, i32::from(i16::MAX)));
    // Outside that range the encoding was never seen: -1 as an I16 could
    // be stored as -1 or as 65,535, and one of the two would send the
    // constant -1 to the default case instead of its own.
    unresolved(&counter(NumericType::I16, -1));
    unresolved(&counter(NumericType::I16, 65_535));
    unresolved(&counter(NumericType::I16, 32_768));
    unresolved(&counter(NumericType::U8, 256));
    unresolved(&counter(NumericType::U8, -1));
    // No table of these types was available at all.
    for numeric in [
        NumericType::I8,
        NumericType::U16,
        NumericType::U32,
        NumericType::I64,
        NumericType::U64,
    ] {
        unresolved(&counter(numeric, 2));
    }

    // Enumerations of every width were seen; a negative number in the
    // table of one was not.
    for numeric in [NumericType::Enum8, NumericType::Enum16, NumericType::Enum32] {
        let mut spec = base();
        spec.state = ty(numeric);
        assert!(only(&spec.build()).check().exact);
        spec.frames[2].matches = vec![one(-1)];
        let found = only(&spec.build());
        assert!(found
            .states
            .iter()
            .all(|s| s.successors == Successors::Unknown));
        assert_eq!(found.initial, None);
    }
    let mut spec = base();
    spec.state = ty(NumericType::Enum8);
    spec.frames[2].matches = vec![one(256)];
    assert_eq!(only(&spec.build()).initial, None);
}

#[test]
fn a_proof_about_a_type_definition_says_which_copy_it_is_about() {
    let mut spec = doomed();
    assert_eq!(only(&spec.build()).type_definition, None);
    let plain = claims(&spec.build());
    assert!(plain
        .iter()
        .all(|(_, _, message)| !message.contains("saved")));

    spec.type_definition = Some("State.ctl");
    let diagram = spec.build();
    assert_eq!(only(&diagram).type_definition.as_deref(), Some("State.ctl"));
    let saved = " (going by the items of the type definition `State.ctl` as saved in this file)";
    assert_eq!(
        claims(&diagram),
        [
            (
                Confidence::Proven,
                "BZ201",
                format!("state `Exit` can never be reached{saved}")
            ),
            (
                Confidence::Proven,
                "BZ201",
                format!("state `Orphan` can never be reached{saved}")
            ),
            (
                Confidence::Proven,
                "BZ202",
                format!("the loop's condition terminal can never stop the loop{saved}")
            ),
        ]
    );
    let report = text_of(&diagram);
    assert!(
        report.contains(
            "    states are the items of the type definition State.ctl as saved in this file; \
             the type definition itself was not read\n"
        ),
        "{report}"
    );
    assert!(lint_diagram(diagram)[0]
        .2
        .ends_with(&format!("{saved} (proven)")));

    // A string machine has no item list to go stale.
    let mut strings = Spec::strings(&["Init", "Run"])
        .default(0)
        .starts(text("Init"));
    strings.type_definition = Some("Name.ctl");
    assert_eq!(only(&strings.build()).type_definition, None);
}

#[test]
fn registers_that_do_not_pair_one_to_one_give_no_machine() {
    let none = |diagram: &BlockDiagram| assert_eq!(extract(diagram).machines, []);
    let register_terminal = |class: &str, dco: i32, uid: i32| buzzard_rsrc::Terminal {
        uid,
        dco: Some(dco),
        dco_class: Some(class.to_owned()),
        data_type: Some(DataType::Numeric),
        numeric: Some(NumericType::Enum16),
        ..buzzard_rsrc::Terminal::default()
    };
    // A second left terminal that the register does not list, as a
    // stacked register stored one element per object would look: on the
    // loop, on its diagram, and on both.
    for (outer, inner) in [(true, false), (false, true), (true, true)] {
        for class in ["lSR", "rSR"] {
            let mut diagram = base().build();
            with_node(&mut diagram, LOOP, |looping| {
                if outer {
                    looping
                        .terminals
                        .push(register_terminal(class, 88_000, 88_001));
                }
                if inner {
                    let boundary = &mut looping.diagrams[0].nodes[0];
                    boundary
                        .terminals
                        .push(register_terminal(class, 88_000, 88_002));
                }
            });
            none(&diagram);
        }
    }
    // A second register that is listed and paired is fine: it is data.
    let mut diagram = base().build();
    with_node(&mut diagram, LOOP, |looping| {
        looping
            .terminals
            .push(register_terminal("lSR", 88_000, 88_001));
        looping
            .terminals
            .push(register_terminal("rSR", 88_010, 88_011));
        let boundary = &mut looping.diagrams[0].nodes[0];
        boundary
            .terminals
            .push(register_terminal("lSR", 88_000, 88_002));
        boundary
            .terminals
            .push(register_terminal("rSR", 88_010, 88_012));
        let info = looping.loop_info.as_mut().unwrap();
        info.shift_registers.push(buzzard_rsrc::ShiftRegister {
            left: vec![88_000],
            right: 88_010,
        });
    });
    assert_eq!(extract(&diagram).machines.len(), 1);
    // The other register is stacked: the loop is refused, even though
    // the state register itself is a plain one.
    with_node(&mut diagram, LOOP, |looping| {
        let info = looping.loop_info.as_mut().unwrap();
        info.shift_registers[1].left.push(88_020);
        looping
            .terminals
            .push(register_terminal("lSR", 88_020, 88_021));
        let boundary = &mut looping.diagrams[0].nodes[0];
        boundary
            .terminals
            .push(register_terminal("lSR", 88_020, 88_022));
    });
    none(&diagram);
}

#[test]
fn a_structure_is_what_its_class_says_only_when_the_rest_agrees() {
    let count_terminal = || buzzard_rsrc::Terminal {
        uid: 88_100,
        dco: Some(88_101),
        dco_class: Some("lMax".to_owned()),
        data_type: Some(DataType::Numeric),
        numeric: Some(NumericType::I32),
        ..buzzard_rsrc::Terminal::default()
    };
    // "Init" chooses its successor in a nested case structure of which
    // one frame is unwired.
    let nested = Expr::Case {
        frames: vec![base().item("Run"), Expr::Unwired],
        default_if_unwired: true,
    };
    let mut diagram = base().next(0, nested).build();
    let init = |diagram: &BlockDiagram| only(diagram).states[0].successors.clone();
    assert_eq!(init(&diagram), Successors::Known(names(&["Init", "Run"])));
    // Without a selector table for its frames it is not read as a case
    // structure: an event structure whose class is misread would look
    // like this, and one with a timeout can hand out a default that no
    // frame writes.
    let strip = |diagram: &mut BlockDiagram, change: fn(&mut buzzard_rsrc::Node)| {
        build::visit(&mut diagram.root, &mut |node| {
            if node.class == "select" && node.uid != CASE {
                change(node);
            }
        });
    };
    strip(&mut diagram, |node| {
        node.selector.as_mut().unwrap().frames.pop();
    });
    assert_eq!(init(&diagram), Successors::Unknown);
    strip(&mut diagram, |node| node.selector = None);
    assert_eq!(init(&diagram), Successors::Unknown);

    // A nested while loop hands its value out; a loop with a count
    // terminal is a for loop whatever its class says, and a for loop that
    // runs zero times hands out a default.
    let looped = Expr::While(Box::new(base().item("Run")));
    let mut diagram = base().next(0, looped).build();
    assert_eq!(init(&diagram), Successors::Known(names(&["Run"])));
    build::visit(&mut diagram.root, &mut |node| {
        if node.class == "whileLoop" && node.uid != LOOP {
            node.terminals.push(count_terminal());
        }
    });
    assert_eq!(init(&diagram), Successors::Unknown);

    // The same for the machine's own loop, on either side of its border.
    let mut diagram = base().build();
    with_node(&mut diagram, LOOP, |looping| {
        looping.terminals.push(count_terminal());
    });
    assert_eq!(extract(&diagram).machines, []);
    let mut diagram = base().build();
    with_node(&mut diagram, LOOP, |looping| {
        looping.diagrams[0].nodes[0]
            .terminals
            .push(count_terminal());
    });
    assert_eq!(extract(&diagram).machines, []);
}

#[test]
fn a_huge_case_structure_is_not_modelled_and_a_large_one_is_quick() {
    let items = |count: usize| -> Vec<&'static str> {
        (0..count)
            .map(|index| &*Box::leak(format!("S{index}").into_boxed_str()))
            .collect()
    };
    // Every next state comes from a subVI, so every state may go
    // anywhere. That used to list every pair of states: 215 MB for 1,000
    // states and 3.4 GB for 4,000.
    let all_unknown = |count: usize| {
        let mut spec = Spec::enumeration(&items(count));
        for frame in &mut spec.frames {
            frame.next = Expr::Call;
        }
        spec.initial = spec.item("S0");
        spec.build()
    };
    let started = std::time::Instant::now();
    let largest = verify(&all_unknown(MAX_STATES));
    assert_eq!(largest.machines[0].machine.states.len(), MAX_STATES);
    assert_eq!(largest.machines[0].transitions, []);
    assert_eq!(
        largest.machines[0].reachable.as_ref().map(Vec::len),
        Some(MAX_STATES)
    );
    // Structural, because of the subVI.
    assert_eq!(largest.issues().len(), 1);
    let report = Verified::new(vec![(PathBuf::from("Main.vi"), Ok(Some(largest)))]);
    assert!(to_text(&report).contains("      S1023 -> ? (any state); never stops the loop\n"));
    assert!(buzzard_lint::verify::report::to_dot(&report).contains("m0_s1023 -> m0_any"));

    // One frame more is not a machine, however many more there are.
    for count in [MAX_STATES + 1, 8_000] {
        let found = extract(&all_unknown(count));
        assert_eq!(found.machines, []);
        assert!(!found.incomplete);
    }
    // Generous: all of the above takes about a second in a debug build.
    assert!(started.elapsed() < std::time::Duration::from_secs(60));
}

// ---------------------------------------------------------- damaged heaps
//
// The decoder skips what it cannot resolve. A wire it skipped used to
// look like a tunnel left unwired, which the tracer reads as the type's
// default, and that turned one changed byte in a real file into states
// "proven" unreachable. The decoder now records every loss and a diagram
// with a loss is not searched for classic machines.

/// The proven findings of a diagram, one line each.
fn proven(diagram: &BlockDiagram) -> BTreeSet<String> {
    verify(diagram)
        .issues()
        .into_iter()
        .filter(|issue| issue.confidence() == Confidence::Proven)
        .map(|issue| {
            format!(
                "{} at {}: {}",
                issue.code(),
                issue.location,
                issue.message()
            )
        })
        .collect()
}

/// What changing single bytes of a file's block diagram heap did.
#[derive(Debug, Default)]
struct Damage {
    /// Changed heaps tried.
    trials: usize,
    /// Of those, the ones that still decoded to a diagram.
    decoded: usize,
    /// Of those, the ones with a recorded loss.
    incomplete: usize,
    /// Of the decoded ones, those that still gave a classic machine.
    machines: usize,
    /// Proven findings the undamaged file does not have, with the place
    /// and the byte that produced them.
    added: Vec<String>,
    /// The same, where the damage changed nothing but a value: see
    /// [`skeleton`].
    revalued: Vec<String>,
    /// Damaged heaps that decoded without a recorded loss to a diagram
    /// with less on it than the undamaged one.
    vanished: Vec<String>,
}

/// The structure of a diagram without the values on it: what constants
/// and case tables say, types, names, positions and the polarity of the
/// condition terminal are blanked.
///
/// A changed byte that leaves this as it was has changed a value: an
/// enumeration constant now holds another item, a case is labelled with
/// another string or selected by another value, another case is the
/// default. The result is a complete diagram of another program, and a
/// proof about it is right about it; nothing was lost. Such findings are
/// counted apart. Everything else, a wire, a terminal, a node, a class,
/// a tunnel flag or a parameter index that differs, is damage to the
/// structure, and that must never add a proof.
fn skeleton(diagram: &BlockDiagram) -> BlockDiagram {
    let mut copy = diagram.clone();
    build::visit(&mut copy.root, &mut |node| {
        node.name = None;
        node.bounds = None;
        node.selector = None;
        if let Some(stop) = node.loop_info.as_mut().and_then(|info| info.stop.as_mut()) {
            stop.stop_if_true = false;
        }
        for terminal in &mut node.terminals {
            terminal.bounds = None;
            terminal.constant = None;
            terminal.data_type = None;
            terminal.numeric = None;
        }
    });
    copy
}

/// How much there is on a diagram: frames, nodes, terminals, wires and
/// wire ends.
fn amount(diagram: &BlockDiagram) -> [usize; 5] {
    [
        diagram.diagrams().len(),
        diagram.nodes().count(),
        diagram.nodes().map(|node| node.terminals.len()).sum(),
        diagram.wires().count(),
        diagram.wires().map(|wire| 1 + wire.sinks.len()).sum(),
    ]
}

/// The values a byte is changed to: both extremes, single bits low, in
/// the middle and high, and the neighbouring values.
fn damaged_values(byte: u8) -> BTreeSet<u8> {
    let mut values = BTreeSet::from([
        0x00,
        0xFF,
        byte ^ 0x01,
        byte ^ 0x02,
        byte ^ 0x10,
        byte ^ 0x80,
        byte.wrapping_add(1),
        byte.wrapping_sub(1),
    ]);
    values.remove(&byte);
    values
}

/// Change one byte at a time of the inflated block diagram heap of a
/// file, at every `step`-th position, to each of [`damaged_values`], and
/// decode and verify the result with the file's own types, links and
/// front panel.
fn damage(bytes: &[u8], step: usize) -> Damage {
    use buzzard_rsrc::blocks::zlib;
    use buzzard_rsrc::RsrcFile;

    let file = RsrcFile::parse(bytes).expect("parse the container");
    let heap = zlib::decompress(
        file.first(b"BDHb")
            .expect("the file has a block diagram heap"),
    )
    .expect("inflate the block diagram heap");
    let pristine = BlockDiagram::from_inflated_heap(&file, &heap)
        .expect("decode the block diagram")
        .expect("the heap holds a diagram");
    assert!(pristine.is_complete());
    let allowed = proven(&pristine);
    let whole = amount(&pristine);
    let structure = skeleton(&pristine);

    let mut out = Damage::default();
    let mut copy = heap.clone();
    for at in (0..heap.len()).step_by(step) {
        for value in damaged_values(heap[at]) {
            copy[at] = value;
            out.trials += 1;
            let Ok(Some(diagram)) = BlockDiagram::from_inflated_heap(&file, &copy) else {
                continue;
            };
            out.decoded += 1;
            out.incomplete += usize::from(!diagram.is_complete());
            let left = amount(&diagram);
            if diagram.is_complete() && left.iter().zip(&whole).any(|(now, before)| now < before) {
                out.vanished.push(format!(
                    "byte {at} {:#04x} -> {value:#04x}: {whole:?} -> {left:?}",
                    heap[at]
                ));
            }
            out.machines += usize::from(!extract(&diagram).machines.is_empty());
            let found = proven(&diagram);
            if found.is_subset(&allowed) {
                continue;
            }
            let list = if skeleton(&diagram) == structure {
                &mut out.revalued
            } else {
                &mut out.added
            };
            for finding in found.difference(&allowed) {
                list.push(format!(
                    "byte {at} {:#04x} -> {value:#04x}: {finding}",
                    heap[at]
                ));
            }
        }
        copy[at] = heap[at];
    }
    out
}

/// One changed byte of a file in the repository that gave a proof: the
/// tag of the field that says which case of the machine's case structure
/// is the default (`26 54 01`: field 565, one byte, frame 1). An absent
/// field means frame 0, so with the tag damaged the default moved from
/// "Idle" to "Init", the empty string that "Init" hands on selected
/// "Init" itself, and "Idle" was "proven" unreachable. The object now
/// has a field that cannot be named, or the same field twice, which is a
/// recorded loss.
#[test]
fn a_damaged_field_tag_in_a_real_file_proves_nothing() {
    use buzzard_rsrc::blocks::zlib;
    use buzzard_rsrc::RsrcFile;

    const AT: usize = 72_333;
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify/jki-smo/process.vi");
    let bytes = std::fs::read(path).unwrap();
    let file = RsrcFile::parse(&bytes).unwrap();
    let mut heap = zlib::decompress(file.first(b"BDHb").unwrap()).unwrap();
    assert_eq!(heap[AT - 1..AT + 2], [0x26, 0x54, 0x01]);
    let mut decode = |at: usize, value: u8| {
        let old = std::mem::replace(&mut heap[at], value);
        let diagram = BlockDiagram::from_inflated_heap(&file, &heap);
        heap[at] = old;
        diagram.unwrap().unwrap()
    };
    assert_eq!(proven(&decode(AT, 0x54)), BTreeSet::new());
    let mut lossy = 0;
    for value in TAGS {
        let diagram = decode(AT, value);
        lossy += usize::from(!diagram.is_complete());
        assert_eq!(extract(&diagram).machines, [], "{value:#04x}");
        assert_eq!(verify(&diagram).issues(), [], "{value:#04x}");
    }

    assert!(lossy >= 2);

    // The value of the field is another matter. With frame 0 written
    // there the file is whole and says that "Init" is the default case:
    // another program, in which "Idle" really is never reached.
    let other = decode(AT + 1, 0x00);
    assert!(other.is_complete());
    assert_eq!(only(&other).default.as_deref(), Some("Init"));
    assert_eq!(only(&other).check().findings, [unreachable("Idle")]);
}

/// Every `step`-th byte of the heap is damaged. Set
/// `BUZZARD_HEAP_DAMAGE_STEP=1` to try every byte, which takes minutes.
fn damage_step(default: usize) -> usize {
    match std::env::var("BUZZARD_HEAP_DAMAGE_STEP") {
        Ok(step) => step.parse().expect("BUZZARD_HEAP_DAMAGE_STEP is a number"),
        Err(_) => default,
    }
}

/// What the tag byte in `a_damaged_field_tag_in_a_real_file_proves_nothing`
/// is changed to.
///
/// `0x55` makes it a second copy of a field the object already has, and
/// `0xff` a field nothing has: both are a recorded loss, and the diagram
/// is not searched. The others make it a field that exists elsewhere but
/// that no case structure has; the selector table is then not trusted to
/// say that frame 0 is the default, so there is no table and no machine.
const TAGS: [u8; 6] = [0x00, 0x44, 0x53, 0x55, 0xd4, 0xff];

const FIXTURES: [&str; 5] = [
    "jki-smo/process.vi",
    "jki-state-machine/queued-state-machine-template.vi",
    "jki-state-machine/new-support-vis-examples.vi",
    "jki-state-machine/parse-state-queue-performance-test.vi",
    "lv-mqtt-broker/test-simple-connection.vi",
];

/// The invariant behind the word "proven": damage to the structure of a
/// diagram may remove findings, but no proven finding appears that the
/// undamaged file does not have. And the property of the decoder that the
/// invariant rests on: whenever a frame, a node, a terminal, a wire or a
/// wire end is gone from the decoded diagram, a loss is on record.
///
/// Damage that only changes a value (see [`skeleton`]) gives a complete
/// diagram of another program; proofs about it are counted and printed,
/// not failed. With every byte tried, `process.vi` has 34 of them: the
/// label of the case "Init" spelled otherwise (the new name is then
/// unreachable) and the number of the default case changed.
#[test]
fn a_damaged_heap_never_adds_a_proven_finding() {
    let step = damage_step(499);
    let mut total = Damage::default();
    for name in FIXTURES {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/verify")
            .join(name);
        let found = damage(&std::fs::read(path).unwrap(), step);
        assert_eq!(found.added, [""; 0], "{name}");
        assert_eq!(found.vanished, [""; 0], "{name}");
        total.trials += found.trials;
        total.decoded += found.decoded;
        total.incomplete += found.incomplete;
        total.machines += found.machines;
        total.revalued.extend(found.revalued);
    }
    println!(
        "{} damaged heaps, {} decoded, {} of them with a recorded loss, {} still with a machine",
        total.trials, total.decoded, total.incomplete, total.machines
    );
    println!(
        "{} proofs about a diagram that differs in a value only: {:#?}",
        total.revalued.len(),
        total.revalued
    );
    // The test means something only if damage is noticed and some
    // damaged files still give a machine to make claims about.
    assert!(total.decoded > 1_000 && total.incomplete > 100 && total.machines > 100);
}
