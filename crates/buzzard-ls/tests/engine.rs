//! Incremental behaviour of the analysis engine, without the protocol layer.

use std::fs;
use std::path::{Path, PathBuf};

use buzzard_lint::project::encode_parent_link;
use buzzard_lint::{FileState, LintConfig};
use buzzard_ls::engine::{Engine, SymbolKind, Update};
use buzzard_rsrc::builder::RsrcBuilder;
use buzzard_rsrc::LvPath;

const V23: u32 = 0x2300_8000;
const CLEAN: [u32; 6] = [0, 0x400, 0, 0, 0, 0];
const INLINE_CODE: [u32; 6] = [0; 6];

struct Fixture {
    dir: PathBuf,
    engine: Engine,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("buzzard-ls-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let engine = Engine::new(vec![dir.clone()], LintConfig::default());
        Self { dir, engine }
    }

    fn vi(&self, name: &str, words: &[u32], calls: &[&str]) {
        let links: Vec<(&str, LvPath)> = calls.iter().map(|n| (*n, LvPath::sibling(n))).collect();
        let bytes = RsrcBuilder::default()
            .save_record(V23, words)
            .links(&links)
            .build();
        fs::write(self.dir.join(name), bytes).unwrap();
    }

    /// Tell the engine a file changed on disk, as the watcher would.
    fn touch(&mut self, name: &str) -> Vec<(String, Vec<&'static str>)> {
        let path = self.dir.join(name);
        let state = path.is_file().then(|| FileState::load(&path));
        let updates = self.engine.apply(vec![(path, state)]);
        self.simplify(updates)
    }

