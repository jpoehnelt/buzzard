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
    // The rows are in directory order, which is not name order here.
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

    // An archive has no diagram, so the error names a member to ask for.
    for format in ["svg", "dot", "mermaid", "html"] {
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
    // The HTML report names the file as it was passed, and hashes the
    // member's own bytes.
    let member = run(&["inspect", "lib.llb/add.vi", "--format", "html"], &dir.0);
    let loose = run(&["inspect", "loose/add.vi", "--format", "html"], &dir.0);
    assert_eq!((member.0, member.2.as_str()), (0, ""));
    assert_eq!((loose.0, loose.2.as_str()), (0, ""));
    let hash = sha256(&add_vi());
    for (report, name) in [(&member.1, "lib.llb/add.vi"), (&loose.1, "loose/add.vi")] {
        assert!(report.contains(&hash), "{report}");
        assert!(
            slashes(report).contains(&format!("<h1 class=\"bz-title\">{name}</h1>")),
            "{report}"
        );
        assert_eq!(report.matches("<svg ").count(), 1);
    }
    // The page is the same but for the name of the file.
    assert_eq!(
        slashes(&member.1).replace("lib.llb/add.vi", "X"),
        slashes(&loose.1).replace("loose/add.vi", "X")
    );
    // The name is matched as the editor matches it, without letter
    // case, and shown as the archive has it.
    let (code, stdout, _) = run(&["inspect", "lib.llb/ADD.VI"], &dir.0);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("### add.vi\n"), "{stdout}");
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
    // Every command that scans refuses a path that does not exist.
    for command in ["check", "verify", "deps"] {
        for path in ["lib.llb/Absent.vi", "absent.llb/Entry.vi"] {
            let (code, stdout, stderr) = run(&[command, path], &dir.0);
            assert_eq!((code, stdout.as_str()), (2, ""), "{command} {path}");
            assert!(stderr.contains("path does not exist"), "{stderr}");
        }
    }
}

#[test]
fn the_real_archives_are_listed_or_shown_as_resource_files() {
    let fixtures = fixtures().join("../pylabview");
    let (code, stdout, _) = run(&["inspect", "empty_libfile.llb"], &fixtures);
    assert_eq!(code, 0);
    assert!(stdout.contains("| Members | 0 |"), "{stdout}");
    let (code, stdout, _) = run(&["check", "empty_libfile.llb"], &fixtures);
    assert_eq!(
        (code, stdout.as_str()),
        (0, "No problems found in 1 file.\n")
    );
    // The JSON of an archive has what the JSON of any resource file has,
    // and the directory.
    let (code, stdout, _) = run(
        &["inspect", "empty_libfile.llb", "--format", "json"],
        &fixtures,
    );
    assert_eq!(code, 0);
    let json: Value = serde_json::from_str(&stdout).unwrap();
    let keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "file_type",
            "flags",
            "kind",
            "libraries",
            "links",
            "members",
            "password_protected",
            "resource_types",
            "saved_name",
            "unlisted",
            "version"
        ]
    );
    assert_eq!(json["resource_types"][0], "ADir");

    // An archive taken out of a built application has no directory. It
    // is a resource file, and nothing about it is a problem.
    let built = "blank_project1_extr_from_exe_lv14f1.llb";
    let (code, stdout, stderr) = run(&["inspect", built], &fixtures);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("| Kind | LLB archive (`LVAR`) |"),
        "{stdout}"
    );
    assert!(!stdout.contains("| Members |"), "{stdout}");
    assert!(
        stderr.starts_with("note: ") && stderr.contains("has no directory of members"),
        "{stderr}"
    );
    let (code, stdout, _) = run(&["inspect", built, "--format", "json"], &fixtures);
    assert_eq!(code, 0);
    let json: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["kind"], "library");
    assert_eq!(json["resource_types"], serde_json::json!(["FLAG", "LVzp"]));
    assert!(json.get("members").is_none());
    let (code, stdout, _) = run(&["check", built], &fixtures);
    assert_eq!(
        (code, stdout.as_str()),
        (0, "No problems found in 1 file.\n")
    );
    let (code, stdout, _) = run(&["verify", built, "--format", "json"], &fixtures);
    assert_eq!(code, 0);
    let json: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["unreadable"], serde_json::json!([]));
    // It is compared like any resource file, too.
    let (code, stdout, _) = run(&["diff", built, built], &fixtures);
    assert_eq!((code, stdout.as_str()), (0, "no differences\n"));
}

