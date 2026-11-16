//! End-to-end rule tests over synthetic workspaces written to a temp dir.

use std::fs;
use std::path::{Path, PathBuf};

use buzzard_lint::graph::CaseSensitivity;
use buzzard_lint::project::encode_parent_link;
use buzzard_lint::{lint, DependencyGraph, Diagnostic, EdgeKind, LintConfig, Workspace};
use buzzard_rsrc::builder::RsrcBuilder;
use buzzard_rsrc::{LvPath, PathKind};

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

    /// A clean file whose links are given as `(name, path)` pairs.
    fn linked(&self, rel: &str, links: &[(&str, LvPath)]) {
        self.write(
            rel,
            RsrcBuilder::default()
                .save_record(V23, &CLEAN)
                .links(links)
                .build(),
        );
    }

    fn graph(&self, case: CaseSensitivity) -> DependencyGraph {
        let workspace = Workspace::scan(std::slice::from_ref(&self.0));
        DependencyGraph::build_with(&workspace, case)
    }

    /// Every edge as `from -> to`, with paths relative to the directory.
    fn edges(&self, graph: &DependencyGraph) -> Vec<String> {
        let rel = |p: &Path| {
            p.strip_prefix(&self.0)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        };
        graph
            .edges()
            .into_iter()
            .map(|(from, to, _)| format!("{} -> {}", rel(from), rel(to)))
            .collect()
    }

    /// This directory spelled relative to the current directory.
    #[cfg(unix)]
    fn relative_spelling(&self) -> PathBuf {
        let cwd = std::env::current_dir().unwrap();
        let mut out = PathBuf::new();
        for _ in cwd.components().skip(1) {
            out.push("..");
        }
        out.join(self.0.strip_prefix("/").unwrap())
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
fn bz002_tie_reports_nothing() {
    // With as many files on one version as on the other, neither is what
    // the workspace "mostly uses".
    let dir = TempDir::new("bz002-tie");
    dir.vi("new.vi", V23, &CLEAN, &[]);
    dir.vi("old.vi", V21, &CLEAN, &[]);
    assert_eq!(dir.check(), []);

    // A three-way tie is still a tie.
    dir.vi("older.vi", 0x2000_8000, &CLEAN, &[]);
    assert_eq!(dir.check(), []);

    // One more file makes 23.0 a strict plurality, and the others stand out.
    dir.vi("new2.vi", V23, &CLEAN, &[]);
    let found = dir.check();
    assert_eq!(codes(&found), [("old.vi", "BZ002"), ("older.vi", "BZ002")]);
    assert_eq!(
        found[0].2,
        "saved with version 21.0 but the workspace mostly uses 23.0 (2 files)"
    );
}

#[test]
fn bz003_own_path_is_measured_from_the_scan_root() {
    // 64 characters from the root, wherever the root is.
    let rel = format!("{}/{}/a.vi", "d".repeat(29), "e".repeat(29));
    let config = LintConfig {
        max_path_length: 63,
        ..LintConfig::default()
    };
    let expected = "file path is 64 characters long from the scan root, over the limit of 63";
    let own = |root: &Path| -> Vec<String> {
        let workspace = Workspace::scan(&[root.to_path_buf()]);
        let graph = DependencyGraph::build(&workspace);
        lint(&workspace, &graph, &config)
            .into_iter()
            .filter(|d| d.code == "BZ003")
            .map(|d| d.message)
            .collect()
    };

    // The same tree under a short and a long location gives one answer.
    let short = TempDir::new("bz003-own");
    short.vi(&rel, V23, &CLEAN, &[]);
    assert_eq!(own(&short.0), [expected]);
    let nested = short.0.join("x".repeat(40));
    let long = TempDir(nested.join("checkout"));
    long.vi(&rel, V23, &CLEAN, &[]);
    assert_eq!(own(&long.0), [expected]);
    // Spelling the root differently does not change it either.
    assert_eq!(own(&long.0.join("d".repeat(29)).join("..")), [expected]);
    #[cfg(unix)]
    assert_eq!(own(&long.relative_spelling()), [expected]);

    // One character shorter is within the limit.
    let config = LintConfig {
        max_path_length: 64,
        ..LintConfig::default()
    };
    let workspace = Workspace::scan(std::slice::from_ref(&long.0));
    let graph = DependencyGraph::build(&workspace);
    assert_eq!(lint(&workspace, &graph, &config), []);
    // A file given as the root itself is measured by its name.
    let file = long.0.join(&rel);
    let workspace = Workspace::scan(std::slice::from_ref(&file));
    let graph = DependencyGraph::build(&workspace);
    let config = LintConfig {
        max_path_length: 3,
        ..LintConfig::default()
    };
    let found = lint(&workspace, &graph, &config);
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].message,
        "file path is 4 characters long from the scan root, over the limit of 3"
    );
    drop(long);
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

