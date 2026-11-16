//! Black-box tests of `buzzard diff`, including a real `git diff` that
//! runs it as an external diff driver.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::*;

fn run(args: &[&str]) -> (i32, String, String) {
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(BIN).args(args).output().unwrap();
    (
        status.code().unwrap(),
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

fn pylavi(name: &str) -> String {
    fixtures().join(name).to_string_lossy().into_owned()
}

/// A pair of consecutive revisions of a real file.
fn revision(side: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../buzzard-lint/tests/fixtures/diff/jki-state-machine")
        .join(format!("vi-tree.{side}.vi"))
        .to_string_lossy()
        .into_owned()
}

const ADD_TO_EMPTY: &str = "1 node removed, 1 changed; 3 wires removed; 1 setting changed\n\
     changed: saved name `add.vi` -> `empty.vi`\n\
     removed: built-in function 1050 #43\n\
     changed: diagram boundary #7: terminals 3 -> 0 (-3 numeric)\n\
     removed: numeric wire from built-in function 1050 #43 to diagram boundary #7\n\
     removed: numeric wire from diagram boundary #7 to built-in function 1050 #43\n\
     removed: numeric wire from diagram boundary #7 to built-in function 1050 #43\n";

#[test]
fn exit_status_follows_diff() {
    let (code, stdout, stderr) = run(&["diff", &pylavi("add.vi"), &pylavi("add.vi")]);
    assert_eq!(
        (code, stdout.as_str(), stderr.as_str()),
        (0, "no differences\n", "")
    );

    let (code, stdout, stderr) = run(&["diff", &pylavi("add.vi"), &pylavi("empty.vi")]);
    assert_eq!(
        (code, stdout.as_str(), stderr.as_str()),
        (1, ADD_TO_EMPTY, "")
    );

    // Not an RSRC file, on either side.
    let readme = fixtures().join("../README.md");
    let readme = readme.to_string_lossy();
    for args in [
        ["diff", &readme, &pylavi("add.vi")],
        ["diff", &pylavi("add.vi"), &readme],
    ] {
        let (code, stdout, stderr) = run(&args);
        assert_eq!((code, stdout.as_str()), (2, ""));
        assert!(stderr.starts_with("error: "), "{stderr}");
        assert!(
            stderr.ends_with("README.md: missing RSRC signature\n"),
            "{stderr}"
        );
    }

    // A truncated file is an error too, not an empty side.
    let dir = TempDir::new("diff-truncated");
    let bytes = fs::read(pylavi("add.vi")).unwrap();
    let cut = dir.0.join("cut.vi");
    fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
    let (code, stdout, stderr) = run(&["diff", &pylavi("add.vi"), &cut.to_string_lossy()]);
    assert_eq!((code, stdout.as_str()), (2, ""));
    assert!(stderr.starts_with("error: "), "{stderr}");
}

#[test]
fn a_missing_side_is_all_added_or_all_removed() {
    let dir = TempDir::new("diff-missing");
    let absent = dir.0.join("absent.vi");
    let absent = absent.to_string_lossy();

    let (code, stdout, _) = run(&["diff", &absent, &pylavi("add.vi")]);
    assert_eq!(code, 1);
    assert_eq!(
        stdout,
        "2 nodes added; 3 wires added\n\
         note: old file is missing: everything is reported as added\n\
         added: built-in function 1050 #43\n\
         added: diagram boundary #7\n\
         added: numeric wire from built-in function 1050 #43 to diagram boundary #7\n\
         added: numeric wire from diagram boundary #7 to built-in function 1050 #43\n\
         added: numeric wire from diagram boundary #7 to built-in function 1050 #43\n"
    );

    let (code, stdout, _) = run(&["diff", &pylavi("empty.vi"), "/dev/null"]);
    assert_eq!(code, 1);
    assert_eq!(
        stdout,
        "1 node removed\n\
         note: new file is missing: everything is reported as removed\n\
         removed: diagram boundary #7\n"
    );

    let (code, stdout, stderr) = run(&["diff", &absent, "/dev/null"]);
    assert_eq!((code, stdout.as_str()), (2, ""));
    assert!(stderr.contains("neither"), "{stderr}");
}

#[test]
fn wrong_number_of_files_is_an_error() {
    let (code, _, stderr) = run(&["diff", &pylavi("add.vi")]);
    assert_eq!(code, 2);
    assert!(stderr.contains("expected two files"), "{stderr}");
    let (code, _, _) = run(&["diff", &pylavi("add.vi"), &pylavi("add.vi"), "extra"]);
    assert_eq!(code, 2);
    let (code, _, _) = run(&["diff"]);
    assert_eq!(code, 2);
}

#[test]
fn moves_can_be_ignored() {
    let (code, stdout, _) = run(&["diff", &revision("old"), &revision("new")]);
    assert_eq!(code, 1);
    assert!(
        stdout
            .starts_with("1 node added, 1 removed, 2 moved; 2 wires added, 2 removed, 2 changed\n"),
        "{stdout}"
    );
    assert_eq!(stdout.matches("\nmoved: ").count(), 2);

    let (code, ignored, _) = run(&["diff", "--ignore-moves", &revision("old"), &revision("new")]);
    assert_eq!(code, 1);
    assert!(
        ignored.starts_with(
            "1 node added, 1 removed; 2 wires added, 2 removed, 2 changed (2 moves ignored)\n"
        ),
        "{ignored}"
    );
    assert!(!ignored.lines().any(|l| l.starts_with("moved: ")));
    // Apart from the moves the two reports are the same.
    let rest = |text: &str| -> Vec<String> {
        let lines = text.lines().skip(1).filter(|l| !l.starts_with("moved: "));
        lines.map(str::to_owned).collect()
    };
    assert_eq!(rest(&stdout), rest(&ignored));
}

#[test]
fn formats_carry_the_same_report() {
    let (old, new) = (pylavi("add.vi"), pylavi("empty.vi"));
    let (code, markdown, _) = run(&["diff", &old, &new, "--format", "markdown"]);
    assert_eq!(code, 1);
    assert_eq!(
        markdown,
        "**1 node removed, 1 changed; 3 wires removed; 1 setting changed**\n\
         \n\
         #### Settings\n\
         \n\
         - changed: saved name `add.vi` -> `empty.vi`\n\
         \n\
         #### Block diagram\n\
         \n\
         - removed: built-in function 1050 #43\n\
         - changed: diagram boundary #7: terminals 3 -> 0 (-3 numeric)\n\
         - removed: numeric wire from built-in function 1050 #43 to diagram boundary #7\n\
         - removed: numeric wire from diagram boundary #7 to built-in function 1050 #43\n\
         - removed: numeric wire from diagram boundary #7 to built-in function 1050 #43\n"
    );

    let (code, json, _) = run(&["diff", &old, &new, "--format", "json"]);
    assert_eq!(code, 1);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["identical"], false);
    assert_eq!(value["summary"]["nodes_removed"], 1);
    assert_eq!(value["summary"]["wires_removed"], 3);
    assert_eq!(value["settings"][0]["setting"], "saved name");
    assert_eq!(value["nodes"]["removed"][0]["uid"], 43);
    assert_eq!(
        value["nodes"]["removed"][0]["description"],
        "built-in function 1050"
    );
    assert_eq!(
        value["nodes"]["changed"][0]["details"][0],
        "terminals 3 -> 0 (-3 numeric)"
    );
    assert_eq!(value["wires"]["removed"].as_array().unwrap().len(), 3);

    let (code, json, _) = run(&["diff", &old, &old, "--format", "json"]);
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["identical"], true);

    // Byte-identical on every run.
    for format in ["text", "markdown", "json"] {
        let args = [
            "diff",
            &revision("old"),
            &revision("new"),
            "--format",
            format,
        ];
        assert_eq!(run(&args), run(&args));
    }
}