/// Two archives are not compared member by member, so they are not said
/// to be the same unless their bytes are.
#[test]
fn diff_does_not_call_two_archives_the_same() {
    let dir = workspace("archive-diff-whole");
    write(
        &dir,
        "other.llb",
        &LlbBuilder::default()
            .member("Entry.vi", &vi(&CLEAN, &[]))
            .build(),
    );
    fs::copy(dir.0.join("lib.llb"), dir.0.join("copy.llb")).unwrap();
    for (old, new) in [
        ("lib.llb", "other.llb"),
        ("other.llb", "lib.llb"),
        ("lib.llb", "Main.vi"),
        ("lib.llb/Entry.vi", "other.llb"),
    ] {
        let (code, stdout, stderr) = run(&["diff", old, new], &dir.0);
        assert_eq!((code, stdout.as_str()), (2, ""), "{old} {new}");
        assert!(
            stderr.contains("an archive is not compared as a whole; name one of its members"),
            "{stderr}"
        );
    }
    for (old, new) in [("lib.llb", "lib.llb"), ("lib.llb", "copy.llb")] {
        let (code, stdout, _) = run(&["diff", old, new], &dir.0);
        assert_eq!(
            (code, stdout.as_str()),
            (0, "no differences\n"),
            "{old} {new}"
        );
    }
    // An archive whose directory cannot be read is an error, not "same".
    let mut cut = fs::read(dir.0.join("lib.llb")).unwrap();
    cut.truncate(cut.len() - 30);
    write(&dir, "cut.llb", &cut);
    let (code, _, _) = run(&["diff", "cut.llb", "other.llb"], &dir.0);
    assert_eq!(code, 2);
    // As a git diff driver the refusal is part of the output.
    let (code, stdout, _) = run(
        &[
            "diff",
            "--git",
            "lib.llb",
            "lib.llb",
            "0",
            "100644",
            "other.llb",
            "1",
            "100644",
        ],
        &dir.0,
    );
    assert_eq!(code, 0);
    assert!(stdout.contains("cannot compare: "), "{stdout}");
}

