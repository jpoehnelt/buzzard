//! Black-box tests of the `buzzard` binary on `.llb` archives and on the
//! paths of their members.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use buzzard_rsrc::builder::{LlbBuilder, RsrcBuilder};
use buzzard_rsrc::{LvPath, PathKind};
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

/// Output with the path separator of this system written as `/`.
fn slashes(text: &str) -> String {
    text.replace("\\\\", "/").replace('\\', "/")
}

fn write(dir: &TempDir, rel: &str, bytes: &[u8]) {
    let file = dir.0.join(rel);
    fs::create_dir_all(file.parent().expect("a file has a parent")).expect("create parents");
    fs::write(file, bytes).expect("write the file");
}

/// A VI with links written like paths from the file itself: `/a.vi` is a
/// sibling.
fn vi(words: &[u32], links: &[&str]) -> Vec<u8> {
    let links: Vec<(&str, LvPath)> = links
        .iter()
        .map(|link| {
            let path = LvPath {
                kind: PathKind::Relative,
                components: link.split('/').map(str::to_owned).collect(),
            };
            (link.rsplit('/').next().unwrap_or(link), path)
        })
        .collect();
    RsrcBuilder::default()
        .save_record(V23, words)
        .links(&links)
        .build()
}

/// A real VI with a block diagram.
fn add_vi() -> Vec<u8> {
    fs::read(fixtures().join("add.vi")).expect("read the fixture")
}

/// A real VI with a state machine that has findings.
fn machine_vi() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "../buzzard-lint/tests/fixtures/verify/jki-state-machine/new-support-vis-examples.vi",
    );
    fs::read(path).expect("read the fixture")
}

/// A workspace with one archive and one file that calls into it.
fn workspace(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    write(&dir, "Main.vi", &vi(&CLEAN, &["/lib.llb/Entry.vi"]));
    write(
        &dir,
        "lib.llb",
        &LlbBuilder::default()
            .version(V23)
            .member("Entry.vi", &vi(&INLINE_CODE, &["/add.vi"]))
            .compressed("add.vi", &add_vi())
            .member("Type.ctl", &vi(&CLEAN, &[]))
            .member("notes.txt", b"hello")
            .build(),
    );
    dir
}

#[test]
fn inspect_lists_the_members_of_an_archive() {
    let dir = workspace("archive-inspect");
    let (code, stdout, stderr) = run(&["inspect", "lib.llb"], &dir.0);
    assert_eq!((code, stderr.as_str()), (0, ""));
    let add = add_vi().len();
    let entry = vi(&INLINE_CODE, &["/add.vi"]).len();
    let ctl = vi(&CLEAN, &[]).len();
    let rows: Vec<&str> = stdout.lines().filter(|l| l.contains("23.0.0")).collect();
    assert_eq!(rows.len(), 4, "{stdout}");
    assert!(stdout.starts_with(
        "### lib.llb\n\n| | |\n| --- | --- |\n| Kind | LLB archive (`LVAR`) |\n| Members | 4 |\n"
    ));
    // Directory order, not name order.
    assert_eq!(
        rows[0],
        format!("| Entry.vi | VI (`LVIN`) | 23.0.0 | stored | {entry} | {entry} |")
    );
    assert!(
        rows[1].starts_with("| add.vi | VI (`LVIN`) | 23.0.0 | zlib | ")
            && rows[1].ends_with(&format!(" | {add} |")),
        "{}",
        rows[1]
    );
    assert_eq!(
        rows[2],
        format!("| Type.ctl | VI (`LVIN`) | 23.0.0 | stored | {ctl} | {ctl} |")
    );
    assert!(rows[3].starts_with("| notes.txt | Other ("), "{}", rows[3]);

    let (code, stdout, _) = run(&["inspect", "lib.llb", "--format", "json"], &dir.0);
    assert_eq!(code, 0);
    let json: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["kind"], "library");
    assert_eq!(json["file_type"], "LVAR");
    assert_eq!(json["unlisted"], 0);
    assert!(json.get("problems").is_none());
    let members = json["members"].as_array().unwrap();
    assert_eq!(members.len(), 4);
    assert_eq!(
        members[1],
        serde_json::json!({
            "name": "add.vi",
            "id": 2,
            "file_type": "LVIN",
            "kind": "vi",
            "version": { "major": 23, "minor": 0, "patch": 0, "stage": "release", "build": 0 },
            "storage": "zlib",
            "stored_size": members[1]["stored_size"],
            "size": add,
        })
    );
    assert!(members[1]["stored_size"].as_u64().unwrap() < add as u64);
    assert_eq!(members[3]["kind"], "other");
    assert_eq!(members[3]["storage"], "stored");

    // An archive has no diagram: the error names a member to ask for.
    for format in ["svg", "dot", "mermaid"] {
        let (code, stdout, stderr) = run(&["inspect", "lib.llb", "--format", format], &dir.0);
        assert_eq!((code, stdout.as_str()), (2, ""), "{format}");
        assert!(
            slashes(&stderr).contains(
                "lib.llb: an archive has no block diagram; name one of its members, as in \
                 `buzzard inspect \"lib.llb/Entry.vi\"`"
            ),
            "{stderr}"
        );
    }
}

