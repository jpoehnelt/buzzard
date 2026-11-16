use std::fs;

use buzzard_rsrc::builder::RsrcBuilder;

use super::*;
use crate::inspector::Inspection;
use crate::project::{encode_parent_link, ProjectFile};

const V23: u32 = 0x2300_8000;
/// A directory that does not exist, so nothing in it is on disk.
const DIR: &str = "no-such-directory-for-the-deps-tests";

fn parts(path: &str) -> Vec<String> {
    path.split('/').map(str::to_owned).collect()
}

fn symbolic(path: &str) -> LvPath {
    LvPath {
        kind: PathKind::Absolute,
        components: parts(path),
    }
}

fn relative(path: &str) -> LvPath {
    LvPath {
        kind: PathKind::Relative,
        components: parts(path),
    }
}

fn vi(links: &[LvPath]) -> FileState {
    let named: Vec<(&str, LvPath)> = links
        .iter()
        .map(|link| (link.file_name().unwrap_or("x.vi"), link.clone()))
        .collect();
    let bytes = RsrcBuilder::default()
        .save_record(V23, &[0, 0x400, 0, 0, 0, 0])
        .links(&named)
        .build();
    FileState::from_inspection(Inspection::from_bytes(&bytes))
}

fn project(path: &str, xml: &str) -> FileState {
    FileState::Project(Box::new(
        ProjectFile::parse(Path::new(path), xml).expect("the test XML parses"),
    ))
}

fn workspace(files: Vec<(String, FileState)>) -> Workspace {
    let mut workspace = Workspace::default();
    for (path, state) in files {
        workspace.insert(Path::new(&path), state);
    }
    workspace
}

fn report(files: Vec<(String, FileState)>, roots: &Roots) -> DepsReport {
    let workspace = workspace(files);
    scan(&workspace, &DependencyGraph::build(&workspace), roots)
}

/// The path of a workspace file as the report prints it.
fn at(name: &str) -> String {
    shown(&format!("{DIR}/{name}"))
}

/// A path of the file system with the separators of this platform.
fn shown(path: &str) -> String {
    text_of(&normalize(Path::new(path)))
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("buzzard-deps-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the temp dir");
        Self(dir)
    }

    fn file(&self, rel: &str) {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().expect("a file has a parent")).expect("create parents");
        fs::write(path, b"x").expect("write the file");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_package_is_the_first_two_directories() {
    let package = |path: &str| package(&parts(path));
    // One directory, two directories, and more than two.
    assert_eq!(package("numeric/Random Number (Range).vi"), "numeric");
    assert_eq!(package("JKI/VIPM API/Build_vipm_api.vi"), "JKI/VIPM API");
    assert_eq!(
        package("gRPC/LabVIEW gRPC Library/Server API/Server/Start.vi"),
        "gRPC/LabVIEW gRPC Library"
    );
    assert_eq!(
        package("_OpenG.lib/error/error.llb/x.vi"),
        "_OpenG.lib/error"
    );
    // A file beside a directory is in the package above it.
    assert_eq!(
        package("LabVIEW Icon API/Get VI Icon.vi"),
        "LabVIEW Icon API"
    );
    assert_eq!(
        package("LabVIEW Icon API/lv_icon/Classes/x.vi"),
        "LabVIEW Icon API/lv_icon"
    );
}

#[test]
fn addons_are_one_directory_deeper() {
    let package = |path: &str| package(&parts(path));
    assert_eq!(
        package("addons/_JKI Toolkits/State Machine/_support/Parse State Queue.vi"),
        "addons/_JKI Toolkits/State Machine"
    );
    assert_eq!(
        package("Addons/_JKI Toolkits/Caraya/Caraya.lvlib"),
        "Addons/_JKI Toolkits/Caraya"
    );
    assert_eq!(package("addons/vendor/x.vi"), "addons/vendor");
    // Only as the first directory.
    assert_eq!(package("vendor/addons/product/x.vi"), "vendor/addons");
}