/// A member given by its own path is located like a member found by a
/// scan, whatever letter case it was typed in.
#[test]
fn a_member_path_in_any_letter_case_is_the_member() {
    let dir = workspace("archive-case");
    let (code, typed, _) = run(&["check", "lib.llb/entry.VI"], &dir.0);
    assert_eq!(code, 1);
    assert_eq!(
        slashes(&typed),
        format!("lib.llb/Entry.vi: {BZ001}\nFound 1 problem (0 errors, 1 warning) in 1 file.\n")
    );
    let (code, both, _) = run(
        &["check", "lib.llb", "lib.llb/ENTRY.vi", "lib.llb/entry.vi"],
        &dir.0,
    );
    let (_, archive, _) = run(&["check", "lib.llb"], &dir.0);
    assert_eq!(code, 1);
    assert_eq!(both, archive);
    assert_eq!(slashes(&both).matches("lib.llb/Entry.vi: ").count(), 1);

    // SARIF never has a member path as an artifact, even when the
    // archive itself was not scanned.
    let (code, stdout, _) = run(
        &[
            "check",
            "lib.llb/entry.vi",
            "lib.llb/Type.ctl",
            "--format",
            "sarif",
        ],
        &dir.0,
    );
    assert_eq!(code, 1);
    let sarif: Value = serde_json::from_str(&stdout).unwrap();
    let results = sarif["runs"][0]["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    let location = &results[0]["locations"][0];
    let uri = location["physicalLocation"]["artifactLocation"]["uri"]
        .as_str()
        .unwrap();
    assert_eq!(uri, "lib.llb");
    assert!(dir.0.join(uri).is_file());
    assert_eq!(location["logicalLocations"][0]["name"], "Entry.vi");
    assert_eq!(
        location["logicalLocations"][0]["fullyQualifiedName"],
        "lib.llb/Entry.vi"
    );
    assert_eq!(
        results[0]["partialFingerprints"]["buzzardMember/v1"],
        "BZ001:lib.llb/Entry.vi"
    );
    assert!(results[0]["message"]["text"]
        .as_str()
        .unwrap()
        .starts_with("Entry.vi: compiled code"));

    let (code, stdout, _) = run(
        &["deps", "lib.llb/ENTRY.VI", "lib.llb", "--format", "json"],
        &dir.0,
    );
    assert_eq!(code, 0);
    let json: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["files"], 4);

    write(
        &dir,
        "m.llb",
        &LlbBuilder::default()
            .member("Machine.vi", &machine_vi())
            .build(),
    );
    let (code, twice, _) = run(&["verify", "m.llb", "m.llb/machine.vi"], &dir.0);
    let (_, once, _) = run(&["verify", "m.llb"], &dir.0);
    assert_eq!(code, 1);
    assert_eq!(twice, once);
    assert!(!slashes(&twice).contains("m.llb/machine.vi"), "{twice}");
}

/// Names in an archive are data. None of them starts a line of a report
/// or reaches the terminal as a control sequence.
#[test]
fn a_member_name_cannot_forge_a_line_of_output() {
    let dir = TempDir::new("archive-forge");
    let inline = vi(&INLINE_CODE, &[]);
    write(
        &dir,
        "odd.llb",
        &LlbBuilder::default()
            .member("ok.vi", &inline)
            .member("a\nfake.vi: error[BZ999]: forged", &inline)
            .member("\u{1b}[31mred.vi", &inline)
            .member("a\\b.vi", &inline)
            .member("old.vi", &inline)
            .legacy("zeros.vi", &inline)
            .build(),
    );
    let (code, stdout, _) = run(&["check", "."], &dir.0);
    assert_eq!(code, 1);
    let lines: Vec<String> = stdout.lines().map(slashes).collect();
    let refused = |name: &str| {
        format!("odd.llb: error[BZ000]: cannot open member `{name}`: its name is not a file name")
    };
    assert_eq!(
        lines,
        [
            refused("/u{1b}[31mred.vi"),
            refused("a/b.vi"),
            refused("a/nfake.vi: error[BZ999]: forged"),
            format!("odd.llb/ok.vi: {BZ001}"),
            format!("odd.llb/old.vi: {BZ001}"),
            format!("odd.llb/zeros.vi: {BZ001}"),
            "Found 6 problems (3 errors, 3 warnings) in 4 files.".to_owned(),
        ]
    );
    assert!(!stdout.chars().any(|c| c.is_ascii_control() && c != '\n'));
    // A backslash in a name is printed once, as the archive has it.
    assert!(stdout.contains("`a\\b.vi`"), "{stdout}");
    let (code, listing, _) = run(&["inspect", "odd.llb"], &dir.0);
    assert_eq!(code, 0);
    assert!(listing.contains("| a\\b.vi | VI"), "{listing}");
    assert!(
        listing.contains("| zeros.vi | VI (`LVIN`) | unknown | legacy | "),
        "{listing}"
    );
    assert!(
        listing.contains("\nNot opened: member `a\\b.vi`: its name is not a file name.\n"),
        "{listing}"
    );
    assert!(!listing.chars().any(|c| c.is_ascii_control() && c != '\n'));
    // Such a member is not opened by its path either.
    #[cfg(unix)]
    for command in ["check", "inspect", "verify", "deps"] {
        let (code, stdout, _) = run(&[command, "odd.llb/a\\b.vi"], &dir.0);
        assert_eq!((code, stdout.as_str()), (2, ""), "{command}");
    }
}

