//! Black-box tests of what a program that drives `buzzard` depends on:
//! errors as JSON, limits on the size of a report, the list of rules, and
//! the checks on path arguments.

mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use buzzard_lint::{ErrorKind, RULES};
use common::*;
use serde_json::Value;

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
        status.code().expect("buzzard exits with a status code"),
        String::from_utf8(stdout).expect("stdout is UTF-8"),
        String::from_utf8(stderr).expect("stderr is UTF-8"),
    )
}

/// A directory with a clean VI, a project that lists it, two files that do
/// not parse, and nothing named `nope`.
fn workspace(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    dir.vi("a.vi", &CLEAN, &[]);
    fs::write(dir.0.join("bad.vi"), b"nope").expect("write the file");
    fs::write(dir.0.join("Bad.lvproj"), "<Project>").expect("write the file");
    fs::write(
        dir.0.join("Good.lvproj"),
        "<Project LVVersion=\"23008000\">\n<Item Name=\"a.vi\" Type=\"VI\" URL=\"a.vi\"/>\n\
         </Project>\n",
    )
    .expect("write the file");
    dir
}

/// The commands that end with status 2, each with a machine format, the
/// kind of its error, and the path the error is about.
const FAILURES: &[(&[&str], &str, Option<&str>)] = &[
    (
        &["check", "nope", "--format", "json"],
        "path_not_found",
        Some("nope"),
    ),
    (
        &["check", "nope", "--format", "sarif"],
        "path_not_found",
        Some("nope"),
    ),
    (
        &["check", "--format=json", "--ignore", "BZ999"],
        "unknown_rule",
        None,
    ),
    (&["check", "--bogus", "--format", "json"], "usage", None),
    (
        &["check", "--format", "sarif", "--limit", "many"],
        "usage",
        None,
    ),
    (
        &["inspect", "nope.vi", "--format", "json"],
        "path_not_found",
        Some("nope.vi"),
    ),
    (
        &["inspect", "bad.vi", "--format", "json"],
        "unreadable_file",
        Some("bad.vi"),
    ),
    (
        &["inspect", "Bad.lvproj", "--format", "json"],
        "unreadable_file",
        Some("Bad.lvproj"),
    ),
    (&["inspect", "--format", "json"], "usage", None),
    (
        &["verify", "nope", "--format", "json"],
        "path_not_found",
        Some("nope"),
    ),
    (
        &["deps", "a.vi", "nope", "--format", "json"],
        "path_not_found",
        Some("nope"),
    ),
    (
        &["deps", "--root", "vilib", "--format", "json"],
        "bad_argument",
        None,
    ),
    (
        &["deps", "--detail", "all", "--format", "json"],
        "usage",
        None,
    ),
    (&["diff", "a.vi", "--format", "json"], "usage", None),
    (
        &["diff", "nope.vi", "nope2.vi", "--format", "json"],
        "path_not_found",
        None,
    ),
    (
        &["diff", "bad.vi", "a.vi", "--format", "json"],
        "unreadable_file",
        Some("bad.vi"),
    ),
    (
        &["tdms", "info", "nope.tdms", "--format", "json"],
        "path_not_found",
        Some("nope.tdms"),
    ),
    (
        &["tdms", "info", "bad.vi", "--format", "json"],
        "unreadable_file",
        Some("bad.vi"),
    ),
    (
        &[
            "tdms",
            "export",
            "nope.tdms",
            "--out",
            "out.csv",
            "--format",
            "csv",
            "--json",
        ],
        "path_not_found",
        Some("nope.tdms"),
    ),
    (&["tdms", "export", "--json"], "usage", None),
    // A directory where one file is expected is the same mistake in every
    // command, and on the MCP server.
    (
        &["inspect", ".", "--format", "json"],
        "bad_argument",
        Some("."),
    ),
    (
        &["tdms", "info", ".", "--format", "json"],
        "bad_argument",
        Some("."),
    ),
    // The format of the error is asked for apart from that of the report.
    (
        &[
            "inspect",
            "Bad.lvproj",
            "--format",
            "svg",
            "--error-format",
            "json",
        ],
        "unreadable_file",
        Some("Bad.lvproj"),
    ),
    (
        &["inspect", "a.vi", "--format", "dot", "--error-format=json"],
        "unsupported",
        Some("a.vi"),
    ),
    // A project file has no block diagram, so it has no HTML report.
    (
        &[
            "inspect",
            "Good.lvproj",
            "--format",
            "html",
            "--error-format",
            "json",
        ],
        "unsupported",
        Some("Good.lvproj"),
    ),
    (
        &[
            "--error-format",
            "json",
            "verify",
            "nope",
            "--format",
            "dot",
        ],
        "path_not_found",
        Some("nope"),
    ),
    (
        &["inspect", "nope.vi", "--error-format", "json"],
        "path_not_found",
        Some("nope.vi"),
    ),
    (
        &[
            "tdms",
            "export",
            "nope.tdms",
            "--out",
            "out.csv",
            "--error-format",
            "json",
        ],
        "path_not_found",
        Some("nope.tdms"),
    ),
    (
        &["mcp", "--root", "nope", "--error-format", "json"],
        "bad_argument",
        Some("nope"),
    ),
    (&["guide", "extra", "--error-format", "json"], "usage", None),
    (
        &[
            "inspect",
            "a.vi",
            "--structure",
            "1",
            "--frame",
            "0",
            "--error-format",
            "json",
        ],
        "bad_argument",
        None,
    ),
    (&["rules", "--format", "json", "extra"], "usage", None),
    (&["nothing", "--format", "json"], "usage", None),
];