#[test]
fn a_library_ends_the_package_name() {
    let package = |path: &str| package(&parts(path));
    assert_eq!(
        package("Utility/error.llb/Clear Errors.vi"),
        "Utility/error.llb"
    );
    assert_eq!(package("dlg_ctls.llb/x.ctl"), "dlg_ctls.llb");
    assert_eq!(package("Utility/ERROR.LLB/x.vi"), "Utility/ERROR.LLB");
    assert_eq!(package("Vendor.lvlibp/sub/dir/x.vi"), "Vendor.lvlibp");
    assert_eq!(package("Motor.lvclass/sub/x.vi"), "Motor.lvclass");
    assert_eq!(package("Tools.lvlib/sub/x.vi"), "Tools.lvlib");
    // A link to the library file itself is in the same package as a link
    // to something inside it.
    assert_eq!(package("Utility/error.llb"), "Utility/error.llb");
    assert_eq!(package("Vendor.lvlibp"), "Vendor.lvlibp");
    // A library below the depth of a package does not change it.
    assert_eq!(
        package("express/express input/Block.llb/x.vi"),
        "express/express input"
    );
    assert_eq!(
        package("express/express input/Block.llb"),
        "express/express input"
    );
    // A class or library file is a file of its directory.
    assert_eq!(
        package("ChannelInstances/Messenger-bool.lvlib"),
        "ChannelInstances"
    );
    assert_eq!(package("Base/Base.lvclass"), "Base");
    // The private data control inside a class file is too.
    assert_eq!(package("Base/Base.lvclass/Base.ctl"), "Base/Base.lvclass");
}

#[test]
fn a_file_directly_under_the_root_is_its_own_package() {
    assert_eq!(package(&parts("lvanlys.dll")), "lvanlys.dll");
    assert_eq!(package(&parts("Top.lvlib")), "Top.lvlib");
    assert_eq!(package(&[]), ".");
    // A parent step is shown the way stored paths show it.
    assert_eq!(package(&parts("/x.vi")), "..");
}

#[test]
fn targets_are_grouped_by_root_then_package_with_each_referrer_once() {
    let clear = symbolic("<vilib>/Utility/error.llb/Clear Errors.vi");
    let found = report(
        vec![
            (
                at("b.vi"),
                vi(&[
                    clear.clone(),
                    // The same target twice in one file is one referrer.
                    clear.clone(),
                    symbolic("<vilib>/Utility/error.llb/Error Cluster.ctl"),
                    symbolic("<userlib>/_OpenG.lib/error/error.llb/Build Error__ogtk.vi"),
                ]),
            ),
            (
                at("a.vi"),
                vi(&[
                    clear,
                    symbolic("<vilib>/addons/_JKI Toolkits/State Machine/x.vi"),
                    // Root names are matched without regard to case.
                    symbolic("<VILIB>/Utility/file.llb/y.vi"),
                    symbolic("<resource>/lvanlys.dll"),
                ]),
            ),
        ],
        &Roots::default(),
    );
    // Root, package, target.
    let mut shape: Vec<(&str, &str, &str)> = Vec::new();
    for root in &found.roots {
        for package in &root.packages {
            for target in &package.targets {
                shape.push((&root.root, &package.name, &target.path));
            }
        }
    }
    assert_eq!(
        shape,
        [
            ("resource", "lvanlys.dll", "<resource>/lvanlys.dll"),
            (
                "userlib",
                "_OpenG.lib/error",
                "<userlib>/_OpenG.lib/error/error.llb/Build Error__ogtk.vi"
            ),
            (
                "vilib",
                "Utility/error.llb",
                "<vilib>/Utility/error.llb/Clear Errors.vi"
            ),
            (
                "vilib",
                "Utility/error.llb",
                "<vilib>/Utility/error.llb/Error Cluster.ctl"
            ),
            ("vilib", "Utility/file.llb", "<vilib>/Utility/file.llb/y.vi"),
            (
                "vilib",
                "addons/_JKI Toolkits/State Machine",
                "<vilib>/addons/_JKI Toolkits/State Machine/x.vi"
            ),
        ]
    );
    let clear = &found.roots[2].packages[0].targets[0];
    assert_eq!(clear.referrers, [at("a.vi"), at("b.vi")]);
    assert_eq!(clear.status, Status::Unchecked);
    assert!(found.roots.iter().all(|root| root.directory.is_none()));
    assert_eq!(found.files, 2);
    assert_eq!(
        (&found.absolute, &found.missing),
        (&Vec::new(), &Vec::new())
    );
    assert_eq!(found.unparsed, []);
    let totals = found.totals();
    assert_eq!(
        (totals.packages, totals.targets, totals.unchecked),
        (5, 6, 6)
    );
    assert_eq!((totals.found, totals.missing), (0, 0));
}