/// A file on disk whose name holds a line break or an escape character is
/// reported under that name on one line, as a member of an archive is. The
/// file systems of some hosts refuse such a name, and the test then has
/// nothing to check there.
#[test]
fn a_file_name_cannot_forge_a_line_of_output() {
    let dir = TempDir::new("name-forge");
    let inline = vi(&INLINE_CODE, &[]);
    let hostile: [(&str, &[u8]); 3] = [
        ("a.vi\nfake.vi: error[BZ999]: forged.vi", &inline),
        ("\u{1b}[31mred.vi", &inline),
        ("bad.vi\nUnparsed files (9)\n.vi", b"nope"),
    ];
    let created: Vec<&str> = hostile
        .iter()
        .filter(|(name, bytes)| fs::write(dir.0.join(name), bytes).is_ok())
        .map(|(name, _)| *name)
        .collect();
    if created.is_empty() {
        return;
    }
    write(&dir, "ok.vi", &inline);
    // One line per finding and the summary, whatever the names hold.
    let quiet = |text: &str| !text.chars().any(|c| c.is_ascii_control() && c != '\n');
    let (code, stdout, stderr) = run(&["check", "."], &dir.0);
    assert_eq!((code, stderr.as_str()), (1, ""));
    assert!(quiet(&stdout), "{stdout:?}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), created.len() + 2, "{stdout}");
    for line in &lines[..lines.len() - 1] {
        assert!(
            line.contains(": warning[BZ001]: ") || line.contains(": error[BZ000]: "),
            "{line}"
        );
        assert!(!line.starts_with("fake.vi: error[BZ999]"), "{line}");
    }
    assert!(
        lines.contains(&format!("ok.vi: {BZ001}").as_str()),
        "{stdout}"
    );
    if created.contains(&hostile[0].0) {
        assert!(
            lines.contains(&format!("a.vi\\nfake.vi: error[BZ999]: forged.vi: {BZ001}").as_str()),
            "{stdout}"
        );
    }
    if created.contains(&hostile[1].0) {
        assert!(
            lines.contains(&format!("\\u{{1b}}[31mred.vi: {BZ001}").as_str()),
            "{stdout}"
        );
    }
    let (_, limited, _) = run(&["check", ".", "--limit", "1"], &dir.0);
    assert!(quiet(&limited), "{limited:?}");
    assert_eq!(limited.lines().count(), 3, "{limited}");

    // JSON and SARIF carry the name itself, as an escaped string.
    let (_, json, _) = run(&["check", ".", "--format", "json"], &dir.0);
    let parsed: Value = serde_json::from_str(&json).expect("valid JSON");
    let paths: Vec<&str> = parsed
        .as_array()
        .expect("a list")
        .iter()
        .map(|d| d["path"].as_str().expect("a path"))
        .collect();
    for name in &created {
        assert!(paths.contains(name), "{name:?}: {paths:?}");
    }
    let (_, sarif, _) = run(&["check", ".", "--format", "sarif"], &dir.0);
    assert!(serde_json::from_str::<Value>(&sarif).is_ok());

    // The other reports of a directory print each name on one line too.
    for args in [
        &["verify", "."][..],
        &["deps", "."],
        &["deps", ".", "--format", "markdown"],
        &["deps", ".", "--detail", "packages"],
    ] {
        let (code, stdout, stderr) = run(args, &dir.0);
        assert!(code < 2, "{args:?}: {stderr}");
        assert!(quiet(&stdout), "{args:?}: {stdout:?}");
        assert!(quiet(&stderr), "{args:?}: {stderr:?}");
        assert!(
            !stdout
                .lines()
                .any(|line| line == "Unparsed files (9)"
                    || line.starts_with("fake.vi: error[BZ999]")),
            "{args:?}: {stdout}"
        );
    }
    // Such a file is found by a scan. Named as an argument it is refused,
    // with the name on one line.
    for name in &created {
        for command in ["check", "inspect", "verify", "deps"] {
            let (code, stdout, stderr) = run(&[command, name], &dir.0);
            assert_eq!((code, stdout.as_str()), (2, ""), "{command}");
            assert!(quiet(&stderr), "{stderr:?}");
            assert_eq!(stderr.lines().count(), 1, "{stderr}");
        }
    }
}

/// A project library inside an archive is linted and inspected like one
/// on disk, and its links are part of the dependency report.
#[test]
fn a_project_library_in_an_archive_is_a_file_of_the_workspace() {
    let dir = TempDir::new("archive-library");
    let xml = "<Library LVVersion=\"23008000\">\n\
               <Item Name=\"Here.vi\" Type=\"VI\" URL=\"../Here.vi\"/>\n\
               <Item Name=\"Gone.vi\" Type=\"VI\" URL=\"../Gone.vi\"/>\n\
               <Item Name=\"T.vit\" Type=\"VI\" URL=\"/&lt;vilib&gt;/Utility/T.vit\"/>\n\
               </Library>\n";
    write(
        &dir,
        "lib.llb",
        &LlbBuilder::default()
            .member("Here.vi", &vi(&CLEAN, &[]))
            .compressed("Lib.lvlib", xml.as_bytes())
            .build(),
    );
    write(&dir, "loose/Here.vi", &vi(&CLEAN, &[]));
    write(&dir, "loose/Lib.lvlib", xml.as_bytes());
    let (code, stdout, _) = run(&["check", "lib.llb"], &dir.0);
    assert_eq!(code, 1);
    assert_eq!(
        slashes(&stdout),
        "lib.llb/Lib.lvlib: warning[BZ006]: member `Gone.vi` does not exist on disk \
         (`../Gone.vi`)\nFound 1 problem (0 errors, 1 warning) in 3 files.\n"
    );
    let (_, on_disk, _) = run(&["check", "loose"], &dir.0);
    assert_eq!(
        slashes(&stdout)
            .lines()
            .next()
            .unwrap()
            .replace("lib.llb/", "loose/"),
        slashes(&on_disk).lines().next().unwrap()
    );
    // The library is also read by its member path, in each command that
    // takes a project file.
    let (code, by_path, _) = run(&["check", "lib.llb/Lib.lvlib"], &dir.0);
    assert_eq!(code, 1);
    assert!(
        by_path.contains("BZ006") && by_path.contains("in 1 file."),
        "{by_path}"
    );
    let (code, member, stderr) = run(&["inspect", "lib.llb/lib.LVLIB"], &dir.0);
    assert_eq!((code, stderr.as_str()), (0, ""));
    let (_, loose, _) = run(&["inspect", "loose/Lib.lvlib"], &dir.0);
    assert_eq!(member, loose);
    assert!(member.contains("| Members | 3 |"), "{member}");
    // The dependency report has the library's links.
    let report = |path: &str| {
        let (code, stdout, _) = run(&["deps", path, "--format", "json"], &dir.0);
        assert_eq!(code, 0);
        serde_json::from_str::<Value>(&stdout).unwrap()
    };
    let (packed, loose) = (report("lib.llb"), report("loose"));
    assert_eq!(packed["roots"][0]["root"], "vilib");
    assert_eq!(
        packed["roots"][0]["packages"][0]["targets"][0]["path"],
        "<vilib>/Utility/T.vit"
    );
    assert_eq!(
        packed["roots"][0]["packages"][0]["name"],
        loose["roots"][0]["packages"][0]["name"]
    );
    assert_eq!(packed["missing"].as_array().unwrap().len(), 1);
    assert_eq!(
        (packed["files"].as_u64(), loose["files"].as_u64()),
        (Some(3), Some(2))
    );
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
    // The 5 files are the archive, three members that are VIs or
    // controls, and Main.vi.
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
    // The archive alone gives its members without the caller.
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
    // The two sides hold the same file, whichever side is the member.
    for (old, new) in [
        ("lib.llb/add.vi", "loose/add.vi"),
        ("loose/add.vi", "lib.llb/add.vi"),
        ("lib.llb/add.vi", "lib.llb/add.vi"),
    ] {
        let (code, stdout, stderr) = run(&["diff", old, new], &dir.0);
        assert_eq!((code, stderr.as_str()), (0, ""), "{old} {new}: {stdout}");
    }
    // Two members of two archives differ, and so do a file and a member.
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

    // A member that its archive does not hold counts as a file that is
    // not on disk, so the other side was added or deleted.
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
    // The 7 files are the archive, its three members, Main.vi, Stale.vi,
    // and the project.
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

    // With a member path as the only argument, the report holds the links
    // of that one member.
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

#[test]
fn a_file_name_cannot_forge_a_line_of_the_html_report() {
    let dir = TempDir::new("html-forge");
    // A path argument with a control character is refused, so a line
    // break or an escape reaches the page through the names of the links
    // that the file holds. The names of files reach it through the path.
    let links = ["a.vi\n<h1>forged.vi", "\u{1b}[31mred.vi"];
    let inline = vi(&INLINE_CODE, &links);
    let hostile = ["x\u{202e}iv.exe.vi", "<script>alert(1)<\\script>.vi"];
    write(&dir, "ok.vi", &inline);
    let (code, plain, _) = run(&["inspect", "ok.vi", "--format", "html"], &dir.0);
    assert_eq!(code, 0);
    let clean = |html: &str| {
        // No control character but the line breaks of the page, nothing
        // that reorders text, and no markup from a name.
        assert!(
            !html.chars().any(|c| c.is_ascii_control() && c != '\n'),
            "{html:?}"
        );
        assert!(!html.contains('\u{202e}'), "{html:?}");
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("<h1>"), "{html}");
        assert_eq!(html.matches("<h1 ").count(), 1, "{html}");
        assert_eq!(
            html.matches("<li class=\"bz-finding\">").count(),
            1,
            "{html}"
        );
    };
    clean(&plain);
    assert!(plain.contains("a.vi\\n&lt;h1&gt;forged.vi"), "{plain}");
    assert!(plain.contains("\\u{1b}[31mred.vi"), "{plain}");
    let mut tried = 0;
    for name in hostile {
        if fs::write(dir.0.join(name), &inline).is_err() {
            continue;
        }
        tried += 1;
        let (code, html, stderr) = run(&["inspect", name, "--format", "html"], &dir.0);
        assert_eq!((code, stderr.as_str()), (0, ""), "{name:?}");
        clean(&html);
        // The name is in the title, the heading, the metadata, and the
        // finding, and adds no line to any of them.
        assert_eq!(html.lines().count(), plain.lines().count(), "{html}");
    }
    let shown = |name: &str| run(&["inspect", name, "--format", "html"], &dir.0).1;
    if dir.0.join(hostile[0]).exists() {
        assert!(shown(hostile[0]).contains("<h1 class=\"bz-title\">x\\u{202e}iv.exe.vi</h1>"));
    }
    if dir.0.join(hostile[1]).exists() {
        assert!(shown(hostile[1]).contains("&lt;script&gt;alert(1)&lt;\\script&gt;.vi"));
    }
    // Every name above is legal on Linux.
    if cfg!(target_os = "linux") {
        assert_eq!(tried, hostile.len());
    }
}
