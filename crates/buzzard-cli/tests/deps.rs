//! Black-box tests of `buzzard deps` on synthetic files.

mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use buzzard_rsrc::builder::RsrcBuilder;
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
        status.code().expect("buzzard exits with a status"),
        String::from_utf8(stdout).expect("stdout is UTF-8"),
        String::from_utf8(stderr).expect("stderr is UTF-8"),
    )
}

/// The report of a run that must succeed without writing to stderr.
fn deps(args: &[&str], dir: &Path) -> String {
    let mut all = vec!["deps"];
    all.extend(args);
    let (code, stdout, stderr) = run(&all, dir);
    assert_eq!(stderr, "", "{args:?}");
    assert_eq!(code, 0, "{args:?}");
    stdout
}

fn path(kind: PathKind, text: &str) -> LvPath {
    LvPath {
        kind,
        components: text.split('/').map(str::to_owned).collect(),
    }
}

fn symbolic(text: &str) -> LvPath {
    path(PathKind::Absolute, text)
}

fn vi_bytes(links: &[LvPath]) -> Vec<u8> {
    let named: Vec<(&str, LvPath)> = links
        .iter()
        .map(|link| (link.file_name().unwrap_or("x.vi"), link.clone()))
        .collect();
    RsrcBuilder::default()
        .save_record(V23, &CLEAN)
        .links(&named)
        .build()
}

fn write(dir: &TempDir, rel: &str, bytes: &[u8]) {
    let file = dir.0.join(rel);
    fs::create_dir_all(file.parent().expect("a file has a parent")).expect("create parents");
    fs::write(file, bytes).expect("write the file");
}

fn linked(dir: &TempDir, rel: &str, links: &[LvPath]) {
    write(dir, rel, &vi_bytes(links));
}

/// A small project: two VIs and a project file with links of every kind.
fn project(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    linked(
        &dir,
        "src/Main.vi",
        &[
            symbolic("<vilib>/Utility/error.llb/Clear Errors.vi"),
            symbolic("<vilib>/Utility/error.llb/Simple Error Handler.vi"),
            symbolic("<vilib>/addons/_JKI Toolkits/State Machine/Parse State Queue.vi"),
            symbolic("<userlib>/_OpenG.lib/error/error.llb/Build Error Cluster__ogtk.vi"),
            LvPath::absolute(&["C", "Users", "me", "Scratch.vi"]),
            LvPath::sibling("Helper.vi"),
            LvPath::sibling("Removed.vi"),
        ],
    );
    linked(
        &dir,
        "src/Helper.vi",
        &[
            symbolic("<vilib>/Utility/error.llb/Clear Errors.vi"),
            symbolic("<vilib>/Utility/High Resolution Relative Seconds.vi"),
        ],
    );
    let lvproj = r#"<?xml version='1.0' encoding='UTF-8'?>
<Project Type="Project" LVVersion="23008000">
    <Item Name="My Computer" Type="My Computer">
        <Item Name="Main.vi" Type="VI" URL="../src/Main.vi"/>
        <Item Name="Notes.vi" Type="VI" URL="../src/Notes.vi"/>
        <Item Name="Dependencies" Type="Dependencies">
            <Item Name="vi.lib" Type="Folder">
                <Item Name="Clear Errors.vi" Type="VI" URL="/&lt;vilib&gt;/Utility/error.llb/Clear Errors.vi"/>
                <Item Name="lvanlys.dll" Type="Document" URL="/&lt;resource&gt;/lvanlys.dll"/>
            </Item>
        </Item>
    </Item>
</Project>
"#;
    write(&dir, "App.lvproj", lvproj.as_bytes());
    dir
}

