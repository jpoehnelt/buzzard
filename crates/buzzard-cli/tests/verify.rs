//! Black-box tests of `buzzard verify`, and of the state machine rules in
//! `buzzard check`, on the real files under
//! `crates/buzzard-lint/tests/fixtures/verify` (its README records their
//! sources and licenses).

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::*;
use serde_json::Value;

fn machines() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../buzzard-lint/tests/fixtures/verify")
}

fn run(args: &[&str], dir: &Path) -> (i32, String, String) {
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run the buzzard binary");
    (
        status.code().expect("buzzard exits with a status"),
        String::from_utf8(stdout).expect("stdout is UTF-8"),
        String::from_utf8(stderr).expect("stderr is UTF-8"),
    )
}

#[test]
fn a_machine_without_findings_is_described_and_exits_zero() {
    let (code, stdout, stderr) = run(&["verify"], &machines().join("jki-smo"));
    assert_eq!(stderr, "");
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "process.vi\n\
         \x20 state machine at while loop #3379, case structure #3023\n\
         \x20   states (2): Init, Idle\n\
         \x20   initial state: Init\n\
         \x20   default state: Idle\n\
         \x20   model: over-approximation (0 of 2 states with a next state that could not be \
         followed, 2 with a stop condition that could not be read)\n\
         \x20   transitions:\n\
         \x20     Init -> Idle; may stop the loop\n\
         \x20     Idle -> Idle, Init; may stop the loop\n\
         \n\
         Checked 1 file: 1 with a state machine (1 state machine, 0 string/queue machines), \
         0 without.\n\
         No findings.\n"
    );
}

#[test]
fn findings_are_labelled_justified_and_exit_one() {
    let dir = machines().join("jki-state-machine");
    let (code, stdout, stderr) = run(&["verify", "."], &dir);
    assert_eq!(stderr, "");
    assert_eq!(code, 1);
    let why = "      why: the name appears in a string constant wired to the queue and matches \
               no case label; names built at run time or enqueued elsewhere are not seen\n";
    let finding = |name: &str| {
        format!(
            "    HEURISTIC [BZ203] state `{name}` is enqueued but no case handles it; the \
             default case `Default` runs instead\n{why}"
        )
    };
    let expected = format!(
        "new-support-vis-examples.vi\n\
         \x20 string/queue state machine at while loop #70, case structure #177 (recognized, \
         not modeled)\n\
         \x20   cases: 21, default case Default; 13 enqueued names found in constants\n\
         {}{}{}\n\
         queued-state-machine-template.vi\n\
         \x20 string/queue state machine at while loop #117, case structure #5 (recognized, \
         not modeled)\n\
         \x20   cases: 21, default case Default; 9 enqueued names found in constants\n\
         \n\
         Checked 3 files: 2 with a state machine (0 state machines, 2 string/queue machines), \
         1 without.\n\
         3 findings: 0 proven, 0 structural, 3 heuristic.\n",
        finding("Action: Clean up after doing work"),
        finding("Action: Step 1"),
        finding("Action: Step 2"),
    );
    assert_eq!(stdout, expected);

    // With one file given by name, the file without a machine is only
    // counted.
    let (code, stdout, _) = run(&["verify", "parse-state-queue-performance-test.vi"], &dir);
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "Checked 1 file: 0 with a state machine (0 state machines, 0 string/queue machines), \
         1 without.\nNo findings.\n"
    );
}