#[test]
fn git_calling_convention_always_exits_zero() {
    let (old, new) = (pylavi("add.vi"), pylavi("empty.vi"));
    let hex = "0123456789abcdef0123456789abcdef01234567";
    let (code, stdout, stderr) = run(&[
        "diff",
        "--git",
        "src/Main.vi",
        &old,
        hex,
        "100644",
        &new,
        hex,
        "100644",
    ]);
    assert_eq!((code, stderr.as_str()), (0, ""));
    assert_eq!(
        stdout,
        format!("buzzard diff a/src/Main.vi b/src/Main.vi\n{ADD_TO_EMPTY}")
    );

    // A rename adds the new path and a similarity line.
    let (code, stdout, _) = run(&[
        "diff",
        "--git",
        "Old.vi",
        &old,
        hex,
        "100644",
        &new,
        hex,
        "100644",
        "New.vi",
        "similarity index 90%\n",
    ]);
    assert_eq!(code, 0);
    assert!(
        stdout.starts_with("buzzard diff a/Old.vi b/New.vi\n"),
        "{stdout}"
    );

    // An added file: git passes /dev/null and a mode of ".".
    let (code, stdout, _) = run(&[
        "diff",
        "--git",
        "Main.vi",
        "/dev/null",
        ".",
        ".",
        &new,
        hex,
        "100644",
    ]);
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "buzzard diff a/Main.vi b/Main.vi\n\
         1 node added\n\
         note: old file is missing: everything is reported as added\n\
         added: diagram boundary #7\n"
    );

    // A file that cannot be parsed is reported, not fatal.
    let readme = fixtures().join("../README.md");
    let (code, stdout, _) = run(&[
        "diff",
        "--git",
        "Main.vi",
        &old,
        hex,
        "100644",
        &readme.to_string_lossy(),
        hex,
        "100644",
    ]);
    assert_eq!(code, 0);
    assert!(
        stdout.starts_with("buzzard diff a/Main.vi b/Main.vi\ncannot compare: "),
        "{stdout}"
    );
    assert!(stdout.ends_with("missing RSRC signature\n"), "{stdout}");

    // An unmerged path comes alone.
    let (code, stdout, _) = run(&["diff", "--git", "Main.vi"]);
    assert_eq!(
        (code, stdout.as_str()),
        (0, "buzzard diff Main.vi\nunmerged\n")
    );

    // Anything else is a usage error.
    let (code, _, stderr) = run(&["diff", "--git", &old, &new]);
    assert_eq!(code, 2);
    assert!(stderr.contains("--git expects"), "{stderr}");
}