/// A project-level XML file whose members are siblings in the same directory.
fn project_xml(root: &str, members: &[&str], extra: &str) -> Vec<u8> {
    let mut xml =
        format!("<?xml version='1.0' encoding='UTF-8'?>\n<{root} LVVersion=\"23008000\">\n");
    xml.push_str(extra);
    for member in members {
        let item_type = match member.rsplit('.').next() {
            Some("lvclass") => "LVClass",
            Some("lvlib") => "Library",
            _ => "VI",
        };
        xml.push_str(&format!(
            "\t<Item Name=\"{member}\" Type=\"{item_type}\" URL=\"../{member}\"/>\n"
        ));
    }
    xml.push_str(&format!("</{root}>\n"));
    xml.into_bytes()
}

/// The property that names a parent class by a path relative to the child.
fn parent_property(components: &[&str]) -> String {
    let link = if components[0].starts_with('<') {
        LvPath::absolute(components)
    } else {
        LvPath {
            components: components.iter().map(|c| (*c).to_owned()).collect(),
            ..LvPath::sibling("x")
        }
    };
    let name = *components.last().unwrap();
    let text = encode_parent_link(V23, &[name], &link)
        .replace('&', "&amp;")
        .replace('<', "&lt;");
    format!("\t<Property Name=\"NI.LVClass.ParentClassLinkInfo\" Type=\"Bin\">{text}</Property>\n")
}

fn file_names(paths: Vec<&Path>) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn bz000_malformed_project_file() {
    let dir = TempDir::new("bz000-xml");
    dir.write("ok.lvlib", project_xml("Library", &[], ""));
    dir.write("cut.lvproj", b"<Project><Item Name='a'".to_vec());
    dir.write("wrong.lvclass", b"<html/>".to_vec());
    dir.write(".hidden/skipped.lvproj", b"<<<".to_vec());
    let found = dir.check();
    assert_eq!(
        codes(&found),
        [("cut.lvproj", "BZ000"), ("wrong.lvclass", "BZ000")]
    );
    assert!(found[0].2.contains("invalid XML"), "{}", found[0].2);
    assert!(
        found[1].2.contains("unexpected root element"),
        "{}",
        found[1].2
    );
}