#[test]
fn json_carries_the_model_and_the_findings() {
    let (code, stdout, _) = run(&["verify", ".", "--format", "json"], &machines());
    assert_eq!(code, 1);
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        report["summary"],
        serde_json::json!({
            "files": 5,
            "files_with_machines": 3,
            "files_without_machines": 2,
            "machines": 1,
            "queue_machines": 2,
            "incomplete": 0,
            "findings": 3,
            "proven": 0,
            "structural": 0,
            "heuristic": 3,
        })
    );
    assert_eq!(report["unreadable"], serde_json::json!([]));
    assert_eq!(report["incomplete"], serde_json::json!([]));
    let files = report["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);

    let classic = &files[0];
    assert!(classic["path"].as_str().unwrap().ends_with("process.vi"));
    let machine = &classic["machines"][0];
    assert_eq!(
        machine["machine"]["location"],
        "while loop #3379, case structure #3023"
    );
    assert_eq!(machine["machine"]["initial"], serde_json::json!(["Init"]));
    assert_eq!(machine["machine"]["default"], "Idle");
    assert_eq!(machine["exact"], false);
    assert_eq!(
        machine["machine"]["states"][1],
        serde_json::json!({
            "name": "Idle",
            "successors": { "known": ["Idle", "Init"] },
            "unmatched": [],
            "stop": "maybe",
        })
    );
    assert_eq!(
        machine["transitions"],
        serde_json::json!([["Idle", "Idle"], ["Idle", "Init"], ["Init", "Idle"]])
    );
    assert_eq!(classic["findings"], serde_json::json!([]));

    let finding = &files[1]["findings"][1];
    assert_eq!(finding["code"], "BZ203");
    assert_eq!(finding["confidence"], "heuristic");
    assert_eq!(finding["kind"], "unhandled_queue_state");
    assert_eq!(finding["name"], "Action: Step 1");
    assert_eq!(finding["handled_by"], "Default");
    assert_eq!(finding["location"], "while loop #70, case structure #177");
    assert!(finding["message"]
        .as_str()
        .unwrap()
        .starts_with("state `Action: Step 1` is enqueued"));
    assert!(finding["justification"]
        .as_str()
        .unwrap()
        .contains("matches no case label"));
    assert_eq!(
        files[1]["queue_machines"][0]["unhandled"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn dot_draws_the_transition_graph() {
    let (code, stdout, _) = run(&["verify", "--format", "dot"], &machines().join("jki-smo"));
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "digraph state_machines {\n\
         \x20 rankdir=LR;\n\
         \x20 node [shape=box];\n\
         \x20 subgraph cluster_0 {\n\
         \x20   label=\"process.vi: while loop #3379, case structure #3023\";\n\
         \x20   m0_s0 [label=\"Init\"];\n\
         \x20   m0_s1 [label=\"Idle\"];\n\
         \x20   m0_start [shape=point];\n\
         \x20   m0_start -> m0_s0;\n\
         \x20   m0_s1 -> m0_s1;\n\
         \x20   m0_s1 -> m0_s0;\n\
         \x20   m0_s0 -> m0_s1;\n\
         \x20 }\n\
         }\n"
    );
    // Queue machines are not drawn, but their findings still set the status.
    let (code, stdout, _) = run(
        &["verify", "--format", "dot"],
        &machines().join("jki-state-machine"),
    );
    assert_eq!(code, 1);
    assert_eq!(
        stdout,
        "digraph state_machines {\n  rankdir=LR;\n  node [shape=box];\n}\n"
    );
}

#[test]
fn output_is_byte_identical_across_runs() {
    for format in ["text", "json", "dot"] {
        let first = run(&["verify", ".", "--format", format], &machines());
        assert_eq!(first.0, 1);
        for _ in 0..3 {
            assert_eq!(
                run(&["verify", ".", "--format", format], &machines()),
                first,
                "{format}"
            );
        }
    }
}

#[test]
fn errors_exit_two_and_unreadable_files_are_listed() {
    let (code, stdout, stderr) = run(&["verify", "no-such-file.vi"], &machines());
    assert_eq!(code, 2);
    assert_eq!(stdout, "");
    assert!(stderr.starts_with("error: path does not exist: no-such-file.vi"));
    let (code, _, _) = run(&["verify", "--format", "xml"], &machines());
    assert_eq!(code, 2);

    // A file that is not an RSRC container is listed, like `BZ000` in
    // `check`, and is not a finding about a state machine.
    let dir = TempDir::new("verify-unreadable");
    std::fs::write(dir.0.join("broken.vi"), b"not a resource file").unwrap();
    dir.vi("plain.vi", &CLEAN, &[]);
    let (code, stdout, _) = run(&["verify"], &dir.0);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("broken.vi: cannot analyze file: "));
    assert!(stdout.contains(
        "Checked 0 files: 0 with a state machine (0 state machines, 0 string/queue machines), \
         0 without.\n1 file could not be read.\nNo findings.\n"
    ));
}

#[test]
fn check_reports_the_state_machine_rules() {
    let dir = machines().join("jki-state-machine");
    let (code, stdout, _) = run(&["check", "new-support-vis-examples.vi"], &dir);
    assert_eq!(code, 1);
    for name in [
        "Action: Clean up after doing work",
        "Action: Step 1",
        "Action: Step 2",
    ] {
        assert!(stdout.contains(&format!(
            "new-support-vis-examples.vi: warning[BZ203]: string/queue state machine at while \
             loop #70, case structure #177: state `{name}` is enqueued but no case handles it; \
             the default case `Default` runs instead (heuristic, not proven)\n"
        )));
    }
    let (_, ignored, _) = run(
        &["check", "new-support-vis-examples.vi", "--ignore", "BZ203"],
        &dir,
    );
    assert!(!ignored.contains("BZ203"));

    // SARIF names the rules and carries the findings.
    let (_, sarif, _) = run(
        &["check", "new-support-vis-examples.vi", "--format", "sarif"],
        &dir,
    );
    let sarif: Value = serde_json::from_str(&sarif).unwrap();
    let run = &sarif["runs"][0];
    let rules: Vec<&str> = run["tool"]["driver"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule["id"].as_str().unwrap())
        .collect();
    for code in ["BZ201", "BZ202", "BZ203"] {
        assert!(rules.contains(&code), "{rules:?}");
    }
    let found = run["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|result| result["ruleId"] == "BZ203")
        .count();
    assert_eq!(found, 3);
}

/// Runs `verify` over a large directory of real files and checks that it
/// exits with status 0 or 1 and prints the same bytes on every run. Run it
/// with `BUZZARD_CORPUS=<dir> cargo test -p buzzard-cli --test verify --
/// --ignored`.
#[test]
#[ignore = "needs BUZZARD_CORPUS to point at a directory of real files"]
fn corpus_run_is_deterministic() {
    let corpus =
        PathBuf::from(std::env::var_os("BUZZARD_CORPUS").expect("BUZZARD_CORPUS is not set"));
    for format in ["text", "json", "dot"] {
        let first = run(&["verify", ".", "--format", format], &corpus);
        assert!(first.0 == 0 || first.0 == 1, "{}", first.2);
        assert_eq!(first.2, "");
        assert_eq!(
            run(&["verify", ".", "--format", format], &corpus),
            first,
            "{format}"
        );
    }
}