#[test]
fn absolute_and_missing_links_are_listed_apart() {
    let found = report(
        vec![
            (
                at("main.vi"),
                vi(&[
                    LvPath::absolute(&["C", "Users", "me", "far.vi"]),
                    LvPath {
                        kind: PathKind::Unc,
                        components: parts("server/share/net.vi"),
                    },
                    LvPath::sibling("present.vi"),
                    LvPath::sibling("gone.vi"),
                    relative("//lib/also gone.vi"),
                    // A stale location still finds the one file of that name.
                    relative("/moved/away/unique.vi"),
                    // A bare name is not a location.
                    relative("user32.dll"),
                    LvPath {
                        kind: PathKind::NotAPath,
                        components: Vec::new(),
                    },
                ]),
            ),
            (at("other.vi"), vi(&[LvPath::sibling("gone.vi")])),
            (at("present.vi"), vi(&[])),
            (at("sub/unique.vi"), vi(&[])),
        ],
        &Roots::default(),
    );
    assert_eq!(found.roots, []);
    let pairs = |links: &[Link]| -> Vec<(String, Vec<String>)> {
        links
            .iter()
            .map(|l| (l.target.clone(), l.referrers.clone()))
            .collect()
    };
    assert_eq!(
        pairs(&found.absolute),
        [
            ("//server/share/net.vi".to_owned(), vec![at("main.vi")]),
            ("C:/Users/me/far.vi".to_owned(), vec![at("main.vi")]),
        ]
    );
    assert_eq!(
        pairs(&found.missing),
        [
            (shown("lib/also gone.vi"), vec![at("main.vi")]),
            (at("gone.vi"), vec![at("main.vi"), at("other.vi")]),
        ]
    );
}

#[test]
fn project_files_contribute_member_urls_and_parent_links() {
    let parent = |link: &LvPath| {
        let name = link.file_name().expect("a parent link names a file");
        let text = encode_parent_link(V23, &[name], link)
            .replace('&', "&amp;")
            .replace('<', "&lt;");
        format!("<Property Name=\"NI.LVClass.ParentClassLinkInfo\" Type=\"Bin\">{text}</Property>")
    };
    let class =
        |link: &LvPath| format!("<LVClass LVVersion=\"23008000\">{}</LVClass>", parent(link));
    let lvproj = r#"<Project Type="Project" LVVersion="23008000">
        <Item Name="My Computer" Type="My Computer">
            <Item Name="present.vi" Type="VI" URL="../present.vi"/>
            <Item Name="gone.vi" Type="VI" URL="../gone.vi"/>
            <Item Name="pinned.vi" Type="VI" URL="/C/work/pinned.vi"/>
            <Item Name="Inner.ctl" Type="Class Private Data" URL="Inner.ctl"/>
            <Item Name="Dependencies" Type="Dependencies">
                <Item Name="vi.lib" Type="Folder">
                    <Item Name="Clear Errors.vi" Type="VI" URL="/&lt;vilib&gt;/Utility/error.llb/Clear Errors.vi"/>
                </Item>
                <Item Name="stale.vi" Type="VI" URL="../stale.vi"/>
            </Item>
        </Item>
    </Project>"#;
    let found = report(
        vec![
            (at("App.lvproj"), project(&at("App.lvproj"), lvproj)),
            (
                at("present.vi"),
                vi(&[symbolic("<vilib>/Utility/error.llb/Clear Errors.vi")]),
            ),
            (
                at("Installed.lvclass"),
                project(
                    &at("Installed.lvclass"),
                    &class(&symbolic("<vilib>/Vendor/Base/Base.lvclass")),
                ),
            ),
            (
                at("Orphan.lvclass"),
                project(
                    &at("Orphan.lvclass"),
                    &class(&LvPath::sibling("Lost.lvclass")),
                ),
            ),
            (
                at("Child.lvclass"),
                project(
                    &at("Child.lvclass"),
                    &class(&LvPath::sibling("Orphan.lvclass")),
                ),
            ),
            (
                at("Pinned.lvclass"),
                project(
                    &at("Pinned.lvclass"),
                    &class(&LvPath::absolute(&["D", "classes", "Far.lvclass"])),
                ),
            ),
        ],
        &Roots::default(),
    );
    let targets: Vec<(&str, &[String])> = found.roots[0]
        .packages
        .iter()
        .flat_map(|p| &p.targets)
        .map(|t| (t.path.as_str(), t.referrers.as_slice()))
        .collect();
    assert_eq!(
        targets,
        [
            (
                "<vilib>/Utility/error.llb/Clear Errors.vi",
                // The member URL and the link of the VI name one target.
                &[at("App.lvproj"), at("present.vi")][..]
            ),
            (
                "<vilib>/Vendor/Base/Base.lvclass",
                &[at("Installed.lvclass")][..]
            ),
        ]
    );
    let absolute: Vec<&str> = found.absolute.iter().map(|l| l.target.as_str()).collect();
    assert_eq!(absolute, ["/C/work/pinned.vi", "D:/classes/Far.lvclass"]);
    // `stale.vi` is under `Dependencies`, which rule BZ006 skips too, and
    // `Inner.ctl` is inside the project file.
    let missing: Vec<(&str, &[String])> = found
        .missing
        .iter()
        .map(|l| (l.target.as_str(), l.referrers.as_slice()))
        .collect();
    assert_eq!(
        missing,
        [
            (at("Lost.lvclass").as_str(), &[at("Orphan.lvclass")][..]),
            (at("gone.vi").as_str(), &[at("App.lvproj")][..]),
        ]
    );
}