#[test]
fn bz006_missing_member() {
    let dir = TempDir::new("bz006");
    dir.vi("src/present.vi", V23, &CLEAN, &[]);
    dir.write("src/notes.txt", b"not analyzed, but it exists".to_vec());
    let xml = r#"<Project Type="Project" LVVersion="23008000">
        <Item Name="My Computer" Type="My Computer">
            <Item Name="src" Type="Folder" URL="../src">
                <Item Name="present.vi" Type="VI" URL="../src/present.vi"/>
                <Item Name="notes.txt" Type="Document" URL="../src/notes.txt"/>
                <Item Name="gone.vi" Type="VI" URL="../src/gone.vi"/>
                <Item Name="gone.vi" Type="VI" URL="../src/gone.vi"/>
            </Item>
            <Item Name="Lost.lvlib" Type="Library" URL="../lost/Lost.lvlib"/>
            <Item Name="Inner.ctl" Type="Class Private Data" URL="Inner.ctl"/>
            <Item Name="Dependencies" Type="Dependencies">
                <Item Name="Clear Errors.vi" Type="VI" URL="/&lt;vilib&gt;/Utility/error.llb/Clear Errors.vi"/>
                <Item Name="stale.vi" Type="VI" URL="../stale.vi"/>
            </Item>
        </Item>
    </Project>"#;
    dir.write("App.lvproj", xml.as_bytes().to_vec());
    dir.write("src/Lib.lvlib", project_xml("Library", &["present.vi"], ""));
    dir.write("src/Bad.lvclass", project_xml("LVClass", &["nope.vi"], ""));

    let found = dir.check();
    assert_eq!(
        found,
        [
            (
                "App.lvproj".to_owned(),
                "BZ006",
                "member `Lost.lvlib` does not exist on disk (`../lost/Lost.lvlib`)".to_owned()
            ),
            (
                "App.lvproj".to_owned(),
                "BZ006",
                "member `gone.vi` does not exist on disk (`../src/gone.vi`)".to_owned()
            ),
            (
                "src/Bad.lvclass".to_owned(),
                "BZ006",
                "member `nope.vi` does not exist on disk (`../nope.vi`)".to_owned()
            ),
        ]
    );
    let diagnostics = dir.diagnostics();
    assert!(diagnostics
        .iter()
        .all(|d| d.severity == buzzard_lint::Severity::Warning));

    // Creating the file fixes the finding; the rule can also be ignored.
    dir.vi("src/nope.vi", V23, &CLEAN, &[]);
    assert_eq!(
        codes(&dir.check()),
        [("App.lvproj", "BZ006"), ("App.lvproj", "BZ006")]
    );
    let workspace = Workspace::scan(std::slice::from_ref(&dir.0));
    let graph = DependencyGraph::build(&workspace);
    let config = LintConfig {
        ignore: ["BZ006".to_owned()].into(),
        ..LintConfig::default()
    };
    assert_eq!(lint(&workspace, &graph, &config), []);
    let rule = buzzard_lint::rules::rule("BZ006").unwrap();
    assert_eq!(rule.name, "missing-member");
}