#[test]
fn symbolic_links_are_grouped_by_root_and_package() {
    let dir = project("deps-grouped");
    // Paths of the file system follow the platform; stored paths never do.
    assert_eq!(
        deps(&[], &dir.0).replace('\\', "/"),
        "<resource> (not checked: no directory was given for root resource)
  lvanlys.dll (1 file)
    unchecked <resource>/lvanlys.dll
      used by App.lvproj

<userlib> (not checked: no directory was given for root userlib)
  _OpenG.lib/error (1 file)
    unchecked <userlib>/_OpenG.lib/error/error.llb/Build Error Cluster__ogtk.vi
      used by src/Main.vi

<vilib> (not checked: no directory was given for root vilib)
  Utility (1 file)
    unchecked <vilib>/Utility/High Resolution Relative Seconds.vi
      used by src/Helper.vi
  Utility/error.llb (2 files)
    unchecked <vilib>/Utility/error.llb/Clear Errors.vi
      used by App.lvproj
      used by src/Helper.vi
      used by src/Main.vi
    unchecked <vilib>/Utility/error.llb/Simple Error Handler.vi
      used by src/Main.vi
  addons/_JKI Toolkits/State Machine (1 file)
    unchecked <vilib>/addons/_JKI Toolkits/State Machine/Parse State Queue.vi
      used by src/Main.vi

Machine-absolute links (1)
  C:/Users/me/Scratch.vi
    used by src/Main.vi

Missing relative links (2)
  src/Notes.vi
    used by App.lvproj
  src/Removed.vi
    used by src/Main.vi

Scanned 3 files: 6 external files in 5 packages under 3 symbolic roots (0 found, 0 missing, 6 \
         unchecked), 1 machine-absolute link, 2 missing relative links, 0 unparsed files.
"
    );
}

#[test]
fn machine_absolute_and_missing_relative_links_are_listed() {
    let dir = TempDir::new("deps-absolute-missing");
    linked(
        &dir,
        "a.vi",
        &[
            LvPath::absolute(&["D", "work", "far.vi"]),
            path(PathKind::Unc, "server/share/net.vi"),
            LvPath::sibling("b.vi"),
            LvPath::sibling("gone.vi"),
            // On disk but not a file this tool analyzes.
            LvPath::sibling("notes.txt"),
            // A bare name is not a location.
            path(PathKind::Relative, "user32.dll"),
        ],
    );
    linked(&dir, "b.vi", &[LvPath::sibling("gone.vi")]);
    write(&dir, "notes.txt", b"x");
    let json: Value = serde_json::from_str(&deps(&["--format", "json"], &dir.0)).unwrap();
    assert_eq!(
        json["absolute"],
        serde_json::json!([
            { "target": "//server/share/net.vi", "referrers": ["a.vi"] },
            { "target": "D:/work/far.vi", "referrers": ["a.vi"] },
        ])
    );
    assert_eq!(
        json["missing"],
        serde_json::json!([{ "target": "gone.vi", "referrers": ["a.vi", "b.vi"] }])
    );
    assert_eq!(json["roots"], serde_json::json!([]));
    // `check` agrees about which links are absolute.
    let (_, check, _) = run(&["check", ".", "--format", "json"], &dir.0);
    assert_eq!(check.matches("linked by absolute path").count(), 2);
}