#[test]
fn a_missing_member_is_exactly_what_bz006_reports() {
    let lvproj = r#"<Project Type="Project" LVVersion="23008000">
        <Item Name="a.vi" Type="VI" URL="../a.vi"/>
        <Item Name="gone.vi" Type="VI" URL="../gone.vi"/>
        <Item Name="lost.ctl" Type="VI" URL="../sub/lost.ctl"/>
    </Project>"#;
    let files = vec![
        (at("App.lvproj"), project(&at("App.lvproj"), lvproj)),
        (at("a.vi"), vi(&[])),
    ];
    let workspace = workspace(files);
    let graph = DependencyGraph::build(&workspace);
    let diagnostics = crate::lint(&workspace, &graph, &crate::LintConfig::default());
    let flagged = diagnostics.iter().filter(|d| d.code == "BZ006").count();
    let found = scan(&workspace, &graph, &Roots::default());
    assert_eq!(flagged, 2);
    assert_eq!(found.missing.len(), flagged);
}

#[test]
fn unparsed_files_are_listed_with_their_reason() {
    let found = report(
        vec![
            (
                at("bad.vi"),
                FileState::Failed("not an RSRC file".to_owned()),
            ),
            (at("good.vi"), vi(&[])),
        ],
        &Roots::default(),
    );
    assert_eq!(
        found.unparsed,
        [Unparsed {
            path: at("bad.vi"),
            message: "not an RSRC file".to_owned()
        }]
    );
    assert_eq!(found.files, 2);
}