#[test]
fn inspect_reads_a_member_like_the_file_it_holds() {
    let dir = workspace("archive-member");
    write(&dir, "loose/add.vi", &add_vi());
    for format in ["markdown", "json", "svg", "dot", "mermaid"] {
        let member = run(&["inspect", "lib.llb/add.vi", "--format", format], &dir.0);
        let loose = run(&["inspect", "loose/add.vi", "--format", format], &dir.0);
        assert_eq!(member.0, 0, "{format}: {}", member.2);
        assert_eq!(member, loose, "{format}");
        assert!(member.1.len() > 50, "{format}");
    }
    // The name is matched as the editor matches it: without letter case.
    let (code, stdout, _) = run(&["inspect", "lib.llb/ADD.VI"], &dir.0);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("### ADD.VI\n"));
}

#[test]
fn what_cannot_be_opened_exits_two_with_the_reason() {
    let dir = workspace("archive-errors");
    write(&dir, "broken.llb", b"not an archive at all");
    let (code, stdout, stderr) = run(&["inspect", "lib.llb/Absent.vi"], &dir.0);
    assert_eq!((code, stdout.as_str()), (2, ""));
    assert!(
        stderr.contains("the archive has no member `Absent.vi`"),
        "{stderr}"
    );
    let (code, _, stderr) = run(&["inspect", "broken.llb"], &dir.0);
    assert_eq!(code, 2);
    assert!(stderr.contains("RSRC"), "{stderr}");
    let (code, _, stderr) = run(&["inspect", "broken.llb/Any.vi"], &dir.0);
    assert_eq!(code, 2);
    assert!(stderr.contains("RSRC"), "{stderr}");
    // A member of a member, and a name that is a path, are not members.
    for path in ["lib.llb/add.vi/deeper.vi", "lib.llb/../lib.llb/nothing.vi"] {
        let (code, _, _) = run(&["inspect", path], &dir.0);
        assert_eq!(code, 2, "{path}");
    }
    // A path that does not exist is refused by every command that scans.
    for command in ["check", "verify", "deps"] {
        for path in ["lib.llb/Absent.vi", "absent.llb/Entry.vi"] {
            let (code, stdout, stderr) = run(&[command, path], &dir.0);
            assert_eq!((code, stdout.as_str()), (2, ""), "{command} {path}");
            assert!(stderr.contains("path does not exist"), "{stderr}");
        }
    }
}

#[test]
fn the_real_archives_are_listed_or_refused() {
    let fixtures = fixtures().join("../pylabview");
    let (code, stdout, _) = run(&["inspect", "empty_libfile.llb"], &fixtures);
    assert_eq!(code, 0);
    assert!(stdout.contains("| Members | 0 |"), "{stdout}");
    let (code, stdout, _) = run(&["check", "empty_libfile.llb"], &fixtures);
    assert_eq!(
        (code, stdout.as_str()),
        (0, "No problems found in 1 file.\n")
    );
    // An archive taken out of a built application has no directory.
    let built = "blank_project1_extr_from_exe_lv14f1.llb";
    let (code, _, stderr) = run(&["inspect", built], &fixtures);
    assert_eq!(code, 2);
    assert!(stderr.contains("zipped program tree"), "{stderr}");
    let (code, stdout, _) = run(&["check", built], &fixtures);
    assert_eq!(code, 1);
    assert!(
        stdout.contains("error[BZ000]: cannot analyze file: "),
        "{stdout}"
    );
    assert!(stdout.contains("Found 1 problem (1 error, 0 warnings) in 1 file."));
}

const BZ001: &str = "warning[BZ001]: compiled code is not separated from the source file; \
                     enable \"Separate compiled code from source file\"";