/// The words of a command line without the ones that ask for a machine
/// format.
fn without_machine_format<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut kept = Vec::new();
    let mut words = args.iter();
    while let Some(&word) = words.next() {
        match word {
            "--json" | "--format=json" | "--error-format=json" => {}
            "--error-format" => {
                words.next();
            }
            "--format" => match words.next() {
                Some(&"json" | &"sarif") | None => {}
                Some(&other) => kept.extend(["--format", other]),
            },
            _ => kept.push(word),
        }
    }
    kept
}

#[test]
fn an_error_is_one_json_object_when_a_machine_format_was_asked() {
    let dir = workspace("agent-errors");
    let known: Vec<&str> = ErrorKind::ALL.iter().map(|kind| kind.as_str()).collect();
    for &(args, kind, path) in FAILURES {
        let (code, stdout, stderr) = run(args, &dir.0);
        assert_eq!(code, 2, "{args:?}: {stderr}");
        // One object on one line, and nothing else.
        assert_eq!(stdout.matches('\n').count(), 1, "{args:?}: {stdout}");
        assert!(stdout.ends_with("}\n"), "{args:?}: {stdout}");
        let parsed: Value = serde_json::from_str(&stdout).expect("stdout is JSON");
        let object = parsed.as_object().expect("an object");
        assert_eq!(object.keys().collect::<Vec<_>>(), ["error"], "{args:?}");
        let error = parsed["error"].as_object().expect("an object");
        assert_eq!(error["kind"], kind, "{args:?}: {stdout}");
        assert!(known.contains(&kind), "{kind}");
        assert_eq!(error.get("path").and_then(Value::as_str), path, "{args:?}");
        let expected = if path.is_some() {
            vec!["kind", "message", "path"]
        } else {
            vec!["kind", "message"]
        };
        assert_eq!(error.keys().collect::<Vec<_>>(), expected, "{args:?}");
        // The prose is still on stderr, and it is the message. The message
        // of a usage error also holds the list that follows the first
        // line.
        let message = error["message"].as_str().expect("a string");
        let first = stderr.lines().next().expect("a line on stderr");
        let prose = first.strip_prefix("error: ").expect("the line of an error");
        if kind == "usage" {
            assert!(message.starts_with(prose), "{args:?}: {stderr}");
        } else {
            assert_eq!(message, prose, "{args:?}");
        }

        // Without the machine format the same error prints nothing on
        // stdout, as before.
        let plain = without_machine_format(args);
        let (code, stdout, stderr) = run(&plain, &dir.0);
        assert_eq!((code, stdout.as_str()), (2, ""), "{plain:?}: {stderr}");
        assert!(stderr.starts_with("error: "), "{plain:?}: {stderr}");
    }
    assert!(!dir.0.join("out.csv").exists());
}