#[test]
fn a_root_with_a_directory_is_looked_up_on_disk() {
    let dir = TempDir::new("status");
    dir.file("vi.lib/Utility/error.llb");
    dir.file("vi.lib/Utility/High Resolution Relative Seconds.vi");
    dir.file("vi.lib/Base/Base.lvclass");
    let mut roots = Roots::default();
    roots.add("<VILIB>", &dir.0.join("vi.lib")).unwrap();
    let found = report(
        vec![(
            at("main.vi"),
            vi(&[
                symbolic("<vilib>/Utility/error.llb/Clear Errors.vi"),
                symbolic("<vilib>/Utility/error.llb"),
                symbolic("<vilib>/Utility/file.llb/Gone.vi"),
                symbolic("<vilib>/Utility/High Resolution Relative Seconds.vi"),
                symbolic("<vilib>/Utility/Gone.vi"),
                symbolic("<vilib>/Base/Base.lvclass/Base.ctl"),
                // Parent steps and separators would leave the directory.
                symbolic("<vilib>//Utility/error.llb"),
                symbolic("<vilib>/../vi.lib/Utility/error.llb"),
                symbolic("<userlib>/x/y.vi"),
            ]),
        )],
        &roots,
    );
    let statuses: Vec<(&str, Status)> = found
        .roots
        .iter()
        .flat_map(|root| &root.packages)
        .flat_map(|package| &package.targets)
        .map(|target| (target.path.as_str(), target.status))
        .collect();
    assert_eq!(
        statuses,
        [
            ("<userlib>/x/y.vi", Status::Unchecked),
            ("<vilib>/../Utility/error.llb", Status::Unchecked),
            ("<vilib>/../vi.lib/Utility/error.llb", Status::Unchecked),
            ("<vilib>/Base/Base.lvclass/Base.ctl", Status::Found),
            ("<vilib>/Utility/Gone.vi", Status::Missing),
            (
                "<vilib>/Utility/High Resolution Relative Seconds.vi",
                Status::Found
            ),
            ("<vilib>/Utility/error.llb", Status::Found),
            ("<vilib>/Utility/error.llb/Clear Errors.vi", Status::Found),
            ("<vilib>/Utility/file.llb/Gone.vi", Status::Missing),
        ]
    );
    assert_eq!(found.roots[0].directory, None);
    assert_eq!(
        found.roots[1].directory.as_deref(),
        Some(text_of(&dir.0.join("vi.lib")).as_str())
    );
    let totals = found.totals();
    assert_eq!((totals.found, totals.missing, totals.unchecked), (4, 2, 3));
}

#[test]
fn root_specifications_are_checked() {
    let dir = TempDir::new("roots");
    assert_eq!(
        Roots::split("vilib=/opt/vi.lib"),
        Ok(("vilib", "/opt/vi.lib"))
    );
    // Only the first `=` separates; a directory may hold one.
    assert_eq!(Roots::split("vilib=/a=b"), Ok(("vilib", "/a=b")));
    for bad in ["vilib", "=dir", "vilib=", "", " =dir"] {
        assert!(Roots::split(bad).is_err(), "{bad:?}");
    }
    let mut roots = Roots::default();
    for bad in ["", "<>", "vi lib", "a/b", "<vilib", "a=b"] {
        assert!(roots.add(bad, &dir.0).is_err(), "{bad:?}");
    }
    let missing = roots.add("vilib", &dir.0.join("nowhere")).unwrap_err();
    assert!(missing.contains("does not exist"), "{missing}");
    roots.add("vilib", &dir.0).unwrap();
    // The same directory again is fine, under any spelling of the name.
    roots.add("<ViLib>", &dir.0).unwrap();
    fs::create_dir(dir.0.join("other")).unwrap();
    let twice = roots.add("VILIB", &dir.0.join("other")).unwrap_err();
    assert!(twice.contains("two directories"), "{twice}");
    assert_eq!(roots.get("vilib"), Some(dir.0.as_path()));
    assert_eq!(roots.get("userlib"), None);
}

fn sample() -> DepsReport {
    let mut found = report(
        vec![
            (
                at("main.vi"),
                vi(&[
                    symbolic("<vilib>/Utility/error.llb/Clear Errors.vi"),
                    symbolic("<vilib>/Utility/odd|na`me\n.vi"),
                    LvPath::absolute(&["C", "far.vi"]),
                    LvPath::sibling("gone.vi"),
                ]),
            ),
            (at("bad.vi"), FileState::Failed("too\nshort".to_owned())),
        ],
        &Roots::default(),
    );
    found.map_paths(|path| path.replace(DIR, "ws").replace('\\', "/"));
    found
}