/// Run git in `dir`, isolated from the user's and the system's settings.
fn git(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let output = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=buzzard",
            "-c",
            "user.email=buzzard@example.invalid",
        ])
        .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_PAGER", "cat")
        .env_remove("GIT_EXTERNAL_DIFF")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("git must be installed to run this test");
    (
        output.status.code().unwrap(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let (code, stdout, stderr) = git(dir, args);
    assert_eq!(code, 0, "git {args:?}: {stderr}");
    stdout
}

/// A repository with `Main.vi` (the `add.vi` fixture) committed and the
/// `.gitattributes` line from the README.
fn repository(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    git_ok(&dir.0, &["init", "-q"]);
    fs::write(dir.0.join(".gitattributes"), "*.vi diff=buzzard\n").unwrap();
    fs::copy(pylavi("add.vi"), dir.0.join("Main.vi")).unwrap();
    git_ok(&dir.0, &["add", "."]);
    git_ok(&dir.0, &["commit", "-q", "-m", "first"]);
    dir
}

/// The binary as git's shell will see it.
fn quoted_bin() -> String {
    format!("'{}'", BIN.replace('\\', "/"))
}

#[test]
fn git_diff_runs_the_external_driver() {
    let dir = repository("diff-git-driver");
    let driver = format!("diff.buzzard.command={} diff --git", quoted_bin());

    // Unchanged: git does not call the driver at all.
    assert_eq!(git_ok(&dir.0, &["-c", &driver, "diff"]), "");

    // Modified in the working tree.
    fs::copy(pylavi("empty.vi"), dir.0.join("Main.vi")).unwrap();
    assert_eq!(
        git_ok(&dir.0, &["-c", &driver, "diff"]),
        format!("buzzard diff a/Main.vi b/Main.vi\n{ADD_TO_EMPTY}")
    );
    // Options of the driver go into its command line.
    let markdown = format!("{driver} --format markdown --ignore-moves");
    let out = git_ok(&dir.0, &["-c", &markdown, "diff"]);
    assert!(
        out.starts_with(
            "buzzard diff a/Main.vi b/Main.vi\n\
             **1 node removed, 1 changed; 3 wires removed; 1 setting changed**\n"
        ),
        "{out}"
    );

    // A new file and a deleted file, staged.
    fs::copy(pylavi("empty.vi"), dir.0.join("New.vi")).unwrap();
    fs::remove_file(dir.0.join("Main.vi")).unwrap();
    git_ok(&dir.0, &["add", "-A"]);
    assert_eq!(
        git_ok(&dir.0, &["-c", &driver, "diff", "--cached", "--no-renames"]),
        "buzzard diff a/Main.vi b/Main.vi\n\
         2 nodes removed; 3 wires removed\n\
         note: new file is missing: everything is reported as removed\n\
         removed: built-in function 1050 #43\n\
         removed: diagram boundary #7\n\
         removed: numeric wire from built-in function 1050 #43 to diagram boundary #7\n\
         removed: numeric wire from diagram boundary #7 to built-in function 1050 #43\n\
         removed: numeric wire from diagram boundary #7 to built-in function 1050 #43\n\
         buzzard diff a/New.vi b/New.vi\n\
         1 node added\n\
         note: old file is missing: everything is reported as added\n\
         added: diagram boundary #7\n"
    );

    // History: `git log -p` and `git show` need --ext-diff.
    git_ok(&dir.0, &["commit", "-q", "-m", "second"]);
    let shown = git_ok(
        &dir.0,
        &[
            "-c",
            &driver,
            "show",
            "--ext-diff",
            "--no-renames",
            "--format=",
            "HEAD",
        ],
    );
    assert!(
        shown.contains("buzzard diff a/New.vi b/New.vi\n1 node added\n"),
        "{shown}"
    );

    // A file git tracks as a VI but that is not one does not break git.
    fs::write(dir.0.join("New.vi"), "not a resource file\n").unwrap();
    let (code, out, stderr) = git(&dir.0, &["-c", &driver, "diff"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        out.starts_with("buzzard diff a/New.vi b/New.vi\ncannot compare: "),
        "{out}"
    );
}

#[test]
fn git_diff_with_textconv_shows_a_line_diff_of_the_summary() {
    let dir = repository("diff-git-textconv");
    let textconv = format!(
        "diff.buzzard.textconv={} inspect --format markdown",
        quoted_bin()
    );
    fs::copy(pylavi("add_breakpoints.vi"), dir.0.join("Main.vi")).unwrap();
    let out = git_ok(&dir.0, &["-c", &textconv, "diff"]);
    assert!(
        out.contains("\n-| Breakpoints | no |\n+| Breakpoints | yes |\n"),
        "{out}"
    );
    // Without the setting git only knows that a binary file changed.
    let plain = git_ok(&dir.0, &["diff"]);
    assert!(plain.contains("Binary files"), "{plain}");
}