    fn simplify(&self, updates: Vec<Update>) -> Vec<(String, Vec<&'static str>)> {
        updates
            .into_iter()
            .map(|(path, diagnostics)| {
                let name = path
                    .strip_prefix(&self.dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                (name, diagnostics.iter().map(|d| d.code).collect())
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn publishes_only_what_changed() {
    let mut fx = Fixture::new("incremental");
    fx.vi("main.vi", &CLEAN, &["util.vi"]);
    fx.vi("util.vi", &INLINE_CODE, &[]);

    let updates = fx.engine.scan();
    assert_eq!(
        fx.simplify(updates),
        [("util.vi".to_owned(), vec!["BZ001"])]
    );

    // Nothing changed on disk: nothing to publish.
    assert_eq!(fx.touch("util.vi"), []);

    // Fixing the file clears its diagnostics.
    fx.vi("util.vi", &CLEAN, &[]);
    assert_eq!(fx.touch("util.vi"), [("util.vi".to_owned(), vec![])]);

    // A new edge closes a cycle; both members are reported.
    fx.vi("util.vi", &CLEAN, &["main.vi"]);
    assert_eq!(
        fx.touch("util.vi"),
        [
            ("main.vi".to_owned(), vec!["BZ005"]),
            ("util.vi".to_owned(), vec!["BZ005"])
        ]
    );

    // Deleting one member clears the cycle everywhere.
    fs::remove_file(fx.dir.join("util.vi")).unwrap();
    assert_eq!(
        fx.touch("util.vi"),
        [
            ("main.vi".to_owned(), vec![]),
            ("util.vi".to_owned(), vec![])
        ]
    );
    assert!(fx.engine.metadata(&fx.dir.join("util.vi")).is_none());

    // A file that becomes unreadable is reported, not dropped.
    fs::write(fx.dir.join("main.vi"), b"garbage").unwrap();
    assert_eq!(fx.touch("main.vi"), [("main.vi".to_owned(), vec!["BZ000"])]);
}

#[test]
fn answers_navigation_queries() {
    let mut fx = Fixture::new("navigation");
    fx.vi("Main.vi", &CLEAN, &["Read Config.vi", "Type.ctl"]);
    fx.vi("Test Main.vi", &CLEAN, &["Read Config.vi"]);
    fx.vi("Read Config.vi", &CLEAN, &[]);
    let control = RsrcBuilder::new(buzzard_rsrc::header::file_type::CONTROL)
        .save_record(V23, &CLEAN)
        .build();
    fs::write(fx.dir.join("Type.ctl"), control).unwrap();
    fs::write(fx.dir.join("App.lvproj"), "<Project/>").unwrap();
    fs::write(fx.dir.join("Motor.lvclass"), "<Library/>").unwrap();
    assert_eq!(fx.engine.scan(), []);

    let names = |paths: Vec<PathBuf>| -> Vec<String> {
        paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    };
    assert_eq!(
        names(fx.engine.callers(&fx.dir.join("Read Config.vi"))),
        ["Main.vi", "Test Main.vi"]
    );
    assert_eq!(
        names(fx.engine.callees(&fx.dir.join("Main.vi"))),
        ["Read Config.vi", "Type.ctl"]
    );
    assert_eq!(
        names(fx.engine.callers(&fx.dir.join("missing.vi"))),
        Vec::<String>::new()
    );

    let hover = fx.engine.hover(&fx.dir.join("Read Config.vi")).unwrap();
    assert!(hover.contains("### Read Config.vi"), "{hover}");
    assert!(hover.contains("| Saved version | 23.0.0 |"), "{hover}");
    assert!(hover.contains("| Callers in workspace | 2 |"), "{hover}");
    assert!(fx.engine.hover(&fx.dir.join("missing.vi")).is_none());
    let hover = fx.engine.hover(&fx.dir.join("App.lvproj")).unwrap();
    assert!(hover.contains("| Kind | Project |"), "{hover}");
    assert!(hover.contains("| Members | 0 |"), "{hover}");

    let all = fx.engine.symbols("", 100);
    assert_eq!(all.len(), 6);
    let kind = |name: &str| all.iter().find(|s| s.name == name).unwrap().kind;
    assert_eq!(kind("Main.vi"), SymbolKind::Vi);
    assert_eq!(kind("Type.ctl"), SymbolKind::Control);
    assert_eq!(kind("Motor.lvclass"), SymbolKind::Class);
    assert_eq!(kind("App.lvproj"), SymbolKind::Project);

    let found: Vec<String> = fx
        .engine
        .symbols("main", 100)
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(found, ["Main.vi", "Test Main.vi"]);
    assert_eq!(fx.engine.symbols("rdcfg", 100)[0].name, "Read Config.vi");
    assert_eq!(fx.engine.symbols("", 2).len(), 2);
    assert!(fx.engine.symbols("zzz", 100).is_empty());
}

fn class_xml(members: &[&str], parent: Option<&str>) -> String {
    let mut xml = String::from("<LVClass LVVersion=\"23008000\">\n");
    if let Some(parent) = parent {
        let text = encode_parent_link(V23, &[parent], &LvPath::sibling(parent))
            .replace('&', "&amp;")
            .replace('<', "&lt;");
        xml.push_str(&format!(
            "<Property Name=\"NI.LVClass.ParentClassLinkInfo\" Type=\"Bin\">{text}</Property>\n"
        ));
    }
    for member in members {
        xml.push_str(&format!(
            "<Item Name=\"{member}\" Type=\"VI\" URL=\"../{member}\"/>\n"
        ));
    }
    xml.push_str("</LVClass>\n");
    xml
}

fn project_xml(members: &[&str]) -> String {
    let mut xml = String::from(
        "<Project Type=\"Project\" LVVersion=\"23008000\">\n<Item Name=\"My Computer\" Type=\"My Computer\">\n",
    );
    for member in members {
        xml.push_str(&format!(
            "<Item Name=\"{member}\" Type=\"LVClass\" URL=\"../{member}\"/>\n"
        ));
    }
    xml.push_str(
        "<Item Name=\"Dependencies\" Type=\"Dependencies\">\n\
         <Item Name=\"x.vi\" Type=\"VI\" URL=\"/&lt;vilib&gt;/x.vi\"/>\n</Item>\n</Item>\n</Project>\n",
    );
    xml
}

#[test]
fn parses_and_tracks_project_files() {
    let mut fx = Fixture::new("projects");
    let names = |paths: Vec<PathBuf>| -> Vec<String> {
        paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    };
    let dir = fx.dir.clone();
    let at = |name: &str| dir.join(name);
    let write = |name: &str, text: &str| fs::write(at(name), text).unwrap();
    fx.vi("Run.vi", &CLEAN, &[]);
    fx.vi("Stop.vi", &CLEAN, &[]);
    write("Base.lvclass", &class_xml(&["Run.vi", "Stop.vi"], None));
    write("Child.lvclass", &class_xml(&[], Some("Base.lvclass")));
    write(
        "App.lvproj",
        &project_xml(&["Base.lvclass", "Child.lvclass", "Gone.lvclass"]),
    );
    fs::create_dir_all(at(".cache")).unwrap();
    write(".cache/Hidden.lvlib", "<Library/>");

    // The one scan finds and parses the XML files along with the binaries.
    let updates = fx.engine.scan();
    assert_eq!(
        fx.simplify(updates),
        [("App.lvproj".to_owned(), vec!["BZ006"])]
    );
    assert!(fx.engine.project(&at("Base.lvclass")).is_some());
    assert!(fx.engine.project(&at("Run.vi")).is_none());
    assert!(fx.engine.metadata(&at("Base.lvclass")).is_none());
    assert_eq!(
        names(fx.engine.members(&at("Base.lvclass"))),
        ["Run.vi", "Stop.vi"]
    );
    assert_eq!(
        names(fx.engine.members(&at("App.lvproj"))),
        ["Base.lvclass", "Child.lvclass"]
    );
    assert_eq!(names(fx.engine.owners(&at("Run.vi"))), ["Base.lvclass"]);
    assert_eq!(
        fx.engine.parent_class(&at("Child.lvclass")),
        Some(at("Base.lvclass"))
    );
    assert_eq!(fx.engine.parent_class(&at("Base.lvclass")), None);
    assert_eq!(
        names(fx.engine.child_classes(&at("Base.lvclass"))),
        ["Child.lvclass"]
    );

    // Symbols come from the parsed workspace; hidden directories stay out.
    let all = fx.engine.symbols("", 100);
    let listed: Vec<(&str, SymbolKind)> = all.iter().map(|s| (s.name.as_str(), s.kind)).collect();
    assert_eq!(
        listed,
        [
            ("App.lvproj", SymbolKind::Project),
            ("Base.lvclass", SymbolKind::Class),
            ("Child.lvclass", SymbolKind::Class),
            ("Run.vi", SymbolKind::Vi),
            ("Stop.vi", SymbolKind::Vi),
        ]
    );

    let hover = fx.engine.hover(&at("Child.lvclass")).unwrap();
    assert!(hover.starts_with("### Child.lvclass\n"), "{hover}");
    assert!(hover.contains("| Kind | Class |"), "{hover}");
    assert!(hover.contains("| Saved version | 23.0.0 |"), "{hover}");
    assert!(hover.contains("| Members | 0 |"), "{hover}");
    assert!(
        hover.contains("| Parent class | Base.lvclass (`../Base.lvclass`) |"),
        "{hover}"
    );
    let hover = fx.engine.hover(&at("Base.lvclass")).unwrap();
    assert!(hover.contains("| Members | 2 |"), "{hover}");
    assert!(
        hover.contains("| Parent class | none recorded |"),
        "{hover}"
    );
    assert!(
        hover.contains("| Child classes in workspace | 1 |"),
        "{hover}"
    );
    let hover = fx.engine.hover(&at("App.lvproj")).unwrap();
    assert!(hover.contains("| Kind | Project |"), "{hover}");
    assert!(hover.contains("| Members | 3 |"), "{hover}");
    assert!(hover.contains("| Dependencies | 1 |"), "{hover}");
    assert!(hover.contains("| Members in workspace | 2 |"), "{hover}");
    assert!(!hover.contains("Parent class"), "{hover}");

    // The watcher reports all three extensions, in any letter case.
    for name in ["a.lvproj", "a.lvlib", "a.LVCLASS", "a.vi", "a.ctl"] {
        assert!(Engine::is_relevant(Path::new(name)), "{name}");
    }
    assert!(!Engine::is_relevant(Path::new("a.xml")));

    // Saving the project without the missing member clears the finding.
    fs::write(
        at("App.lvproj"),
        project_xml(&["Base.lvclass", "Child.lvclass"]),
    )
    .unwrap();
    assert_eq!(fx.touch("App.lvproj"), [("App.lvproj".to_owned(), vec![])]);
    assert_eq!(fx.touch("App.lvproj"), []);

    // Deleting a member file is reported on the class that lists it.
    fs::remove_file(at("Stop.vi")).unwrap();
    assert_eq!(
        fx.touch("Stop.vi"),
        [("Base.lvclass".to_owned(), vec!["BZ006"])]
    );
    assert_eq!(names(fx.engine.members(&at("Base.lvclass"))), ["Run.vi"]);
    fs::write(at("Base.lvclass"), class_xml(&["Run.vi"], None)).unwrap();
    assert_eq!(
        fx.touch("Base.lvclass"),
        [("Base.lvclass".to_owned(), vec![])]
    );

    // Re-parenting a class is picked up from the re-parsed file.
    fs::write(at("Child.lvclass"), class_xml(&[], None)).unwrap();
    assert_eq!(fx.touch("Child.lvclass"), []);
    assert_eq!(fx.engine.parent_class(&at("Child.lvclass")), None);
    assert!(fx.engine.child_classes(&at("Base.lvclass")).is_empty());

    // A file that stops being valid XML is reported and stays a symbol.
    fs::write(at("App.lvproj"), "<Project><Item").unwrap();
    assert_eq!(
        fx.touch("App.lvproj"),
        [("App.lvproj".to_owned(), vec!["BZ000"])]
    );
    assert!(fx.engine.project(&at("App.lvproj")).is_none());
    assert!(fx.engine.members(&at("App.lvproj")).is_empty());
    let hover = fx.engine.hover(&at("App.lvproj")).unwrap();
    assert!(
        hover.contains("Cannot analyze file: invalid XML"),
        "{hover}"
    );
    assert_eq!(fx.engine.symbols("app", 10)[0].kind, SymbolKind::Project);

    // Deleting it removes the diagnostics and the symbol.
    fs::remove_file(at("App.lvproj")).unwrap();
    assert_eq!(fx.touch("App.lvproj"), [("App.lvproj".to_owned(), vec![])]);
    assert!(fx.engine.symbols("app", 10).is_empty());
    assert!(fx.engine.hover(&at("App.lvproj")).is_none());
}

/// A snapshot is what requests read while the next state is computed: it
/// must not change under them.
#[test]
fn snapshots_are_not_affected_by_later_changes() {
    let mut f = Fixture::new("snapshot");
    f.vi("a.vi", &CLEAN, &["b.vi"]);
    f.vi("b.vi", &INLINE_CODE, &[]);
    f.engine.scan();
    let before = f.engine.snapshot();
    let a = f.dir.join("a.vi");
    let b = f.dir.join("b.vi");

    fs::remove_file(&b).unwrap();
    f.touch("b.vi");
    assert!(before.contains(&b));
    assert_eq!(before.callees(&a), std::slice::from_ref(&b));
    assert!(before.hover(&b).is_some());
    let after = f.engine.snapshot();
    assert!(!after.contains(&b));
    assert!(after.callees(&a).is_empty());
    assert_eq!(after.paths().collect::<Vec<_>>(), [a.as_path()]);
}

#[test]
fn lists_files_below_a_directory() {
    let mut f = Fixture::new("under");
    for name in ["sub", "sub/deep", "sub2", "sub.d"] {
        fs::create_dir_all(f.dir.join(name)).unwrap();
    }
    for name in [
        "a.vi",
        "sub.vi",
        "sub/b.vi",
        "sub/deep/c.vi",
        "sub2/d.vi",
        "sub.d/e.vi",
        "t.vi",
    ] {
        f.vi(name, &CLEAN, &[]);
    }
    f.engine.scan();
    let snapshot = f.engine.snapshot();
    let below = |name: &str| -> Vec<PathBuf> { snapshot.paths_under(&f.dir.join(name)) };
    assert_eq!(
        below("sub"),
        [f.dir.join("sub/b.vi"), f.dir.join("sub/deep/c.vi")]
    );
    assert_eq!(below("sub/deep/../deep"), [f.dir.join("sub/deep/c.vi")]);
    assert_eq!(below("sub/b.vi"), Vec::<PathBuf>::new());
    assert_eq!(below("missing"), Vec::<PathBuf>::new());
    assert_eq!(snapshot.paths_under(&f.dir).len(), 7);
}

/// The state machine rules are computed when a file is loaded, so they
/// are published like every other diagnostic. The file is a real one;
/// `crates/buzzard-lint/tests/fixtures/verify/README.md` records its source.
#[test]
fn publishes_state_machine_findings() {
    let mut fx = Fixture::new("state-machines");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../buzzard-lint/tests/fixtures/verify/jki-state-machine");
    fs::copy(
        source.join("new-support-vis-examples.vi"),
        fx.dir.join("examples.vi"),
    )
    .unwrap();
    fs::copy(
        source.join("queued-state-machine-template.vi"),
        fx.dir.join("template.vi"),
    )
    .unwrap();

    let updates = fx.engine.scan();
    let found: Vec<(PathBuf, Vec<(&str, String)>)> = updates
        .into_iter()
        .map(|(path, diagnostics)| {
            let machine = diagnostics
                .iter()
                .filter(|d| d.code.starts_with("BZ2"))
                .map(|d| (d.code, d.message.clone()))
                .collect();
            (path.strip_prefix(&fx.dir).unwrap().to_path_buf(), machine)
        })
        .filter(|(_, machine): &(PathBuf, Vec<(&str, String)>)| !machine.is_empty())
        .collect();
    let [(path, findings)] = &found[..] else {
        panic!("{found:?}");
    };
    assert_eq!(path, Path::new("examples.vi"));
    assert_eq!(findings.len(), 3);
    assert!(findings.iter().all(|(code, _)| *code == "BZ203"));
    assert!(findings[1].1.starts_with(
        "string/queue state machine at while loop #70, case structure #177: state \
         `Action: Step 1` is enqueued but no case handles it"
    ));
    assert!(findings[1].1.ends_with("(heuristic, not proven)"));

    // Removing the file withdraws its findings.
    fs::remove_file(fx.dir.join("examples.vi")).unwrap();
    let updates = fx.touch("examples.vi");
    assert_eq!(updates, [("examples.vi".to_owned(), vec![])]);
}