#[test]
fn text_report_lists_every_section_and_a_summary() {
    assert_eq!(
        to_text(&sample()),
        "<vilib> (not checked: no directory was given for root vilib)
  Utility (1 file)
    unchecked <vilib>/Utility/odd|na`me .vi
      used by ws/main.vi
  Utility/error.llb (1 file)
    unchecked <vilib>/Utility/error.llb/Clear Errors.vi
      used by ws/main.vi

Machine-absolute links (1)
  C:/far.vi
    used by ws/main.vi

Missing relative links (1)
  ws/gone.vi
    used by ws/main.vi

Unparsed files (1)
  ws/bad.vi: cannot analyze file: too short

Scanned 2 files: 2 external files in 2 packages under 1 symbolic root (0 found, 0 missing, 2 \
         unchecked), 1 machine-absolute link, 1 missing relative link, 1 unparsed file.
"
    );
    let empty = report(Vec::new(), &Roots::default());
    assert_eq!(
        to_text(&empty),
        "Scanned 0 files: 0 external files in 0 packages under 0 symbolic roots (0 found, 0 \
         missing, 0 unchecked), 0 machine-absolute links, 0 missing relative links, 0 unparsed \
         files.\n"
    );
}

#[test]
fn markdown_report_keeps_file_text_inside_its_cells() {
    let markdown = to_markdown(&sample());
    assert!(markdown.starts_with("# External dependencies\n\nScanned 2 files"));
    assert!(markdown.contains("\n## `<vilib>`\n\nNot checked: no directory was given"));
    assert!(markdown.contains(
        "| `Utility/error.llb` | `<vilib>/Utility/error.llb/Clear Errors.vi` | unchecked | `ws/main.vi` |"
    ));
    assert!(markdown.contains("| `Utility` | `<vilib>/Utility/odd na me .vi` | unchecked |"));
    assert!(markdown.contains("\n## Machine-absolute links\n\n| Stored path | Used by |"));
    assert!(markdown.contains("| `ws/gone.vi` | `ws/main.vi` |"));
    assert!(markdown.contains("| `ws/bad.vi` | too short |"));
    // Every table row has the cells of its header.
    for line in markdown.lines().filter(|line| line.starts_with('|')) {
        assert!(matches!(line.matches('|').count(), 3 | 5), "{line}");
    }
}

#[test]
fn json_has_the_documented_fields_and_leaves_out_an_absent_directory() {
    let value = serde_json::to_value(sample()).unwrap();
    let keys = |value: &serde_json::Value| -> Vec<String> {
        value.as_object().unwrap().keys().cloned().collect()
    };
    assert_eq!(
        keys(&value),
        ["absolute", "files", "missing", "roots", "unparsed"]
    );
    assert_eq!(keys(&value["roots"][0]), ["packages", "root"]);
    assert_eq!(keys(&value["roots"][0]["packages"][0]), ["name", "targets"]);
    let target = &value["roots"][0]["packages"][1]["targets"][0];
    assert_eq!(keys(target), ["path", "referrers", "status"]);
    assert_eq!(target["status"], "unchecked");
    assert_eq!(keys(&value["absolute"][0]), ["referrers", "target"]);
    assert_eq!(keys(&value["missing"][0]), ["referrers", "target"]);
    assert_eq!(keys(&value["unparsed"][0]), ["message", "path"]);
    for (status, word) in [
        (Status::Found, "found"),
        (Status::Missing, "missing"),
        (Status::Unchecked, "unchecked"),
    ] {
        assert_eq!(serde_json::to_value(status).unwrap(), word);
        assert_eq!(status.as_str(), word);
    }
}

#[test]
fn mapping_paths_keeps_stored_paths_and_sorts_again() {
    let mut found = report(
        vec![
            (
                at("a.vi"),
                vi(&[symbolic("<vilib>/x/y.vi"), LvPath::sibling("gone.vi")]),
            ),
            (at("b.vi"), vi(&[symbolic("<vilib>/x/y.vi")])),
            (at("c.vi"), FileState::Failed("bad".to_owned())),
            (at("d.vi"), FileState::Failed("bad".to_owned())),
        ],
        &Roots::default(),
    );
    // Reverse the order of the file names.
    found.map_paths(|path| {
        let name = Path::new(path).file_name().unwrap().to_string_lossy();
        format!("{}-{name}", 'z' as u32 - u32::from(name.as_bytes()[0]))
    });
    let target = &found.roots[0].packages[0].targets[0];
    assert_eq!(target.path, "<vilib>/x/y.vi");
    assert_eq!(target.referrers, ["24-b.vi", "25-a.vi"]);
    assert_eq!(found.missing[0].target, "19-gone.vi");
    assert_eq!(found.unparsed[0].path, "22-d.vi");
    assert_eq!(found.unparsed[1].path, "23-c.vi");
}