/// The object of a usage error names what is missing, which clap prints on
/// the lines after the first.
#[test]
fn a_usage_error_names_the_arguments_that_are_missing() {
    let dir = workspace("agent-usage");
    let (code, stdout, _) = run(&["inspect", "--format", "json"], &dir.0);
    assert_eq!(code, 2);
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    assert_eq!(
        parsed["error"]["message"],
        "the following required arguments were not provided: <FILE>"
    );
    let (_, stdout, _) = run(&["tdms", "export", "--json"], &dir.0);
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    assert_eq!(
        parsed["error"]["message"],
        "the following required arguments were not provided: --out <PATH>, <FILE>"
    );
}

/// `--error-format text` is the default and changes nothing, and
/// `--error-format json` changes nothing of a command that succeeds.
#[test]
fn the_error_format_changes_only_how_an_error_is_printed() {
    let dir = workspace("agent-error-format");
    let plain = run(&["inspect", "a.vi"], &dir.0);
    assert_eq!(plain.0, 0);
    assert_eq!(
        run(&["inspect", "a.vi", "--error-format", "json"], &dir.0),
        plain
    );
    assert_eq!(
        run(&["inspect", "a.vi", "--error-format", "text"], &dir.0),
        plain
    );
    let (code, stdout, stderr) = run(&["inspect", "nope.vi", "--error-format", "text"], &dir.0);
    assert_eq!((code, stdout.as_str()), (2, ""), "{stderr}");
}

/// `buzzard guide` prints the guide that the MCP server serves as a
/// prompt: the skill file without its front matter.
#[test]
fn guide_prints_the_guide_for_agents() {
    let dir = workspace("agent-guide");
    let (code, stdout, stderr) = run(&["guide"], &dir.0);
    assert_eq!((code, stderr.as_str()), (0, ""));
    assert_eq!(stdout, buzzard_mcp::guide());
    assert!(stdout.starts_with("# "), "{stdout}");
    let skill = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.claude/skills/buzzard/SKILL.md"),
    )
    .expect("read the skill file");
    assert!(skill
        .replace("\r\n", "\n")
        .ends_with(&stdout.replace("\r\n", "\n")));
    assert!(
        stdout.contains("buzzard guide"),
        "the guide names the command"
    );
}

#[test]
fn help_and_version_are_not_errors_in_any_format() {
    let dir = workspace("agent-help");
    for args in [
        &["check", "--format", "json", "--help"][..],
        &["--version", "--format", "json"],
        &["tdms", "export", "--json", "-h"],
    ] {
        let (code, stdout, stderr) = run(args, &dir.0);
        assert_eq!((code, stderr.as_str()), (0, ""), "{args:?}");
        assert!(!stdout.starts_with('{'), "{args:?}: {stdout}");
    }
}