#[test]
fn a_root_with_a_directory_reports_found_and_missing() {
    let dir = project("deps-roots");
    let install = TempDir::new("deps-roots-install");
    // An `.llb` is one file; what is inside it is not looked at.
    write(&install, "vi.lib/Utility/error.llb", b"not opened");
    write(
        &install,
        "vi.lib/addons/_JKI Toolkits/State Machine/Parse State Queue.vi",
        b"x",
    );
    let vilib = install.0.join("vi.lib");
    let spec = format!("VILIB={}", vilib.display());
    let json: Value =
        serde_json::from_str(&deps(&["--root", &spec, "--format", "json"], &dir.0)).unwrap();
    let statuses = |root: &Value| -> Vec<(String, String)> {
        root["packages"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|package| package["targets"].as_array().unwrap())
            .map(|target| {
                (
                    target["path"].as_str().unwrap().to_owned(),
                    target["status"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    let roots = json["roots"].as_array().unwrap();
    assert_eq!(roots.len(), 3);
    assert_eq!(roots[2]["root"], "vilib");
    assert_eq!(
        roots[2]["directory"].as_str().unwrap(),
        vilib.to_string_lossy()
    );
    let pair = |path: &str, status: &str| (path.to_owned(), status.to_owned());
    assert_eq!(
        statuses(&roots[2]),
        [
            pair(
                "<vilib>/Utility/High Resolution Relative Seconds.vi",
                "missing"
            ),
            pair("<vilib>/Utility/error.llb/Clear Errors.vi", "found"),
            pair("<vilib>/Utility/error.llb/Simple Error Handler.vi", "found"),
            pair(
                "<vilib>/addons/_JKI Toolkits/State Machine/Parse State Queue.vi",
                "found"
            ),
        ]
    );
    // The roots that were given no directory stay unchecked.
    for root in &roots[..2] {
        assert!(root.get("directory").is_none(), "{root}");
        assert!(statuses(root)
            .iter()
            .all(|(_, status)| status == "unchecked"));
    }

    let text = deps(&["--root", &spec], &dir.0);
    assert!(
        text.contains(&format!(
            "<vilib> (checked in {}: 3 found, 1 missing)\n",
            vilib.display()
        )),
        "{text}"
    );
    assert!(text.contains("    found     <vilib>/Utility/error.llb/Clear Errors.vi\n"));
    assert!(text.contains("    missing   <vilib>/Utility/High Resolution Relative Seconds.vi\n"));
    assert!(text.contains("    unchecked <resource>/lvanlys.dll\n"));
    assert!(text.contains("(3 found, 1 missing, 2 unchecked)"), "{text}");

    // Several roots, and a directory relative to the working directory.
    write(&dir, "user.lib/_OpenG.lib/error/error.llb", b"x");
    let text = deps(&["--root", &spec, "--root", "userlib=user.lib"], &dir.0);
    assert!(
        text.contains("<userlib> (checked in user.lib: 1 found, 0 missing)\n"),
        "{text}"
    );
}

#[test]
fn an_unparsed_file_is_listed_and_the_status_stays_zero() {
    let dir = TempDir::new("deps-unparsed");
    linked(&dir, "good.vi", &[symbolic("<vilib>/a/b.vi")]);
    write(&dir, "bad.vi", b"this is not a resource file");
    write(&dir, "Bad.lvlib", b"<Library><unclosed></Library>");
    let json: Value = serde_json::from_str(&deps(&["--format", "json"], &dir.0)).unwrap();
    let unparsed = json["unparsed"].as_array().unwrap();
    assert_eq!(unparsed.len(), 2);
    assert_eq!(unparsed[0]["path"], "Bad.lvlib");
    assert!(unparsed[0]["message"]
        .as_str()
        .unwrap()
        .starts_with("invalid XML"));
    assert_eq!(unparsed[1]["path"], "bad.vi");
    assert_ne!(unparsed[1]["message"].as_str().unwrap(), "");
    assert_eq!(json["files"], 3);
    let text = deps(&[], &dir.0);
    assert!(text.contains("Unparsed files (2)\n  Bad.lvlib: cannot analyze file: invalid XML"));
    assert!(text.contains("\n  bad.vi: cannot analyze file: "));
    assert!(text.ends_with("2 unparsed files.\n"));
}

#[test]
fn errors_exit_with_status_two() {
    let dir = project("deps-errors");
    let fails = |args: &[&str], expected: &str| {
        let (code, stdout, stderr) = run(args, &dir.0);
        assert_eq!(code, 2, "{args:?}");
        assert_eq!(stdout, "", "{args:?}");
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    };
    fails(&["deps", "nowhere"], "path does not exist: nowhere");
    fails(&["deps", ".", "nowhere.vi"], "path does not exist");
    fails(&["deps", "--root", "vilib"], "not of the form NAME=DIR");
    fails(&["deps", "--root", "=src"], "not of the form NAME=DIR");
    fails(&["deps", "--root", "vilib="], "not of the form NAME=DIR");
    fails(
        &["deps", "--root", "vi lib=src"],
        "not the name of a symbolic root",
    );
    fails(&["deps", "--root", "vilib=nowhere"], "does not exist");
    fails(
        &["deps", "--root", "vilib=App.lvproj"],
        "does not exist: App.lvproj",
    );
    fs::create_dir(dir.0.join("other")).unwrap();
    fails(
        &["deps", "--root", "vilib=src", "--root", "VILIB=other"],
        "two directories",
    );
    fails(&["deps", "--format", "sarif"], "invalid value");
}

#[test]
fn output_is_byte_identical_across_runs() {
    let dir = project("deps-deterministic");
    write(&dir, "bad.vi", b"RSRC");
    for (target, root) in [(dir.0.clone(), "vilib=src"), (fixtures(), "vilib=.")] {
        for format in ["text", "json", "markdown"] {
            let args = ["--format", format, "--root", root];
            let first = deps(&args, &target);
            assert_ne!(first, "");
            for _ in 0..3 {
                assert_eq!(deps(&args, &target), first, "{format}");
            }
        }
    }
}

#[test]
fn json_has_the_documented_fields() {
    let dir = project("deps-json");
    write(&dir, "bad.vi", b"RSRC");
    let text = deps(&["--format", "json", "--root", "vilib=src"], &dir.0);
    assert!(text.ends_with("}\n"));
    let json: Value = serde_json::from_str(&text).unwrap();
    let keys =
        |value: &Value| -> Vec<String> { value.as_object().unwrap().keys().cloned().collect() };
    assert_eq!(
        keys(&json),
        ["absolute", "files", "missing", "roots", "unparsed"]
    );
    assert_eq!(json["files"], 4);
    let vilib = &json["roots"][2];
    assert_eq!(keys(vilib), ["directory", "packages", "root"]);
    assert_eq!(keys(&json["roots"][0]), ["packages", "root"]);
    let package = &vilib["packages"][1];
    assert_eq!(keys(package), ["name", "targets"]);
    assert_eq!(package["name"], "Utility/error.llb");
    let target = &package["targets"][0];
    assert_eq!(keys(target), ["path", "referrers", "status"]);
    assert_eq!(target["path"], "<vilib>/Utility/error.llb/Clear Errors.vi");
    assert_eq!(target["status"], "missing");
    assert_eq!(
        target["referrers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_str().unwrap().replace('\\', "/"))
            .collect::<Vec<_>>(),
        ["App.lvproj", "src/Helper.vi", "src/Main.vi"]
    );
    assert_eq!(keys(&json["absolute"][0]), ["referrers", "target"]);
    assert_eq!(keys(&json["missing"][0]), ["referrers", "target"]);
    assert_eq!(keys(&json["unparsed"][0]), ["message", "path"]);
}

#[test]
fn markdown_has_a_table_per_root() {
    let dir = project("deps-markdown");
    let markdown = deps(&["--format", "markdown"], &dir.0);
    assert!(markdown.starts_with("# External dependencies\n\nScanned 3 files: "));
    for heading in [
        "\n## `<resource>`\n",
        "\n## `<userlib>`\n",
        "\n## `<vilib>`\n",
        "\n## Machine-absolute links\n",
        "\n## Missing relative links\n",
    ] {
        assert!(markdown.contains(heading), "{heading}: {markdown}");
    }
    assert!(markdown.contains("| Package | File | Status | Used by |"));
    assert!(markdown.contains(
        "| `addons/_JKI Toolkits/State Machine` | `<vilib>/addons/_JKI Toolkits/State Machine/Parse State Queue.vi` | unchecked | "
    ));
    assert!(!markdown.contains("Unparsed files"));
}

/// A file cut at any length and a file with any one byte changed are
/// reported or listed as unparsed; the command never panics.
#[test]
fn truncated_and_corrupted_files_never_panic() {
    let dir = TempDir::new("deps-hostile");
    let good = vi_bytes(&[
        symbolic("<vilib>/Utility/error.llb/Clear Errors.vi"),
        LvPath::absolute(&["C", "far.vi"]),
        LvPath::sibling("gone.vi"),
        path(PathKind::Relative, "//up/and/away.vi"),
    ]);
    let mut count = 1;
    write(&dir, "whole.vi", &good);
    for cut in 0..good.len() {
        write(&dir, &format!("cut/{cut:04}.vi"), &good[..cut]);
        count += 1;
    }
    for at in 0..good.len() {
        for flip in [0x01, 0x80, 0xFF] {
            let mut bytes = good.clone();
            bytes[at] ^= flip;
            write(&dir, &format!("flip/{at:04}-{flip:02x}.vi"), &bytes);
            count += 1;
        }
    }
    let xml =
        "<Library LVVersion=\"23008000\">\n\t<Property Name=\"NI.LVClass.ParentClassLinkInfo\" \
               Type=\"Bin\">!!!!!!!!</Property>\n\t<Item Name=\"a.vi\" Type=\"VI\" \
               URL=\"/&lt;vilib&gt;/../../a.vi\"/>\n\t<Item Name=\"b\" Type=\"VI\" URL=\"///\"/>\n\
               \t<Item Name=\"c\" Type=\"VI\" URL=\"/&lt;&gt;/x\"/>\n</Library>\n";
    for cut in 0..=xml.len() {
        write(
            &dir,
            &format!("xml/{cut:04}.lvclass"),
            &xml.as_bytes()[..cut],
        );
        count += 1;
    }
    for format in ["text", "json", "markdown"] {
        let (code, stdout, stderr) = run(
            &["deps", ".", "--format", format, "--root", "vilib=cut"],
            &dir.0,
        );
        assert_eq!(stderr, "", "{format}");
        assert_eq!(code, 0, "{format}");
        if format == "json" {
            let json: Value = serde_json::from_str(&stdout).unwrap();
            assert_eq!(json["files"], count);
            assert_ne!(json["unparsed"], serde_json::json!([]));
        }
    }
}