#[test]
fn graph_answers_membership_and_inheritance() {
    let dir = TempDir::new("graph-project");
    dir.vi("Base/Init.vi", V23, &CLEAN, &[]);
    dir.vi("Base/Run.vi", V23, &CLEAN, &["Init.vi"]);
    dir.write(
        "Base/Base.lvclass",
        project_xml("LVClass", &["Run.vi", "Init.vi"], ""),
    );
    for child in ["Zeta", "Alpha"] {
        dir.vi(&format!("{child}/Run {child}.vi"), V23, &CLEAN, &[]);
        dir.write(
            &format!("{child}/{child}.lvclass"),
            project_xml(
                "LVClass",
                &[&format!("Run {child}.vi")],
                &parent_property(&["", "", "Base", "Base.lvclass"]),
            ),
        );
    }
    dir.write(
        "Alpha/Leaf/Leaf.lvclass",
        project_xml("LVClass", &[], &parent_property(&["", "", "Alpha.lvclass"])),
    );
    // A parent stored by its installed location is found by its unique name.
    dir.write(
        "Installed.lvclass",
        project_xml(
            "LVClass",
            &[],
            &parent_property(&["<vilib>", "Vendor", "Base.lvclass"]),
        ),
    );
    dir.write(
        "Base/Kit.lvlib",
        project_xml("Library", &["Base.lvclass", "Init.vi"], ""),
    );
    let project = r#"<Project Type="Project" LVVersion="23008000">
        <Item Name="My Computer" Type="My Computer">
            <Item Name="Kit.lvlib" Type="Library" URL="../Base/Kit.lvlib"/>
            <Item Name="Zeta.lvclass" Type="LVClass" URL="../Zeta/Zeta.lvclass"/>
            <Item Name="Alpha.lvclass" Type="LVClass" URL="../Alpha/Alpha.lvclass"/>
            <Item Name="Dependencies" Type="Dependencies">
                <Item Name="Init.vi" Type="VI" URL="../Base/Init.vi"/>
            </Item>
        </Item>
    </Project>"#;
    dir.write("App.lvproj", project.as_bytes().to_vec());
    assert_eq!(dir.check(), []);

    let workspace = Workspace::scan(std::slice::from_ref(&dir.0));
    let graph = DependencyGraph::build(&workspace);
    let at = |rel: &str| dir.0.join(rel);

    assert_eq!(
        file_names(graph.members(&at("Base/Base.lvclass"))),
        ["Init.vi", "Run.vi"]
    );
    assert_eq!(
        file_names(graph.members(&at("App.lvproj"))),
        ["Alpha.lvclass", "Kit.lvlib", "Zeta.lvclass"]
    );
    assert_eq!(
        file_names(graph.owners(&at("Base/Init.vi"))),
        ["Base.lvclass", "Kit.lvlib"]
    );
    assert_eq!(
        file_names(graph.owners(&at("Base/Base.lvclass"))),
        ["Kit.lvlib"]
    );
    assert!(graph.owners(&at("App.lvproj")).is_empty());
    assert!(graph.members(&at("Base/Init.vi")).is_empty());
    assert!(graph.members(&at("missing.lvlib")).is_empty());

    assert_eq!(
        graph.parent_class(&at("Zeta/Zeta.lvclass")),
        Some(at("Base/Base.lvclass").as_path())
    );
    assert_eq!(
        graph.parent_class(&at("Alpha/Leaf/Leaf.lvclass")),
        Some(at("Alpha/Alpha.lvclass").as_path())
    );
    assert_eq!(
        graph.parent_class(&at("Installed.lvclass")),
        Some(at("Base/Base.lvclass").as_path())
    );
    assert_eq!(graph.parent_class(&at("Base/Base.lvclass")), None);
    assert_eq!(graph.parent_class(&at("Base/Kit.lvlib")), None);
    assert_eq!(
        file_names(graph.child_classes(&at("Base/Base.lvclass"))),
        ["Alpha.lvclass", "Installed.lvclass", "Zeta.lvclass"]
    );
    assert_eq!(
        file_names(graph.child_classes(&at("Alpha/Alpha.lvclass"))),
        ["Leaf.lvclass"]
    );

    // Callers and callees keep meaning "uses": neither membership nor
    // inheritance shows up there.
    assert_eq!(file_names(graph.callers(&at("Base/Init.vi"))), ["Run.vi"]);
    assert!(graph.callers(&at("Base/Base.lvclass")).is_empty());
    assert!(graph.callees(&at("Zeta/Zeta.lvclass")).is_empty());
    assert!(graph.callees(&at("App.lvproj")).is_empty());

    let count = |kind| graph.edges().iter().filter(|e| e.2 == kind).count();
    assert_eq!(
        (
            count(EdgeKind::Uses),
            count(EdgeKind::Contains),
            count(EdgeKind::Inherits)
        ),
        (1, 9, 4)
    );
    assert_eq!(graph.edge_count(), 14);
    let edges = graph.edges();
    let mut sorted = edges.clone();
    sorted.sort();
    assert_eq!(edges, sorted);
}

#[test]
fn bz005_membership_is_not_a_cycle_but_inheritance_is() {
    let dir = TempDir::new("bz005-classes");
    // A member that refers back to its owning class is normal.
    dir.vi("Owner/Method.vi", V23, &CLEAN, &["Owner.lvclass"]);
    dir.write(
        "Owner/Owner.lvclass",
        project_xml("LVClass", &["Method.vi"], ""),
    );
    assert_eq!(dir.check(), []);
    let workspace = Workspace::scan(std::slice::from_ref(&dir.0));
    let graph = DependencyGraph::build(&workspace);
    assert_eq!(
        file_names(graph.callers(&dir.0.join("Owner/Owner.lvclass"))),
        ["Method.vi"]
    );

    // Two classes that name each other as parent cannot be loaded.
    for (class, parent) in [("A", "B"), ("B", "A")] {
        let parent_file = format!("{parent}.lvclass");
        dir.write(
            &format!("{class}.lvclass"),
            project_xml("LVClass", &[], &parent_property(&["", &parent_file])),
        );
    }
    let found = dir.check();
    assert_eq!(
        codes(&found),
        [("A.lvclass", "BZ005"), ("B.lvclass", "BZ005")]
    );
    assert_eq!(
        found[0].2,
        "part of a circular dependency: A.lvclass -> B.lvclass -> A.lvclass"
    );
}

