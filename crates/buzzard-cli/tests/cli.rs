//! Black-box tests of the `buzzard` binary.

mod common;

use std::process::{Command, Output};

use common::*;

fn run(args: &[&str], dir: &std::path::Path) -> (i32, String, String) {
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    (
        status.code().unwrap(),
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

#[test]
fn check_clean_tree_exits_zero() {
    let dir = TempDir::new("clean");
    dir.vi("a.vi", &CLEAN, &["b.vi"]);
    dir.vi("b.vi", &CLEAN, &[]);
    let (code, stdout, _) = run(&["check"], &dir.0);
    assert_eq!(
        (code, stdout.as_str()),
        (0, "No problems found in 2 files.\n")
    );
}

#[test]
fn check_reports_problems_and_exits_one() {
    let dir = TempDir::new("dirty");
    dir.vi("a.vi", &CLEAN, &["b.vi"]);
    dir.vi("b.vi", &INLINE_CODE, &["a.vi"]);
    let (code, stdout, _) = run(&["check", "."], &dir.0);
    assert_eq!(code, 1);
    assert_eq!(
        stdout,
        "a.vi: error[BZ005]: part of a circular dependency: a.vi -> b.vi -> a.vi\n\
         b.vi: warning[BZ001]: compiled code is not separated from the source file; \
         enable \"Separate compiled code from source file\"\n\
         b.vi: error[BZ005]: part of a circular dependency: a.vi -> b.vi -> a.vi\n\
         Found 3 problems (2 errors, 1 warning) in 2 files.\n"
    );

    let (code, stdout, _) = run(&["check", "--ignore", "BZ005", "--ignore", "BZ001"], &dir.0);
    assert_eq!(
        (code, stdout.as_str()),
        (0, "No problems found in 2 files.\n")
    );

    let (code, _, _) = run(&["check", "--exit-zero"], &dir.0);
    assert_eq!(code, 0);
}

#[test]
fn check_output_is_byte_identical_across_runs() {
    let fixtures = fixtures();
    for format in ["human", "json", "sarif"] {
        let first = run(&["check", ".", "--format", format], &fixtures);
        assert_eq!(first.0, 1);
        for _ in 0..3 {
            assert_eq!(
                run(&["check", ".", "--format", format], &fixtures),
                first,
                "{format}"
            );
        }
    }
}

#[test]
fn check_json_and_sarif_are_valid() {
    let fixtures = fixtures();
    let (_, json, _) = run(
        &["check", "add_breakpoints.vi", "--format", "json"],
        &fixtures,
    );
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let codes: Vec<&str> = parsed
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["BZ001", "BZ004"]);
    assert_eq!(parsed[1]["severity"], "warning");
    assert_eq!(parsed[1]["path"], "add_breakpoints.vi");

    let (_, sarif, _) = run(
        &["check", "add_breakpoints.vi", "--format", "sarif"],
        &fixtures,
    );
    let parsed: serde_json::Value = serde_json::from_str(&sarif).unwrap();
    assert_eq!(parsed["version"], "2.1.0");
    assert_eq!(parsed["runs"][0]["results"].as_array().unwrap().len(), 2);
}

#[test]
fn check_rejects_bad_arguments() {
    let dir = TempDir::new("args");
    let (code, _, stderr) = run(&["check", "nope"], &dir.0);
    assert_eq!(code, 2);
    assert!(stderr.contains("path does not exist"), "{stderr}");
    let (code, _, stderr) = run(&["check", "--ignore", "BZ999"], &dir.0);
    assert_eq!(code, 2);
    assert!(stderr.contains("unknown rule code"), "{stderr}");
}

#[test]
fn inspect_prints_markdown_and_json() {
    let fixtures = fixtures();
    let (code, markdown, _) = run(&["inspect", "relative_link.vi"], &fixtures);
    assert_eq!(code, 0);
    assert!(markdown.starts_with("### relative_link.vi\n"), "{markdown}");
    assert!(markdown.contains("| Saved version | 21.0.0 |"));
    assert!(markdown.contains("- `../empty.vi`"));

    let (code, json, _) = run(
        &["inspect", "empty_password.vi", "--format", "json"],
        &fixtures,
    );
    assert_eq!(code, 0);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["password_protected"], true);
    assert_eq!(parsed["version"]["major"], 21);

    let dir = TempDir::new("inspect");
    std::fs::write(dir.0.join("bad.vi"), b"nope").unwrap();
    let (code, _, stderr) = run(&["inspect", "bad.vi"], &dir.0);
    assert_eq!(code, 2);
    assert!(stderr.contains("too small"), "{stderr}");
}