#[test]
fn check_lints_the_members_of_an_archive() {
    let dir = workspace("archive-check");
    let (code, stdout, stderr) = run(&["check", "."], &dir.0);
    assert_eq!((code, stderr.as_str()), (1, ""));
    let lines: Vec<String> = slashes(&stdout).lines().map(str::to_owned).collect();
    assert_eq!(lines[0], format!("lib.llb/Entry.vi: {BZ001}"));
    assert!(lines[1..lines.len() - 1]
        .iter()
        .all(|line| line.starts_with("lib.llb/add.vi: ")));
    // The archive, three members that are VIs or controls, and Main.vi.
    assert!(lines.last().unwrap().ends_with(" in 5 files."), "{stdout}");

    // A member path is checked like a file path.
    let (code, stdout, _) = run(&["check", "lib.llb/Entry.vi"], &dir.0);
    assert_eq!(code, 1);
    assert_eq!(
        slashes(&stdout),
        format!("lib.llb/Entry.vi: {BZ001}\nFound 1 problem (0 errors, 1 warning) in 1 file.\n")
    );
    let (code, stdout, _) = run(&["check", "lib.llb/Type.ctl"], &dir.0);
    assert_eq!(
        (code, stdout.as_str()),
        (0, "No problems found in 1 file.\n")
    );
    // The archive alone: its members without the caller.
    let (code, stdout, _) = run(&["check", "lib.llb", "--ignore", "BZ001"], &dir.0);
    assert!(stdout.ends_with(" in 4 files.\n"), "{stdout}");
    assert!(code == 0 || stdout.contains("lib.llb"));

    let (_, json, _) = run(&["check", ".", "--format", "json"], &dir.0);
    let json: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        slashes(json[0]["path"].as_str().unwrap()),
        "lib.llb/Entry.vi"
    );
    assert_eq!(json[0]["code"], "BZ001");
}