#[test]
fn project_files_are_scanned_deterministically() {
    let dir = TempDir::new("determinism-projects");
    for i in 0..30usize {
        let folder = format!("d{}", i % 5);
        dir.vi(&format!("{folder}/f{i}.vi"), V23, &CLEAN, &[]);
        let vi = format!("f{i}.vi");
        let members: [&str; 2] = [&vi, "missing.vi"];
        // Each class inherits from the previous one, in another directory.
        let parent = match i.checked_sub(1) {
            Some(p) => parent_property(&["", "", &format!("d{}", p % 5), &format!("C{p}.lvclass")]),
            None => String::new(),
        };
        dir.write(
            &format!("{folder}/C{i}.lvclass"),
            project_xml("LVClass", &members[..1 + i % 2], &parent),
        );
    }
    let first = dir.diagnostics();
    assert_eq!(first.len(), 15);
    assert!(first.iter().all(|d| d.code == "BZ006"));
    let workspace = Workspace::scan(std::slice::from_ref(&dir.0));
    let paths: Vec<_> = workspace.files().keys().cloned().collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
    assert_eq!(paths.len(), 60);
    let graph = DependencyGraph::build(&workspace);
    assert_eq!(
        graph.parent_class(&dir.0.join("d0/C10.lvclass")),
        Some(dir.0.join("d4/C9.lvclass").as_path())
    );
    let edges = graph
        .edges()
        .into_iter()
        .map(|(a, b, k)| (a.to_path_buf(), b.to_path_buf(), k))
        .collect::<Vec<_>>();
    for _ in 0..5 {
        assert_eq!(dir.diagnostics(), first);
        let again = Workspace::scan(std::slice::from_ref(&dir.0));
        assert_eq!(again, workspace);
        let again_edges = DependencyGraph::build(&again)
            .edges()
            .into_iter()
            .map(|(a, b, k)| (a.to_path_buf(), b.to_path_buf(), k))
            .collect::<Vec<_>>();
        assert_eq!(again_edges, edges);
    }
}

#[test]
fn real_project_fixtures_scan_without_parse_errors() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project");
    let workspace = Workspace::scan(std::slice::from_ref(&fixtures));
    assert_eq!(workspace.files().len(), 3);
    assert!(workspace.files().values().all(|s| s.project().is_some()));
    let graph = DependencyGraph::build(&workspace);
    let found = lint(&workspace, &graph, &LintConfig::default());
    // The members were not copied along with the three files.
    assert!(!found.is_empty());
    assert!(found.iter().all(|d| d.code == "BZ006"), "{found:#?}");
}

#[test]
fn symbolic_links_never_bind_to_a_workspace_file_of_the_same_name() {
    let dir = TempDir::new("symbolic");
    let name = "Path To Command Line String.vi";
    // A wrapper named like the installed VI it calls.
    let vilib = LvPath::absolute(&["<vilib>", "AdvancedString", name]);
    dir.linked(name, &[(name, vilib.clone())]);
    // Common names: an unrelated project file must not become the callee.
    dir.linked("drivers/Open.vi", &[]);
    let open = LvPath::absolute(&["<userlib>", "instr", "Open.vi"]);
    dir.linked("main.vi", &[("Open.vi", open), (name, vilib)]);
    // Neither does a link that is not a path at all.
    let not_a_path = LvPath {
        kind: PathKind::NotAPath,
        components: vec!["Open.vi".into()],
    };
    dir.linked("other.vi", &[("Open.vi", not_a_path)]);

    let graph = dir.graph(CaseSensitivity::Sensitive);
    assert_eq!(dir.edges(&graph), [] as [&str; 0]);
    assert!(graph.cycles().is_empty());
    assert_eq!(dir.check(), []);
}

