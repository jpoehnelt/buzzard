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

    // The block diagram is part of both formats.
    let (_, markdown, _) = run(&["inspect", "add.vi"], &fixtures);
    assert!(
        markdown.contains("## Block diagram\n\n2 nodes, 3 wires, 1 diagrams.\n"),
        "{markdown}"
    );
    assert!(markdown.contains("- built-in function #43\n"), "{markdown}");
    assert!(
        markdown.contains("  - out #51 numeric -> diagram boundary #7 (terminal #162)\n"),
        "{markdown}"
    );
    let (_, markdown, _) = run(&["inspect", "relative_link.vi"], &fixtures);
    assert!(markdown.contains("- subVI `empty.vi` #43\n"), "{markdown}");

    let (_, json, _) = run(&["inspect", "add.vi", "--format", "json"], &fixtures);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["kind"], "vi");
    assert_eq!(parsed["diagram"]["root"]["nodes"][1]["class"], "prim");
    assert_eq!(
        parsed["diagram"]["root"]["wires"].as_array().unwrap().len(),
        3
    );

    // Pictures of the diagram.
    let (code, svg, _) = run(&["inspect", "add.vi", "--format", "svg"], &fixtures);
    assert_eq!(code, 0);
    assert!(svg.starts_with("<svg ") && svg.trim_end().ends_with("</svg>"));
    let (_, mermaid, _) = run(&["inspect", "add.vi", "--format", "mermaid"], &fixtures);
    assert!(mermaid.starts_with("flowchart LR\n"));
    let (_, dot, _) = run(&["inspect", "add.vi", "--format", "dot"], &fixtures);
    assert!(dot.starts_with("digraph diagram {\n"));

    let dir = TempDir::new("inspect");
    std::fs::write(dir.0.join("bad.vi"), b"nope").unwrap();
    let (code, _, stderr) = run(&["inspect", "bad.vi"], &dir.0);
    assert_eq!(code, 2);
    assert!(stderr.contains("too small"), "{stderr}");
}

#[test]
fn check_and_inspect_cover_project_files() {
    let dir = TempDir::new("project");
    dir.vi("Run.vi", &CLEAN, &[]);
    std::fs::write(
        dir.0.join("Kit.lvlib"),
        "<Library LVVersion=\"23008000\">\n\
         <Item Name=\"Run.vi\" Type=\"VI\" URL=\"../Run.vi\"/>\n\
         <Item Name=\"Lost.vi\" Type=\"VI\" URL=\"../Lost.vi\"/>\n</Library>\n",
    )
    .unwrap();
    let (code, stdout, _) = run(&["check"], &dir.0);
    assert_eq!(code, 1);
    assert_eq!(
        stdout,
        "Kit.lvlib: warning[BZ006]: member `Lost.vi` does not exist on disk (`../Lost.vi`)\n\
         Found 1 problem (0 errors, 1 warning) in 2 files.\n"
    );
    let (code, stdout, _) = run(&["check", "--ignore", "BZ006"], &dir.0);
    assert_eq!(
        (code, stdout.as_str()),
        (0, "No problems found in 2 files.\n")
    );

    let (code, markdown, _) = run(&["inspect", "Kit.lvlib"], &dir.0);
    assert_eq!(code, 0);
    assert!(markdown.starts_with("### Kit.lvlib\n"), "{markdown}");
    assert!(markdown.contains("| Kind | Library |"), "{markdown}");
    assert!(markdown.contains("| Members | 2 |"), "{markdown}");
    let (code, json, _) = run(&["inspect", "Kit.lvlib", "--format", "json"], &dir.0);
    assert_eq!(code, 0);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["kind"], "library");
    assert_eq!(parsed["lv_version"], "23.0.0");
    assert_eq!(parsed["members"][1]["url"], "../Lost.vi");

    std::fs::write(dir.0.join("Bad.lvproj"), "<Project>").unwrap();
    let (code, _, stderr) = run(&["inspect", "Bad.lvproj"], &dir.0);
    assert_eq!(code, 2);
    assert!(stderr.contains("invalid XML"), "{stderr}");
}