#[test]
fn the_help_documents_the_error_kinds_and_the_size_controls() {
    let dir = workspace("agent-help-text");
    let help = |args: &[&str]| {
        let (code, stdout, _) = run(args, &dir.0);
        assert_eq!(code, 0, "{args:?}");
        stdout
    };
    let top = help(&["--help"]);
    for kind in ErrorKind::ALL {
        assert!(top.contains(&format!("\n  {kind}: ")), "{kind}: {top}");
    }
    assert!(top.contains("{\"error\":{\"kind\""), "{top}");
    for command in [
        "check", "inspect", "deps", "diff", "verify", "tdms", "rules", "lsp", "mcp",
    ] {
        assert!(top.contains(&format!("\n  {command} ")), "{command}: {top}");
        // Every command says what its exit statuses mean.
        if command != "tdms" {
            let text = help(&[command, "--help"]);
            assert!(text.contains("Exit status: "), "{command}: {text}");
        }
    }
    let check = help(&["check", "--help"]);
    assert!(check.contains("--limit <N>"), "{check}");
    assert!(check.contains("control character"), "{check}");
    let inspect = help(&["inspect", "--help"]);
    assert!(inspect.contains("--detail <DETAIL>"), "{inspect}");
    assert!(inspect.contains("over 1 MB"), "{inspect}");
    assert!(help(&["deps", "--help"]).contains("--detail <DETAIL>"));
    let export = help(&["tdms", "export", "--help"]);
    for flag in ["--force", "--dry-run", "--json"] {
        assert!(export.contains(flag), "{flag}: {export}");
    }
    assert!(export.contains("Exit status: "), "{export}");
    assert!(help(&["tdms", "info", "--help"]).contains("Exit status: "));
}

/// A control character in a path argument is the end of a quoting mistake
/// more often than a file name, and it is refused before anything is read.
#[test]
fn a_path_argument_with_a_control_character_is_refused() {
    let dir = workspace("agent-control");
    for bad in [
        "a\tb.vi",
        "a.vi\nerror: forged",
        "\u{1b}[31ma.vi",
        "a\u{7f}.vi",
    ] {
        let root = format!("vilib={bad}");
        let commands: [&[&str]; 10] = [
            &["check", "a.vi", bad],
            &["inspect", bad],
            &["verify", bad],
            &["deps", bad],
            &["deps", "--root", &root],
            &["diff", bad, "a.vi"],
            &["diff", "a.vi", bad],
            &["tdms", "info", bad],
            &["tdms", "export", bad, "--out", "out.csv"],
            &["tdms", "export", "a.vi", "--out", bad],
        ];
        for args in commands {
            let (code, stdout, stderr) = run(args, &dir.0);
            assert_eq!((code, stdout.as_str()), (2, ""), "{args:?}: {stderr}");
            assert!(
                stderr.starts_with("error: the path holds a control character: "),
                "{args:?}: {stderr}"
            );
            // The message is one line, with the character escaped.
            assert_eq!(stderr.matches('\n').count(), 1, "{args:?}: {stderr}");
            assert!(
                !stderr
                    .trim_end_matches('\n')
                    .chars()
                    .any(|c| c.is_ascii_control()),
                "{args:?}: {stderr:?}"
            );
        }
        let (code, stdout, _) = run(&["check", bad, "--format", "json"], &dir.0);
        assert_eq!(code, 2);
        let parsed: Value = serde_json::from_str(&stdout).expect("stdout is JSON");
        assert_eq!(parsed["error"]["kind"], "bad_argument");
        // JSON carries the argument itself.
        assert_eq!(parsed["error"]["path"], bad);
    }
    assert!(!dir.0.join("out.csv").exists());

    // A --root that is no NAME=DIR is repeated in the error, on one line.
    for root in ["vi\nlib", "vi\u{1b}lib=."] {
        let (code, stdout, stderr) = run(&["deps", "--root", root], &dir.0);
        assert_eq!((code, stdout.as_str()), (2, ""), "{root:?}");
        assert!(stderr.starts_with("error: --root: "), "{stderr}");
        assert_eq!(stderr.matches('\n').count(), 1, "{stderr:?}");
        assert!(!stderr.contains('\u{1b}'), "{stderr:?}");
    }

    // An empty path is refused too, by the argument parser.
    let (code, stdout, _) = run(&["check", ""], &dir.0);
    assert_eq!((code, stdout.as_str()), (2, ""));

    // Characters that a URL would decode are letters of a file name here.
    for name in ["q?x=1.vi", "q#frag.vi", "q%2e%2e.vi", "q&[]{}.vi"] {
        let (code, _, stderr) = run(&["check", name, "--format", "human"], &dir.0);
        assert_eq!(code, 2);
        assert_eq!(stderr, format!("error: path does not exist: {name}\n"));
    }
    // `%2e%2e` is a name, not the parent directory.
    fs::create_dir(dir.0.join("%2e%2e")).unwrap();
    dir.vi("%2e%2e/in#side%20.vi", &CLEAN, &[]);
    let (code, stdout, _) = run(&["check", "%2e%2e/in#side%20.vi"], &dir.0);
    assert_eq!(
        (code, stdout.as_str()),
        (0, "No problems found in 1 file.\n")
    );
}