#[test]
fn stale_links_fall_back_to_a_unique_name_but_never_to_the_file_itself() {
    let dir = TempDir::new("stale");
    dir.linked("lib/util.vi", &[]);
    // Absolute path from another machine.
    let foreign = LvPath::absolute(&["Volumes", "old", "util.vi"]);
    dir.linked("a.vi", &[("util.vi", foreign)]);
    // Relative path that no longer leads anywhere.
    dir.linked("b.vi", &[("util.vi", LvPath::sibling("util.vi"))]);
    // A stale link to something with the file's own name is not a
    // self-reference.
    let own = LvPath::absolute(&["Volumes", "old", "c.vi"]);
    dir.linked("c.vi", &[("c.vi", own)]);
    let gone = LvPath {
        kind: PathKind::Relative,
        components: vec!["".into(), "elsewhere".into(), "d.vi".into()],
    };
    dir.linked("d.vi", &[("d.vi", gone)]);

    let graph = dir.graph(CaseSensitivity::Sensitive);
    assert_eq!(
        dir.edges(&graph),
        ["a.vi -> lib/util.vi", "b.vi -> lib/util.vi"]
    );
    assert!(graph.cycles().is_empty());
    assert!(!dir.check().iter().any(|(_, code, _)| *code == "BZ005"));
}