#[test]
fn check_sarif_is_valid_for_a_member_finding() {
    let dir = workspace("archive-sarif");
    let (code, stdout, _) = run(&["check", ".", "--format", "sarif"], &dir.0);
    assert_eq!(code, 1);
    let sarif: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(sarif["version"], "2.1.0");
    let rules = sarif["runs"][0]["tool"]["driver"]["rules"]
        .as_array()
        .unwrap();
    let results = sarif["runs"][0]["results"].as_array().unwrap();
    assert!(results.len() >= 2);
    for result in results {
        // What a consumer requires of every result: a rule that is
        // listed, a message, and an artifact that is a file.
        let index = usize::try_from(result["ruleIndex"].as_u64().unwrap()).unwrap();
        assert_eq!(rules[index]["id"], result["ruleId"]);
        assert_ne!(result["message"]["text"].as_str().unwrap().len(), 0);
        let locations = result["locations"].as_array().unwrap();
        assert_eq!(locations.len(), 1);
        let uri = locations[0]["physicalLocation"]["artifactLocation"]["uri"]
            .as_str()
            .unwrap();
        assert_eq!(uri, "lib.llb");
        assert!(dir.0.join(uri).is_file());
        let logical = locations[0]["logicalLocations"].as_array().unwrap();
        assert_eq!(logical.len(), 1);
        let name = logical[0]["name"].as_str().unwrap();
        assert_eq!(
            logical[0]["fullyQualifiedName"].as_str().unwrap(),
            format!("lib.llb/{name}")
        );
        assert!(result["message"]["text"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{name}: ")));
    }
    assert_eq!(results[0]["ruleId"], "BZ001");
    assert_eq!(
        results[0]["locations"][0]["logicalLocations"][0]["name"],
        "Entry.vi"
    );
}

#[test]
fn verify_checks_members_and_takes_member_paths() {
    let dir = TempDir::new("archive-verify");
    write(
        &dir,
        "packed.llb",
        &LlbBuilder::default()
            .compressed("Machine.vi", &machine_vi())
            .member("Plain.vi", &vi(&CLEAN, &[]))
            .build(),
    );
    write(&dir, "loose/Machine.vi", &machine_vi());
    let (code, member, stderr) = run(&["verify", "packed.llb/Machine.vi"], &dir.0);
    assert_eq!((code, stderr.as_str()), (1, ""));
    let (_, loose, _) = run(&["verify", "loose/Machine.vi"], &dir.0);
    assert!(
        slashes(&member).starts_with("packed.llb/Machine.vi\n"),
        "{member}"
    );
    assert_eq!(
        slashes(&member).replace("packed.llb/Machine.vi", "loose/Machine.vi"),
        slashes(&loose)
    );
    assert!(member.contains("HEURISTIC [BZ203]"));

    let (code, all, _) = run(&["verify", ".", "--format", "json"], &dir.0);
    assert_eq!(code, 1);
    let json: Value = serde_json::from_str(&all).unwrap();
    let paths: Vec<String> = json["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| slashes(file["path"].as_str().unwrap()))
        .collect();
    assert_eq!(paths, ["loose/Machine.vi", "packed.llb/Machine.vi"]);
    assert_eq!(json["files"][0]["findings"], json["files"][1]["findings"]);

    let (code, stdout, _) = run(&["verify", "packed.llb/Plain.vi"], &dir.0);
    assert_eq!(code, 0);
    assert!(stdout.contains("No findings."), "{stdout}");
    let (code, dot, _) = run(&["verify", "packed.llb", "--format", "dot"], &dir.0);
    assert_eq!(code, 1);
    assert!(dot.starts_with("digraph"), "{dot}");
}

#[test]
fn diff_takes_a_member_on_either_side() {
    let dir = workspace("archive-diff");
    write(&dir, "loose/add.vi", &add_vi());
    write(&dir, "loose/Entry.vi", &vi(&CLEAN, &["/add.vi"]));
    write(
        &dir,
        "other.llb",
        &LlbBuilder::default()
            .member("Entry.vi", &vi(&CLEAN, &["/add.vi", "/more.vi"]))
            .build(),
    );
    // The same file on both sides, whichever is the member.
    for (old, new) in [
        ("lib.llb/add.vi", "loose/add.vi"),
        ("loose/add.vi", "lib.llb/add.vi"),
        ("lib.llb/add.vi", "lib.llb/add.vi"),
    ] {
        let (code, stdout, stderr) = run(&["diff", old, new], &dir.0);
        assert_eq!((code, stderr.as_str()), (0, ""), "{old} {new}: {stdout}");
    }
    // Two members of two archives, and a member against a file.
    let (code, between, _) = run(&["diff", "lib.llb/Entry.vi", "other.llb/Entry.vi"], &dir.0);
    assert_eq!(code, 1);
    assert!(between.contains("more.vi"), "{between}");
    let (code, against, _) = run(&["diff", "loose/Entry.vi", "lib.llb/Entry.vi"], &dir.0);
    assert_eq!(code, 1);
    assert!(
        against.to_lowercase().contains("compiled code"),
        "{against}"
    );
    let (code, json, _) = run(
        &[
            "diff",
            "lib.llb/Entry.vi",
            "other.llb/Entry.vi",
            "--format",
            "json",
        ],
        &dir.0,
    );
    assert_eq!(code, 1);
    assert!(serde_json::from_str::<Value>(&json).is_ok());

    // A member that is not in its archive is a file that is not there:
    // the other side was added or deleted.
    let (code, added, _) = run(&["diff", "lib.llb/Absent.vi", "lib.llb/add.vi"], &dir.0);
    let (_, from_nothing, _) = run(&["diff", "loose/absent.vi", "lib.llb/add.vi"], &dir.0);
    assert_eq!(code, 1);
    assert_eq!(added, from_nothing);
    // An archive that cannot be read is an error.
    write(&dir, "broken.llb", b"garbage");
    let (code, _, stderr) = run(&["diff", "broken.llb/a.vi", "lib.llb/add.vi"], &dir.0);
    assert_eq!(code, 2);
    assert!(stderr.contains("RSRC"), "{stderr}");
}

#[test]
fn deps_and_check_agree_about_members_that_are_not_there() {
    let dir = workspace("archive-deps");
    write(
        &dir,
        "Stale.vi",
        &vi(&CLEAN, &["/lib.llb/Removed.vi", "/lib.llb/Entry.vi"]),
    );
    write(
        &dir,
        "App.lvproj",
        b"<Project Type=\"Project\" LVVersion=\"23008000\">\n\
          <Item Name=\"Entry.vi\" Type=\"VI\" URL=\"../lib.llb/Entry.vi\"/>\n\
          <Item Name=\"Removed.vi\" Type=\"VI\" URL=\"../lib.llb/Removed.vi\"/>\n\
          <Item Name=\"Lost.vi\" Type=\"VI\" URL=\"../lib.llb/Lost.vi\"/>\n\
          </Project>\n",
    );
    let (code, text, stderr) = run(&["deps", "."], &dir.0);
    assert_eq!((code, stderr.as_str()), (0, ""));
    let text = slashes(&text);
    assert!(
        text.contains(
            "Missing relative links (2)\n  lib.llb/Lost.vi\n    used by App.lvproj\n  \
             lib.llb/Removed.vi\n    used by App.lvproj\n    used by Stale.vi\n"
        ),
        "{text}"
    );
    // The archive, its three members, Main.vi, Stale.vi and the project.
    assert!(text.contains("Scanned 7 files: "), "{text}");
    assert!(
        text.contains("2 missing relative links, 0 unparsed files."),
        "{text}"
    );

    let (code, check, _) = run(&["check", ".", "--format", "json"], &dir.0);
    assert_eq!(code, 1);
    let check: Value = serde_json::from_str(&check).unwrap();
    let flagged: Vec<&str> = check
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "BZ006")
        .map(|d| d["message"].as_str().unwrap())
        .collect();
    assert_eq!(
        flagged,
        [
            "member `Lost.vi` does not exist on disk (`../lib.llb/Lost.vi`)",
            "member `Removed.vi` does not exist on disk (`../lib.llb/Removed.vi`)",
        ]
    );

    // A member path as the only root: the links of that one member.
    let (code, json, _) = run(&["deps", "lib.llb/Entry.vi", "--format", "json"], &dir.0);
    assert_eq!(code, 0);
    let json: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(json["files"], 1);
    assert_eq!(json["missing"], serde_json::json!([]));
}

#[test]
fn deps_looks_for_an_installed_member_inside_its_archive() {
    let dir = TempDir::new("archive-root");
    let link = |path: &str| LvPath {
        kind: PathKind::Absolute,
        components: path.split('/').map(str::to_owned).collect(),
    };
    let links = [
        "<vilib>/Utility/error.llb/Clear Errors.vi",
        "<vilib>/Utility/error.llb/Not There.vi",
        "<vilib>/Utility/broken.llb/Any.vi",
    ];
    let named: Vec<(&str, LvPath)> = links.iter().map(|l| ("x.vi", link(l))).collect();
    write(
        &dir,
        "ws/Main.vi",
        &RsrcBuilder::default()
            .save_record(V23, &CLEAN)
            .links(&named)
            .build(),
    );
    write(
        &dir,
        "vi.lib/Utility/error.llb",
        &LlbBuilder::default()
            .compressed("Clear Errors.vi", &vi(&CLEAN, &[]))
            .build(),
    );
    write(&dir, "vi.lib/Utility/broken.llb", b"cannot be read");
    let (code, stdout, stderr) = run(
        &["deps", "ws", "--root", "vilib=vi.lib", "--format", "json"],
        &dir.0,
    );
    assert_eq!((code, stderr.as_str()), (0, ""));
    let json: Value = serde_json::from_str(&stdout).unwrap();
    let statuses: Vec<(&str, &str)> = json["roots"][0]["packages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|package| package["targets"].as_array().unwrap())
        .map(|t| (t["path"].as_str().unwrap(), t["status"].as_str().unwrap()))
        .collect();
    assert_eq!(
        statuses,
        [
            ("<vilib>/Utility/broken.llb/Any.vi", "unchecked"),
            ("<vilib>/Utility/error.llb/Clear Errors.vi", "found"),
            ("<vilib>/Utility/error.llb/Not There.vi", "missing"),
        ]
    );
}

#[test]
fn output_about_archives_is_byte_identical_across_runs() {
    let dir = workspace("archive-determinism");
    write(&dir, "machine.llb", &{
        let mut builder = LlbBuilder::default();
        for i in (0..12).rev() {
            builder = builder.compressed(&format!("M{i:02}.vi"), &machine_vi());
        }
        builder.build()
    });
    let commands: [&[&str]; 8] = [
        &["check", "."],
        &["check", ".", "--format", "json"],
        &["check", ".", "--format", "sarif"],
        &["verify", "."],
        &["verify", ".", "--format", "json"],
        &["deps", ".", "--format", "json"],
        &["inspect", "machine.llb", "--format", "json"],
        &["inspect", "lib.llb/add.vi", "--format", "svg"],
    ];
    for command in commands {
        let first = run(command, &dir.0);
        assert!(!first.1.is_empty(), "{command:?}");
        for _ in 0..4 {
            assert_eq!(run(command, &dir.0), first, "{command:?}");
        }
    }
    // Members are listed in the order of the directory, and reported in
    // path order.
    let (_, listing, _) = run(&["inspect", "machine.llb"], &dir.0);
    let first = listing.find("| M11.vi |").unwrap();
    assert!(first < listing.find("| M00.vi |").unwrap());
    let (_, report, _) = run(&["verify", "machine.llb"], &dir.0);
    let report = slashes(&report);
    let first = report.find("machine.llb/M00.vi").unwrap();
    assert!(first < report.find("machine.llb/M11.vi").unwrap());
}