#[test]
fn check_limit_shortens_the_list_and_nothing_else() {
    let dir = TempDir::new("agent-limit");
    dir.vi("a.vi", &CLEAN, &["b.vi"]);
    dir.vi("b.vi", &INLINE_CODE, &["a.vi"]);
    let (code, whole, _) = run(&["check"], &dir.0);
    assert_eq!(code, 1);
    let lines: Vec<&str> = whole.lines().collect();
    assert_eq!(lines.len(), 4, "{whole}");
    let summary = "Found 3 problems (2 errors, 1 warning) in 2 files.";
    assert_eq!(lines[3], summary);

    // The first findings in the order of the whole report, the count of
    // the rest, and the summary of all of them.
    let (code, stdout, _) = run(&["check", "--limit", "1"], &dir.0);
    assert_eq!(code, 1);
    assert_eq!(
        stdout,
        format!("{}\n... 2 more not shown.\n{summary}\n", lines[0])
    );
    let (code, stdout, _) = run(&["check", "--limit", "0"], &dir.0);
    assert_eq!(code, 1, "the status counts what is not shown");
    assert_eq!(stdout, format!("... 3 more not shown.\n{summary}\n"));
    let (code, _, _) = run(&["check", "--limit", "0", "--exit-zero"], &dir.0);
    assert_eq!(code, 0);
    // A limit that leaves nothing out prints the report as it is.
    for limit in ["3", "4", "18446744073709551615"] {
        let (code, stdout, _) = run(&["check", "--limit", limit], &dir.0);
        assert_eq!((code, stdout.as_str()), (1, whole.as_str()), "{limit}");
    }

    let (_, list, _) = run(&["check", "--format", "json"], &dir.0);
    let list: Value = serde_json::from_str(&list).unwrap();
    assert_eq!(list.as_array().map(Vec::len), Some(3));
    for (limit, shown) in [("2", 2), ("0", 0), ("9", 3)] {
        let (code, stdout, _) = run(&["check", "--format", "json", "--limit", limit], &dir.0);
        assert_eq!(code, 1);
        let parsed: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(
            parsed["diagnostics"],
            Value::from(&list.as_array().unwrap()[..shown])
        );
        let summary = &parsed["summary"];
        assert_eq!(summary["files"], 2);
        assert_eq!(summary["problems"], 3);
        assert_eq!(summary["errors"], 2);
        assert_eq!(summary["warnings"], 1);
        assert_eq!(summary["infos"], 0);
        assert_eq!(summary["by_code"]["BZ005"], 2);
        assert_eq!(summary["shown"], shown);
        assert_eq!(summary["not_shown"], 3 - shown);
    }

    let (_, sarif, _) = run(&["check", "--format", "sarif"], &dir.0);
    let (code, limited, _) = run(&["check", "--format", "sarif", "--limit", "1"], &dir.0);
    assert_eq!(code, 1);
    let parsed: Value = serde_json::from_str(&limited).unwrap();
    assert_eq!(
        parsed["runs"][0]["results"].as_array().map(Vec::len),
        Some(1)
    );
    let stated = &parsed["runs"][0]["properties"]["buzzard"];
    assert_eq!(stated["problems"], 3);
    assert_eq!(stated["shown"], 1);
    assert_eq!(stated["not_shown"], 2);
    // Nothing left out: the log is the one without a limit.
    let (_, all, _) = run(&["check", "--format", "sarif", "--limit", "3"], &dir.0);
    assert_eq!(all, sarif);

    // The limited reports are deterministic too.
    for format in ["human", "json", "sarif"] {
        let args = ["check", "--format", format, "--limit", "2"];
        assert_eq!(run(&args, &dir.0), run(&args, &dir.0), "{format}");
    }
}

