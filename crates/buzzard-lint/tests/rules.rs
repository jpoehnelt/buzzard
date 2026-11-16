//! End-to-end rule tests over synthetic workspaces written to a temp dir.

use std::fs;
use std::path::{Path, PathBuf};

use buzzard_lint::{lint, DependencyGraph, Diagnostic, LintConfig, Workspace};
use buzzard_rsrc::builder::RsrcBuilder;
use buzzard_rsrc::LvPath;

const V23: u32 = 0x2300_8000;
const V21: u32 = 0x2100_8000;
/// Flag words for a clean file: compiled code separated, nothing else set.
const CLEAN: [u32; 6] = [0, 0x400, 0, 0, 0, 0];

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("buzzard-lint-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn vi(&self, rel: &str, version: u32, words: &[u32], calls: &[&str]) {
        let links: Vec<(&str, LvPath)> = calls
            .iter()
            .map(|name| (*name, LvPath::sibling(name)))
            .collect();
        self.write(
            rel,
            RsrcBuilder::default()
                .save_record(version, words)
                .links(&links)
                .build(),
        );
    }

    fn write(&self, rel: &str, bytes: Vec<u8>) {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn check(&self) -> Vec<(String, &'static str, String)> {
        self.diagnostics()
            .into_iter()
            .map(|d| {
                let rel = d
                    .path
                    .strip_prefix(&self.0)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                (rel, d.code, d.message)
            })
            .collect()
    }

    fn diagnostics(&self) -> Vec<Diagnostic> {
        let workspace = Workspace::scan(std::slice::from_ref(&self.0));
        let graph = DependencyGraph::build(&workspace);
        lint(&workspace, &graph, &LintConfig::default())
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn codes<'a>(found: &'a [(String, &'static str, String)]) -> Vec<(&'a str, &'static str)> {
    found.iter().map(|(p, c, _)| (p.as_str(), *c)).collect()
}

#[test]
fn clean_workspace_has_no_findings() {
    let dir = TempDir::new("clean");
    dir.vi("main.vi", V23, &CLEAN, &["sub.vi"]);
    dir.vi("sub.vi", V23, &CLEAN, &[]);
    dir.write("notes.txt", b"ignored".to_vec());
    dir.write(".git/hidden.vi", b"ignored".to_vec());
    assert_eq!(dir.check(), []);
}

#[test]
fn bz000_unreadable_file() {
    let dir = TempDir::new("bz000");
    dir.write(
        "broken.vi",
        b"not an rsrc file at all, but long enough".to_vec(),
    );
    let found = dir.check();
    assert_eq!(codes(&found), [("broken.vi", "BZ000")]);
    assert!(
        found[0].2.contains("missing RSRC signature"),
        "{}",
        found[0].2
    );
}

#[test]
fn bz001_compiled_code_not_separated() {
    let dir = TempDir::new("bz001");
    dir.vi("inline.vi", V23, &[0, 0, 0, 0, 0, 0], &[]);
    dir.vi("ok.vi", V23, &CLEAN, &[]);
    assert_eq!(codes(&dir.check()), [("inline.vi", "BZ001")]);
}

#[test]
fn bz002_version_mismatch_flags_the_minority() {
    let dir = TempDir::new("bz002");
    dir.vi("a.vi", V23, &CLEAN, &[]);
    dir.vi("b.vi", V23, &CLEAN, &[]);
    dir.vi("old.vi", V21, &CLEAN, &[]);
    let found = dir.check();
    assert_eq!(codes(&found), [("old.vi", "BZ002")]);
    assert_eq!(
        found[0].2,
        "saved with version 21.0 but the workspace mostly uses 23.0 (2 files)"
    );
}

#[test]
fn bz002_tie_prefers_newer_version() {
    let dir = TempDir::new("bz002-tie");
    dir.vi("new.vi", V23, &CLEAN, &[]);
    dir.vi("old.vi", V21, &CLEAN, &[]);
    assert_eq!(codes(&dir.check()), [("old.vi", "BZ002")]);
}

#[test]
fn bz003_absolute_and_overlong_paths() {
    let dir = TempDir::new("bz003");
    let long = "d".repeat(200);
    let links = [
        ("abs.vi", LvPath::absolute(&["C", "Users", "me", "abs.vi"])),
        (
            "lib.vi",
            LvPath::absolute(&["<vilib>", "Utility", "lib.vi"]),
        ),
        (
            "long.vi",
            LvPath {
                components: vec![String::new(), long.clone(), long.clone(), "long.vi".into()],
                ..LvPath::sibling("x")
            },
        ),
    ];
    dir.write(
        "main.vi",
        RsrcBuilder::default()
            .save_record(V23, &CLEAN)
            .links(&links)
            .build(),
    );
    let found = dir.check();
    assert_eq!(codes(&found), [("main.vi", "BZ003"), ("main.vi", "BZ003")]);
    assert_eq!(
        found[0].2,
        "dependency is linked by absolute path `C:/Users/me/abs.vi`"
    );
    assert!(
        found[1]
            .2
            .ends_with("is 412 characters long, over the limit of 260"),
        "{}",
        found[1].2
    );
}

#[test]
fn bz004_debug_leftovers_and_password() {
    let dir = TempDir::new("bz004");
    dir.vi("suspend.vi", V23, &[0x1000, 0x400, 0, 0, 0, 0], &[]);
    dir.vi(
        "breakpoints.vi",
        V23,
        &[0, 0x400, 0, 0, 0, 0x2000_0000],
        &[],
    );
    dir.write(
        "locked.vi",
        RsrcBuilder::default()
            .save_record(V23, &CLEAN)
            .resource(b"BDPW", 0, None, vec![7u8; 48])
            .build(),
    );
    dir.write(
        "unlocked.vi",
        RsrcBuilder::default()
            .save_record(V23, &CLEAN)
            .resource(
                b"BDPW",
                0,
                None,
                b"\xd4\x1d\x8c\xd9\x8f\x00\xb2\x04\xe9\x80\x09\x98\xec\xf8\x42\x7e".to_vec(),
            )
            .build(),
    );
    let found = dir.check();
    assert_eq!(
        codes(&found),
        [
            ("breakpoints.vi", "BZ004"),
            ("locked.vi", "BZ004"),
            ("suspend.vi", "BZ004")
        ]
    );
}

#[test]
fn bz005_reports_every_member_of_a_cycle() {
    let dir = TempDir::new("bz005");
    dir.vi("a.vi", V23, &CLEAN, &["b.vi"]);
    dir.vi("b.vi", V23, &CLEAN, &["c.vi"]);
    dir.vi("c.vi", V23, &CLEAN, &["a.vi", "leaf.vi"]);
    dir.vi("leaf.vi", V23, &CLEAN, &[]);
    dir.vi("self.vi", V23, &CLEAN, &["self.vi"]);
    let found = dir.check();
    assert_eq!(
        codes(&found),
        [
            ("a.vi", "BZ005"),
            ("b.vi", "BZ005"),
            ("c.vi", "BZ005"),
            ("self.vi", "BZ005")
        ]
    );
    assert_eq!(
        found[0].2,
        "part of a circular dependency: a.vi -> b.vi -> c.vi -> a.vi"
    );
    assert_eq!(
        found[3].2,
        "part of a circular dependency: self.vi -> self.vi"
    );
    let related: Vec<_> = dir.diagnostics()[0]
        .related
        .iter()
        .map(|p| p.file_name().unwrap().to_owned())
        .collect();
    assert_eq!(related, ["b.vi", "c.vi"]);
}

#[test]
fn graph_answers_callers_and_callees() {
    let dir = TempDir::new("graph");
    dir.vi("main.vi", V23, &CLEAN, &["util.vi"]);
    dir.vi("other.vi", V23, &CLEAN, &["util.vi"]);
    dir.vi("util.vi", V23, &CLEAN, &[]);
    // A stale absolute link still resolves when the file name is unique.
    let stale = [("util.vi", LvPath::absolute(&["Volumes", "old", "util.vi"]))];
    dir.write(
        "sub/moved.vi",
        RsrcBuilder::default()
            .save_record(V23, &CLEAN)
            .links(&stale)
            .build(),
    );

    let workspace = Workspace::scan(std::slice::from_ref(&dir.0));
    let graph = DependencyGraph::build(&workspace);
    let names = |paths: Vec<&Path>| -> Vec<String> {
        paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    };
    assert_eq!(
        names(graph.callers(&dir.0.join("util.vi"))),
        ["main.vi", "other.vi", "moved.vi"]
    );
    assert_eq!(names(graph.callees(&dir.0.join("main.vi"))), ["util.vi"]);
    assert_eq!((graph.file_count(), graph.edge_count()), (4, 3));
    assert!(graph.cycles().is_empty());
}

#[test]
fn output_is_deterministic_and_ignores_apply() {
    let dir = TempDir::new("determinism");
    for i in 0..40 {
        let words = if i % 3 == 0 { [0u32; 6] } else { CLEAN };
        let version = if i % 7 == 0 { V21 } else { V23 };
        dir.vi(&format!("d{}/f{i}.vi", i % 4), version, &words, &[]);
    }
    let first = dir.diagnostics();
    assert!(!first.is_empty());
    for _ in 0..5 {
        assert_eq!(dir.diagnostics(), first);
    }
    let mut sorted = first.clone();
    sorted.sort();
    assert_eq!(sorted, first);

    let workspace = Workspace::scan(std::slice::from_ref(&dir.0));
    let graph = DependencyGraph::build(&workspace);
    let config = LintConfig {
        ignore: ["BZ001".to_owned()].into(),
        ..LintConfig::default()
    };
    assert!(lint(&workspace, &graph, &config)
        .iter()
        .all(|d| d.code == "BZ002"));
}

#[test]
fn real_fixtures_produce_expected_findings() {
    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../buzzard-rsrc/tests/fixtures/pylavi");
    let workspace = Workspace::scan(std::slice::from_ref(&fixtures));
    let graph = DependencyGraph::build(&workspace);
    let found = lint(&workspace, &graph, &LintConfig::default());
    let has = |file: &str, code: &str| {
        found
            .iter()
            .any(|d| d.code == code && d.path.file_name().unwrap() == file)
    };
    assert!(has("empty.vi", "BZ001"));
    assert!(!has("empty_separate_code.vi", "BZ001"));
    assert!(has("empty_password.vi", "BZ004"));
    assert!(has("add_breakpoints.vi", "BZ004"));
    assert!(has("empty_suspend_when_called.vi", "BZ004"));
    assert!(has("absolute_link_PTH.vi", "BZ003"));
    assert!(!has("relative_link.vi", "BZ003"));
    assert!(has("Inline.vi", "BZ002"));
    assert!(!found.iter().any(|d| d.code == "BZ000" || d.code == "BZ005"));
    // relative_link.vi calls its sibling empty.vi.
    let callers = graph.callers(&fixtures.join("empty.vi"));
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].file_name().unwrap(), "relative_link.vi");
}

#[test]
fn sarif_output_is_well_formed() {
    let dir = TempDir::new("sarif");
    dir.vi("a.vi", V23, &CLEAN, &["b.vi"]);
    dir.vi("b.vi", V23, &[0; 6], &["a.vi"]);
    let sarif = buzzard_lint::sarif::to_sarif(&dir.diagnostics(), "9.9.9");
    assert_eq!(sarif["version"], "2.1.0");
    let run = &sarif["runs"][0];
    assert_eq!(run["tool"]["driver"]["name"], "buzzard");
    assert_eq!(
        run["tool"]["driver"]["rules"].as_array().unwrap().len(),
        buzzard_lint::RULES.len()
    );
    let results = run["results"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    for result in results {
        let index = result["ruleIndex"].as_u64().unwrap() as usize;
        assert_eq!(
            run["tool"]["driver"]["rules"][index]["id"],
            result["ruleId"]
        );
        let uri = result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
            .as_str()
            .unwrap();
        assert!(uri.ends_with(".vi") && !uri.contains('\\'));
    }
    let cycle = results.iter().find(|r| r["ruleId"] == "BZ005").unwrap();
    assert_eq!(cycle["level"], "error");
    assert_eq!(cycle["relatedLocations"].as_array().unwrap().len(), 1);
}