#[test]
fn absolute_links_match_files_scanned_under_another_spelling() {
    let dir = TempDir::new("identity");
    // Two files share a name, so only the exact path can tell them apart.
    dir.linked("a/Init.vi", &[]);
    dir.linked("b/Init.vi", &[]);
    let target = dir.0.join("b/Init.vi");
    let components: Vec<String> = target
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            std::path::Component::Prefix(prefix) => match prefix.kind() {
                std::path::Prefix::Disk(letter) => Some(char::from(letter).to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let link = LvPath {
        kind: PathKind::Absolute,
        components,
    };
    dir.linked("main.vi", &[("Init.vi", link)]);

    let expected = ["main.vi -> b/Init.vi"];
    assert_eq!(
        dir.edges(&dir.graph(CaseSensitivity::Sensitive)),
        expected,
        "absolute root"
    );

    // The root spelled with a detour, and relative to the current directory.
    let spellings = [
        dir.0.join("a/.."),
        #[cfg(unix)]
        dir.relative_spelling(),
    ];
    for root in spellings {
        let workspace = Workspace::scan(std::slice::from_ref(&root));
        let graph = DependencyGraph::build_with(&workspace, CaseSensitivity::Sensitive);
        let edges: Vec<String> = graph
            .edges()
            .into_iter()
            .map(|(from, to, _)| {
                let name = |p: &Path| {
                    let parent = p.parent().and_then(Path::file_name).unwrap();
                    let file = p.file_name().unwrap();
                    format!("{}/{}", parent.to_string_lossy(), file.to_string_lossy())
                };
                format!(
                    "{} -> {}",
                    from.file_name().unwrap().to_string_lossy(),
                    name(to)
                )
            })
            .collect();
        assert_eq!(edges, expected, "{}", root.display());
        // Queries accept any spelling of a file's path, too.
        let callers = graph.callers(&dir.0.join("b/./Init.vi"));
        assert_eq!(callers.len(), 1, "{}", root.display());
        assert_eq!(callers[0].file_name().unwrap(), "main.vi");
    }
}

#[test]
fn a_file_reached_through_two_roots_is_analyzed_once() {
    let dir = TempDir::new("two-roots");
    dir.vi("sub/a.vi", V23, &[0; 6], &["b.vi"]);
    dir.vi("sub/b.vi", V23, &CLEAN, &[]);

    let once = Workspace::scan(std::slice::from_ref(&dir.0));
    let mut roots = vec![dir.0.clone(), dir.0.join("sub"), dir.0.join("sub/../sub")];
    #[cfg(unix)]
    roots.push(dir.relative_spelling());
    let twice = Workspace::scan(&roots);
    // The spelling of the first root is the one that is kept.
    let keys = |w: &Workspace| -> Vec<PathBuf> { w.files().keys().cloned().collect() };
    assert_eq!(keys(&twice), keys(&once));
    assert_eq!(keys(&twice).len(), 2);
    assert_eq!(
        buzzard_lint::workspace::discover(&roots),
        buzzard_lint::workspace::discover(std::slice::from_ref(&dir.0))
    );

    let graph = DependencyGraph::build(&twice);
    assert_eq!((graph.file_count(), graph.edge_count()), (2, 1));
    let found = lint(&twice, &graph, &LintConfig::default());
    assert_eq!(
        found,
        lint(
            &once,
            &DependencyGraph::build(&once),
            &LintConfig::default()
        )
    );
    assert_eq!(found.len(), 1);

    // With the roots in the other order the first spelling still wins.
    #[cfg(unix)]
    {
        let relative = dir.relative_spelling();
        let workspace = Workspace::scan(&[relative.clone(), dir.0.clone()]);
        assert_eq!(
            keys(&workspace),
            [relative.join("sub/a.vi"), relative.join("sub/b.vi")]
        );
    }
}

#[test]
fn case_insensitive_lookup_is_a_fallback_for_the_exact_one() {
    let dir = TempDir::new("case");
    // The link was stored as `Util.vi`; the file is now `util.vi`.
    dir.linked("a.vi", &[("Util.vi", LvPath::sibling("Util.vi"))]);
    dir.linked("util.vi", &[("A.VI", LvPath::sibling("A.VI"))]);
    // A directory component that differs in case.
    let nested = LvPath {
        kind: PathKind::Relative,
        components: vec!["".into(), "Lib".into(), "Deep.vi".into()],
    };
    dir.linked("b.vi", &[("Deep.vi", nested)]);
    dir.linked("lib/deep.vi", &[]);
    // A stale path whose name differs in case.
    let stale = LvPath::absolute(&["Volumes", "old", "DEEP.VI"]);
    dir.linked("c.vi", &[("DEEP.VI", stale)]);

    let sensitive = dir.graph(CaseSensitivity::Sensitive);
    assert_eq!(dir.edges(&sensitive), [] as [&str; 0]);
    assert!(sensitive.cycles().is_empty());

    let insensitive = dir.graph(CaseSensitivity::Insensitive);
    assert_eq!(
        dir.edges(&insensitive),
        [
            "a.vi -> util.vi",
            "b.vi -> lib/deep.vi",
            "c.vi -> lib/deep.vi",
            "util.vi -> a.vi"
        ]
    );
    // The cycle a.vi <-> util.vi is now visible.
    assert_eq!(insensitive.cycles().len(), 1);
    // Queries find a file whatever the case of the path they are given.
    assert_eq!(insensitive.callers(&dir.0.join("UTIL.vi")).len(), 1);
    assert_eq!(sensitive.callers(&dir.0.join("UTIL.vi")).len(), 0);
}

/// Two files whose names differ only in case can only be created where the
/// file system tells them apart.
#[cfg(target_os = "linux")]
#[test]
fn case_insensitive_lookup_prefers_exact_and_refuses_ambiguity() {
    let dir = TempDir::new("case-ambiguous");
    dir.linked("util.vi", &[]);
    dir.linked("UTIL.vi", &[]);
    dir.linked("exact.vi", &[("UTIL.vi", LvPath::sibling("UTIL.vi"))]);
    dir.linked("vague.vi", &[("Util.vi", LvPath::sibling("Util.vi"))]);
    let stale = LvPath::absolute(&["Volumes", "old", "uTiL.vi"]);
    dir.linked("stale.vi", &[("uTiL.vi", stale)]);

    for case in [CaseSensitivity::Sensitive, CaseSensitivity::Insensitive] {
        let graph = dir.graph(case);
        assert_eq!(dir.edges(&graph), ["exact.vi -> UTIL.vi"], "{case:?}");
    }
}

/// `テスト.vi` as Japanese LabVIEW stores it: Shift-JIS bytes.
const SJIS_NAME: &[u8] = b"\x83\x65\x83\x58\x83\x67.vi";

fn byte_links(links: &[buzzard_rsrc::builder::ByteLink<'_>]) -> Vec<u8> {
    RsrcBuilder::default()
        .save_record(V23, &CLEAN)
        .links_bytes(links)
        .build()
}

#[test]
fn links_stored_in_a_legacy_code_page_resolve() {
    let dir = TempDir::new("shift-jis");
    dir.linked("テスト.vi", &[]);
    // A sibling link, and a link through a directory that is also Shift-JIS
    // (`日本`).
    dir.write(
        "main.vi",
        byte_links(&[(SJIS_NAME, PathKind::Relative, &[b"", SJIS_NAME])]),
    );
    let folder: &[u8] = b"\x93\xfa\x96\x7b";
    dir.linked("日本/深い.vi", &[]);
    let deep: &[u8] = b"\x90\x5b\x82\xa2.vi";
    dir.write(
        "nested.vi",
        byte_links(&[(deep, PathKind::Relative, &[b"", folder, deep])]),
    );
    // A stale absolute path still finds the file by its recovered name.
    dir.write(
        "sub/stale.vi",
        byte_links(&[(
            SJIS_NAME,
            PathKind::Absolute,
            &[b"C", b"\x8c\xc3\x82\xa2", SJIS_NAME],
        )]),
    );

    let workspace = Workspace::scan(std::slice::from_ref(&dir.0));
    // The parser shows the bytes one character each; the graph recovers them.
    let stored = &workspace.get(&dir.0.join("main.vi")).unwrap();
    let stored = stored.metadata().unwrap().links[0].name().unwrap();
    assert_eq!(stored, "\u{83}e\u{83}X\u{83}g.vi");

    let graph = DependencyGraph::build_with(&workspace, CaseSensitivity::Sensitive);
    assert_eq!(
        dir.edges(&graph),
        [
            "main.vi -> テスト.vi",
            "nested.vi -> 日本/深い.vi",
            "sub/stale.vi -> テスト.vi"
        ]
    );
    let callers = graph.callers(&dir.0.join("テスト.vi"));
    assert_eq!(file_names(callers), ["main.vi", "stale.vi"]);
}

#[test]
fn code_page_recovery_needs_a_single_candidate() {
    // 0xC4 0xE3 is `你` in GBK, `Дг` in Windows-1251 and `Äã` in Windows-1252.
    let name: &[u8] = b"\xc4\xe3.vi";
    let link = byte_links(&[(name, PathKind::Relative, &[b"", name])]);

    let dir = TempDir::new("code-page-one");
    dir.linked("你.vi", &[]);
    dir.write("main.vi", link.clone());
    let graph = dir.graph(CaseSensitivity::Sensitive);
    assert_eq!(dir.edges(&graph), ["main.vi -> 你.vi"]);

    // With a file for each reading there is no telling which one is meant.
    dir.linked("Дг.vi", &[]);
    let graph = dir.graph(CaseSensitivity::Sensitive);
    assert_eq!(dir.edges(&graph), [] as [&str; 0]);

    // A name that is right as stored is taken as stored, whatever else
    // its bytes could mean.
    dir.linked("\u{c4}\u{e3}.vi", &[]);
    let graph = dir.graph(CaseSensitivity::Sensitive);
    assert_eq!(dir.edges(&graph), ["main.vi -> Äã.vi"]);

    // Bytes that are valid in no candidate reading match nothing, and plain
    // ASCII names are never reinterpreted.
    let dir = TempDir::new("code-page-none");
    dir.linked("テスト.vi", &[]);
    dir.linked("main.vi", &[("missing.vi", LvPath::sibling("missing.vi"))]);
    let graph = dir.graph(CaseSensitivity::Sensitive);
    assert_eq!(dir.edges(&graph), [] as [&str; 0]);
}