#[test]
fn rules_lists_every_rule_of_check() {
    let dir = TempDir::new("agent-rules");
    let (code, text, stderr) = run(&["rules"], &dir.0);
    assert_eq!((code, stderr.as_str()), (0, ""));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), RULES.len());
    for (line, rule) in lines.iter().zip(RULES) {
        assert!(line.starts_with(&format!("{} ", rule.code)), "{line}");
    }
    assert_eq!(run(&["rules", "--format", "text"], &dir.0).1, text);

    let (code, json, _) = run(&["rules", "--format", "json"], &dir.0);
    assert_eq!(code, 0);
    let parsed: Value = serde_json::from_str(&json).unwrap();
    let listed = parsed["rules"].as_array().expect("a list");
    assert_eq!(listed.len(), RULES.len());
    for (entry, rule) in listed.iter().zip(RULES) {
        assert_eq!(entry["code"], rule.code);
        assert!(
            matches!(
                entry["severity"].as_str(),
                Some("error" | "warning" | "info")
            ),
            "{entry}"
        );
        assert!(entry["name"].as_str().is_some_and(|name| !name.is_empty()));
        assert!(entry["summary"]
            .as_str()
            .is_some_and(|text| !text.is_empty()));
        // Every listed code is one that `check --ignore` takes.
        let code = entry["code"].as_str().unwrap();
        let (status, _, stderr) = run(&["check", "--ignore", code], &dir.0);
        assert_eq!(status, 0, "{code}: {stderr}");
    }
    assert_eq!(run(&["rules", "--format", "json"], &dir.0).1, json);
}

#[test]
fn inspect_detail_summary_is_the_metadata_and_an_outline() {
    let fixtures = fixtures();
    let (_, full, _) = run(&["inspect", "add.vi", "--format", "json"], &fixtures);
    let explicit = run(
        &["inspect", "add.vi", "--format", "json", "--detail", "full"],
        &fixtures,
    );
    assert_eq!((explicit.0, explicit.1.as_str()), (0, full.as_str()));
    let full_value: Value = serde_json::from_str(&full).unwrap();

    let (code, summary, _) = run(
        &[
            "inspect", "add.vi", "--format", "json", "--detail", "summary",
        ],
        &fixtures,
    );
    assert_eq!(code, 0);
    let parsed: Value = serde_json::from_str(&summary).unwrap();
    assert_eq!(parsed["detail"], "summary");
    // The summary says how large the answer to `--detail full` is.
    assert_eq!(parsed["full_json_bytes"], full.len());
    let outline = &parsed["diagram"];
    assert_eq!(outline["nodes"], 2);
    assert_eq!(outline["wires"], 3);
    assert_eq!(outline["diagrams"], 1);
    assert_eq!(outline["complete"], true);
    assert_eq!(outline["node_classes"]["prim"], 1);
    assert!(outline.get("root").is_none(), "{summary}");
    // The metadata has the keys and values of the full JSON.
    for (key, value) in full_value.as_object().unwrap() {
        if key != "diagram" {
            assert_eq!(&parsed[key], value, "{key}");
        }
    }

    let (code, markdown, _) = run(&["inspect", "add.vi", "--detail", "summary"], &fixtures);
    assert_eq!(code, 0);
    assert!(markdown.starts_with("### add.vi\n"), "{markdown}");
    assert!(
        markdown.contains(
            "## Block diagram\n\n2 nodes, 6 terminals, 3 wires, 1 diagrams.\nNode ids 7 to 43.\n"
        ),
        "{markdown}"
    );
    assert!(markdown.contains("\n### Node classes\n\n"), "{markdown}");
    assert!(markdown.contains("- `prim`: 1\n"), "{markdown}");
    assert!(!markdown.contains("built-in function"), "{markdown}");
    let (_, whole, _) = run(&["inspect", "add.vi"], &fixtures);
    assert_eq!(
        run(&["inspect", "add.vi", "--detail", "full"], &fixtures).1,
        whole
    );

    // A file without a block diagram has a summary without an outline.
    let (code, summary, _) = run(
        &[
            "inspect", "empty.vi", "--format", "json", "--detail", "summary",
        ],
        &fixtures,
    );
    assert_eq!(code, 0);
    let parsed: Value = serde_json::from_str(&summary).unwrap();
    assert_eq!(parsed["detail"], "summary");

    // A picture and the HTML report are always the whole diagram.
    for format in ["svg", "dot", "mermaid", "html"] {
        let (code, stdout, stderr) = run(
            &[
                "inspect", "add.vi", "--format", format, "--detail", "summary",
            ],
            &fixtures,
        );
        assert_eq!((code, stdout.as_str()), (2, ""), "{format}");
        assert!(stderr.contains("--detail summary applies to"), "{stderr}");
    }
}

