//! Incremental behaviour of the analysis engine, without the protocol layer.

use std::fs;
use std::path::PathBuf;

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
