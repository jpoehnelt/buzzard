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
        .expect("run the buzzard binary");
    (
        status.code().expect("buzzard exits with a status code"),
        String::from_utf8(stdout).expect("stdout is UTF-8"),
        String::from_utf8(stderr).expect("stderr is UTF-8"),
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
    assert!(
        markdown.contains("- built-in function `Add` #43\n"),
        "{markdown}"
    );
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

    // The svg, mermaid, and dot formats draw the diagram.
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

/// Every `http://` or `https://` address in `text`.
fn addresses(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    for (at, _) in text.match_indices("http") {
        let rest = &text[at..];
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let end = rest
                .find(['"', '\'', ' ', '<', '>', ')'])
                .unwrap_or(rest.len());
            found.push(&rest[..end]);
        }
    }
    found
}

#[test]
fn inspect_prints_an_html_report() {
    let fixtures = fixtures();
    let (code, html, stderr) = run(&["inspect", "add.vi", "--format", "html"], &fixtures);
    assert_eq!((code, stderr.as_str()), (0, ""));
    assert!(html.starts_with("<!DOCTYPE html>\n"), "{html}");
    assert!(html.trim_end().ends_with("</html>"));
    assert_eq!(html.matches("<svg ").count(), 1);
    assert_eq!(html.matches("</svg>").count(), 1);
    let bytes = std::fs::read(fixtures.join("add.vi")).unwrap();
    assert!(
        html.contains(&format!(
            "SHA-256 <code class=\"bz-code\">{}</code>",
            sha256(&bytes)
        )),
        "{html}"
    );
    assert!(html.contains("<title>add.vi - buzzard</title>"));
    assert!(
        html.contains(
            "<tr><th scope=\"row\">Kind</th><td>VI (<code class=\"bz-code\">LVIN</code>)</td></tr>"
        ),
        "{html}"
    );
    assert!(html.contains("<tr><th scope=\"row\">Saved version</th><td>21.0.0</td></tr>"));
    assert!(html.contains("<h3 class=\"bz-subheading\">Dependencies (0)</h3>"));
    assert!(
        html.contains("2 nodes, 6 terminals, 3 wires, 1 diagram.</p>"),
        "{html}"
    );
    // The metadata is a table, not Markdown text.
    assert!(!html.contains("| --- |"));
    assert!(!html.contains("<pre"));
    assert!(html.contains("The diagram decoded completely."));
    // The page loads nothing and links to nothing. The one address is the
    // namespace of the SVG.
    assert_eq!(addresses(&html), ["http://www.w3.org/2000/svg"]);
    for tag in ["<script", "<link", "<img", "<iframe", "src=", "href="] {
        assert!(!html.contains(tag), "{tag}");
    }
    // No absolute path: only the path as it was passed.
    let absolute = fixtures.canonicalize().unwrap();
    assert!(!html.contains(&*absolute.to_string_lossy()));
}

#[test]
fn the_html_report_is_byte_identical_across_runs() {
    let fixtures = fixtures();
    let first = run(&["inspect", "add.vi", "--format", "html"], &fixtures);
    for _ in 0..3 {
        assert_eq!(
            run(&["inspect", "add.vi", "--format", "html"], &fixtures),
            first
        );
    }
}

#[test]
fn the_html_report_of_a_file_without_a_diagram_says_so() {
    let dir = TempDir::new("html-no-diagram");
    dir.vi("a.vi", &CLEAN, &[]);
    let (code, html, stderr) = run(&["inspect", "a.vi", "--format", "html"], &dir.0);
    assert_eq!((code, stderr.as_str()), (0, ""));
    assert!(html.contains("buzzard decoded no block diagram from this file."));
    assert!(!html.contains("<svg"));
    assert!(html.contains("No problems found."), "{html}");
    let bytes = std::fs::read(dir.0.join("a.vi")).unwrap();
    assert!(html.contains(&sha256(&bytes)));
}

#[test]
fn the_html_report_lists_the_problems_of_the_file_checked_alone() {
    let dir = TempDir::new("html-alone");
    dir.vi("a.vi", &CLEAN, &["b.vi"]);
    dir.vi("b.vi", &INLINE_CODE, &["a.vi"]);
    // Checked together, the two files make a cycle.
    let (_, together, _) = run(&["check", "."], &dir.0);
    assert!(together.contains("b.vi: error[BZ005]"), "{together}");
    let (code, html, _) = run(&["inspect", "b.vi", "--format", "html"], &dir.0);
    assert_eq!(code, 0);
    assert!(
        html.contains("Problems found when this file is checked alone"),
        "{html}"
    );
    assert!(html.contains("(BZ002, BZ005, BZ006)"));
    assert!(
        html.contains(
            "<span class=\"bz-warning\">warning[BZ001]</span> <code class=\"bz-code\">b.vi</code>: \
             compiled code is not separated from the source file; enable \
             &quot;Separate compiled code from source file&quot;</li>"
        ),
        "{html}"
    );
    // Alone, the file is in no cycle.
    assert!(!html.contains("BZ005]"), "{html}");
    assert_eq!(html.matches("<li class=\"bz-finding\">").count(), 1);
}

/// Run with stdout connected to a device that accepts no data, like a full
/// disk. Returns the exit status and stderr.
#[cfg(target_os = "linux")]
fn run_with_full_stdout(args: &[&str], dir: &std::path::Path) -> (i32, String) {
    let full = std::fs::File::options()
        .write(true)
        .open("/dev/full")
        .expect("open /dev/full");
    let output = Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .stdout(full)
        .output()
        .expect("run the buzzard binary");
    (
        output
            .status
            .code()
            .expect("buzzard exits with a status code"),
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
    )
}

/// A report that could not be written is a failure, because a CI step must
/// not pass with an empty or truncated SARIF file.
#[cfg(target_os = "linux")]
#[test]
fn failing_to_write_the_report_exits_two() {
    let dir = TempDir::new("full");
    dir.vi("a.vi", &CLEAN, &[]);
    for args in [
        &["check"][..],
        &["check", "--format", "json"],
        &["check", "--format", "sarif"],
        &["check", "--exit-zero"],
        &["inspect", "a.vi"],
        &["inspect", "a.vi", "--format", "json"],
    ] {
        let (code, stderr) = run_with_full_stdout(args, &dir.0);
        assert_eq!(code, 2, "{args:?}: {stderr}");
        assert!(
            stderr.starts_with("error: cannot write output: "),
            "{args:?}: {stderr}"
        );
    }
    std::fs::write(dir.0.join("App.lvproj"), "<Project/>").unwrap();
    let (code, stderr) = run_with_full_stdout(&["inspect", "App.lvproj"], &dir.0);
    assert_eq!(code, 2, "{stderr}");

    // The write failure also replaces the status 1 of a run that found
    // problems.
    dir.vi("b.vi", &INLINE_CODE, &[]);
    let (code, stderr) = run_with_full_stdout(&["check"], &dir.0);
    assert_eq!(code, 2, "{stderr}");
}

/// A reader that stops reading early (`buzzard check | head`) is not an
/// error.
#[cfg(unix)]
#[test]
fn closed_pipe_is_not_an_error() {
    use std::process::Stdio;

    let dir = TempDir::new("pipe");
    for i in 0..200 {
        dir.vi(&format!("f{i:03}.vi"), &INLINE_CODE, &[]);
    }
    let mut child = Command::new(BIN)
        .args(["check", "--exit-zero", "--format", "json"])
        .current_dir(&dir.0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Close the read end before the report is written.
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(stderr, "");
}