/// The reason the summary exists: the JSON of one real VI is over a
/// megabyte, and its summary is a small part of that.
#[test]
fn the_summary_of_a_large_diagram_is_small() {
    let dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../buzzard-lint/tests/fixtures/verify/jki-smo");
    let (code, full, _) = run(&["inspect", "process.vi", "--format", "json"], &dir);
    assert_eq!(code, 0);
    assert!(full.len() > 1_000_000, "{}", full.len());
    let args = [
        "inspect",
        "process.vi",
        "--format",
        "json",
        "--detail",
        "summary",
    ];
    let (code, summary, _) = run(&args, &dir);
    assert_eq!(code, 0);
    assert!(summary.len() < 64 * 1024, "{}", summary.len());
    let parsed: Value = serde_json::from_str(&summary).unwrap();
    assert_eq!(parsed["full_json_bytes"], full.len());
    assert!(parsed["diagram"]["structures"]
        .as_array()
        .is_some_and(|s| !s.is_empty()));
    assert_eq!(run(&args, &dir).1, summary);
    let (code, markdown, _) = run(&["inspect", "process.vi", "--detail", "summary"], &dir);
    assert_eq!(code, 0);
    assert!(markdown.len() < 64 * 1024, "{}", markdown.len());
    assert!(markdown.contains("\n### Structures\n\n"), "{markdown}");

    // An archive and a project file have one level of detail.
    let dir = workspace("agent-detail");
    fs::write(dir.0.join("Kit.lvlib"), "<Library LVVersion=\"23008000\"/>").unwrap();
    for format in ["json", "markdown"] {
        let whole = run(&["inspect", "Kit.lvlib", "--format", format], &dir.0);
        assert_eq!(whole.0, 0);
        assert_eq!(
            run(
                &[
                    "inspect",
                    "Kit.lvlib",
                    "--format",
                    format,
                    "--detail",
                    "summary"
                ],
                &dir.0
            ),
            whole
        );
    }
}

/// The step between the outline and the megabyte: one frame, the outline
/// of one frame, or a range of node ids.
#[expect(
    clippy::too_many_lines,
    reason = "one walk from the outline to a frame, its outline, and a range"
)]
#[test]
fn inspect_prints_one_frame_or_a_range_of_nodes() {
    let dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../buzzard-lint/tests/fixtures/verify/jki-smo");
    let json = |more: &[&str]| -> Value {
        let mut args = vec!["inspect", "process.vi", "--format", "json"];
        args.extend(more);
        let (code, stdout, stderr) = run(&args, &dir);
        assert_eq!(code, 0, "{args:?}: {stderr}");
        serde_json::from_str(&stdout).expect("stdout is JSON")
    };
    let full = json(&[]);
    let summary = json(&["--detail", "summary"]);
    let structures = summary["diagram"]["structures"].as_array().unwrap();
    // A frame whose own nodes are few and whose JSON is large says so.
    let (structure, frame) = structures
        .iter()
        .flat_map(|structure| {
            structure["frames"]
                .as_array()
                .unwrap()
                .iter()
                .map(move |frame| (structure, frame))
        })
        .max_by_key(|(_, frame)| frame["all_nodes"].as_u64().unwrap())
        .unwrap();
    assert!(frame["all_nodes"].as_u64() > frame["nodes"].as_u64());
    let uid = structure["uid"].to_string();
    let index = frame["index"].to_string();
    let one = json(&["--structure", &uid, "--frame", &index]);
    assert_eq!(
        one["nodes"].as_array().unwrap().len() as u64,
        frame["nodes"]
    );
    assert_eq!(
        one["wires"].as_array().unwrap().len() as u64,
        frame["wires"]
    );
    // The outline of the frame lists the structures inside it, and its
    // count of nodes is the one the outline of the diagram promised.
    let inside = json(&[
        "--structure",
        &uid,
        "--frame",
        &index,
        "--detail",
        "summary",
    ]);
    assert_eq!(inside["nodes"], frame["all_nodes"]);
    assert_ne!(inside["structures"], serde_json::json!([]));
    assert!(inside.get("root").is_none());

    // A range holds each node once, so no range is larger than the
    // diagram.
    let ids = &summary["diagram"]["node_ids"];
    let (first, last) = (ids[0].to_string(), ids[1].to_string());
    let args = [
        "inspect",
        "process.vi",
        "--format",
        "json",
        "--first-node",
        &first,
        "--last-node",
        &last,
    ];
    let (code, all, _) = run(&args, &dir);
    assert_eq!(code, 0);
    assert!(
        all.len()
            < serde_json::to_string_pretty(&full["diagram"])
                .unwrap()
                .len()
                + 4096
    );
    let all: Value = serde_json::from_str(&all).unwrap();
    assert_eq!(
        all.as_array().unwrap(),
        full["diagram"]["root"]["nodes"].as_array().unwrap()
    );
    let none = json(&["--first-node", "-5", "--last-node", "-1"]);
    assert_eq!(none, serde_json::json!([]));

    // What cannot be asked for.
    for (more, kind, words) in [
        (
            &["--structure", "-7", "--frame", "0"][..],
            "bad_argument",
            "no structure with the id -7",
        ),
        (
            &["--structure", &uid, "--frame", "99999"],
            "bad_argument",
            "has a frame 99999",
        ),
        (
            &[
                "--first-node",
                "1",
                "--last-node",
                "2",
                "--detail",
                "summary",
            ],
            "bad_argument",
            "not to a range",
        ),
        (&["--structure", "1"], "usage", "--frame"),
        (&["--first-node", "1"], "usage", "--last-node"),
        (
            &[
                "--structure",
                "1",
                "--frame",
                "0",
                "--first-node",
                "1",
                "--last-node",
                "2",
            ],
            "usage",
            "cannot be used with",
        ),
    ] {
        let mut args = vec!["inspect", "process.vi", "--format", "json"];
        args.extend(more);
        let (code, stdout, stderr) = run(&args, &dir);
        assert_eq!(code, 2, "{args:?}");
        let error: Value = serde_json::from_str(&stdout).expect("stdout is JSON");
        assert_eq!(error["error"]["kind"], kind, "{args:?}: {stdout}");
        assert!(stderr.contains(words), "{args:?}: {stderr}");
    }
    let (code, stdout, stderr) = run(
        &["inspect", "process.vi", "--structure", &uid, "--frame", "0"],
        &dir,
    );
    assert_eq!((code, stdout.as_str()), (2, ""));
    assert!(stderr.contains("pass --format json"), "{stderr}");
    // A project file has no frames.
    let dir = workspace("agent-frame");
    fs::write(dir.0.join("Kit.lvlib"), "<Library LVVersion=\"23008000\"/>").unwrap();
    let (code, stdout, _) = run(
        &[
            "inspect",
            "Kit.lvlib",
            "--format",
            "json",
            "--structure",
            "1",
            "--frame",
            "0",
        ],
        &dir.0,
    );
    assert_eq!(code, 2);
    let error: Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    assert_eq!(error["error"]["kind"], "unsupported");
}
