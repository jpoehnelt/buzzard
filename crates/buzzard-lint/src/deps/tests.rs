use std::fs;

use buzzard_rsrc::builder::{LlbBuilder, RsrcBuilder};

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

    /// An `.llb` archive that holds a VI under each of the names.
    fn archive(&self, rel: &str, members: &[&[u8]]) {
        let vi = RsrcBuilder::default()
            .save_record(V23, &[0, 0x400, 0, 0, 0, 0])
            .build();
        let mut builder = LlbBuilder::default();
        for name in members {
            builder = builder.member_bytes(name, &vi);
        }
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().expect("a file has a parent")).expect("create parents");
        fs::write(path, builder.build()).expect("write the archive");
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
}

#[test]
fn the_package_name_stops_in_front_of_a_component_that_is_not_a_name() {
    // The bare root, and paths that start with a parent step.
    assert_eq!(package(&[]), IRREGULAR);
    assert_eq!(package(&parts("/x.vi")), IRREGULAR);
    assert_eq!(package(&parts("../x.vi")), IRREGULAR);
    assert_eq!(package(&parts("./x.vi")), IRREGULAR);
    assert_eq!(package(&parts("")), IRREGULAR);
    assert_eq!(package(&["/etc/passwd".to_owned()]), IRREGULAR);
    assert_eq!(package(&["/etc".to_owned(), "x".to_owned()]), IRREGULAR);
    // A step further down ends the name where it is.
    assert_eq!(package(&parts("Utility//q.vi")), "Utility");
    assert_eq!(package(&parts("Utility/../Utility/q.vi")), "Utility");
    assert_eq!(
        package(&parts("addons/Vendor//Product/q.vi")),
        "addons/Vendor"
    );
    assert_eq!(package(&parts("addons//x.llb")), "addons");
    assert_eq!(package(&["a\\b".to_owned(), "q.vi".to_owned()]), IRREGULAR);
}

/// Packages by name, each with its targets and their referrers.
type Listed<'a> = Vec<(&'a str, Vec<(&'a str, &'a [String])>)>;

#[test]
fn spellings_of_one_file_are_one_target_in_one_package() {
    let found = report(
        vec![
            (
                at("a.vi"),
                vi(&[
                    symbolic("<vilib>/Platform/TimedLoop/x.vi"),
                    symbolic("<vilib>/platform/TimedLoop/y.vi"),
                    symbolic("<vilib>/Utility/_JKI_lib.llb/Add.vi"),
                ]),
            ),
            (
                at("b.vi"),
                vi(&[
                    symbolic("<VILIB>/platform/timedloop/X.vi"),
                    symbolic("<vilib>/Utility/_jki_lib.llb/Add.vi"),
                    // Two stored paths that print the same.
                    symbolic("<vilib>//secret.vi"),
                    symbolic("<vilib>/../secret.vi"),
                ]),
            ),
        ],
        &Roots::default(),
    );
    assert_eq!(found.roots.len(), 1);
    let listed: Listed<'_> = found.roots[0]
        .packages
        .iter()
        .map(|package| {
            let targets = package
                .targets
                .iter()
                .map(|t| (t.path.as_str(), t.referrers.as_slice()))
                .collect();
            (package.name.as_str(), targets)
        })
        .collect();
    let (a, b) = (at("a.vi"), at("b.vi"));
    assert_eq!(
        listed,
        [
            (IRREGULAR, vec![("<vilib>/../secret.vi", &[b.clone()][..])]),
            (
                // Of the spellings, the one that sorts first is printed.
                "Platform/TimedLoop",
                vec![
                    (
                        "<vilib>/Platform/TimedLoop/x.vi",
                        &[a.clone(), b.clone()][..]
                    ),
                    ("<vilib>/platform/TimedLoop/y.vi", &[a.clone()][..]),
                ]
            ),
            (
                "Utility/_JKI_lib.llb",
                vec![("<vilib>/Utility/_JKI_lib.llb/Add.vi", &[a, b][..])]
            ),
        ]
    );
    assert_eq!(found.totals().targets, 4);
}

#[test]
fn a_location_is_one_link_whether_a_vi_or_a_project_file_stores_it() {
    let lvlib = r#"<Library LVVersion="23008000">
        <Item Name="x.vi" Type="VI" URL="/C/Users/me/x.vi"/>
        <Item Name="y.vi" Type="VI" URL="/home/me/y.vi"/>
        <Item Name="z.vi" Type="VI" URL="/&lt;vilib&gt;/Utility/z.vi"/>
        <Item Name="root" Type="VI" URL="/&lt;vilib&gt;"/>
    </Library>"#;
    let found = report(
        vec![
            (at("L.lvlib"), project(&at("L.lvlib"), lvlib)),
            (
                at("a.vi"),
                vi(&[
                    LvPath::absolute(&["C", "Users", "me", "x.vi"]),
                    LvPath::absolute(&["home", "me", "y.vi"]),
                    symbolic("<vilib>/Utility/z.vi"),
                    symbolic("<vilib>"),
                ]),
            ),
        ],
        &Roots::default(),
    );
    let both = [at("L.lvlib"), at("a.vi")];
    let absolute: Vec<(&str, &[String])> = found
        .absolute
        .iter()
        .map(|l| (l.target.as_str(), l.referrers.as_slice()))
        .collect();
    assert_eq!(
        absolute,
        [
            ("/home/me/y.vi", &both[..]),
            ("C:/Users/me/x.vi", &both[..])
        ]
    );
    let targets: Vec<(&str, &str, &[String])> = found.roots[0]
        .packages
        .iter()
        .flat_map(|p| p.targets.iter().map(|t| (p.name.as_str(), t)))
        .map(|(name, t)| (name, t.path.as_str(), t.referrers.as_slice()))
        .collect();
    assert_eq!(
        targets,
        [
            (IRREGULAR, "<vilib>", &both[..]),
            ("Utility", "<vilib>/Utility/z.vi", &both[..]),
        ]
    );
}

#[test]
fn member_urls_in_the_shapes_of_a_stored_path_are_listed() {
    // LabVIEW does not write these shapes. A hand-edited file may hold them.
    let lvproj = r#"<Project Type="Project" LVVersion="23008000">
        <Item Name="y.vi" Type="VI" URL="C:/Users/me/y.vi"/>
        <Item Name="z.vi" Type="VI" URL="C:\Users\me\z.vi"/>
        <Item Name="n.vi" Type="VI" URL="\\server\share\n.vi"/>
        <Item Name="r.vi" Type="VI" URL="&lt;vilib&gt;/Utility/Real.vi"/>
        <Item Name="odd.vi" Type="VI" URL="sub/&lt;odd&gt;.vi"/>
        <Item Name="back.vi" Type="VI" URL="sub\back.vi"/>
        <Item Name="scheme" Type="VI" URL="http://example.com/a.vi"/>
        <Item Name="inner" Type="VI" URL="inner.ctl"/>
    </Project>"#;
    let found = report(
        vec![(at("App.lvproj"), project(&at("App.lvproj"), lvproj))],
        &Roots::default(),
    );
    let absolute: Vec<&str> = found.absolute.iter().map(|l| l.target.as_str()).collect();
    assert_eq!(
        absolute,
        [
            "//server/share/n.vi",
            "C:/Users/me/y.vi",
            "C:/Users/me/z.vi"
        ]
    );
    assert_eq!(found.roots.len(), 1);
    assert_eq!(
        found.roots[0].packages[0].targets[0].path,
        "<vilib>/Utility/Real.vi"
    );
    assert_eq!(found.totals().targets, 1);
    // The others have no shape to file them under and are left out.
    assert_eq!(found.missing, []);
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
    assert_eq!(absolute, ["C:/work/pinned.vi", "D:/classes/Far.lvclass"]);
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
    dir.archive("vi.lib/Utility/error.llb", &[b"Clear Errors.vi"]);
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
    assert_eq!(found.unused_roots, Vec::<String>::new());
}

/// A target inside an `.llb` is found only if the archive on disk holds
/// that member. When the archive cannot be read, the status is unchecked.
#[test]
fn a_target_inside_an_archive_is_looked_up_in_the_archive() {
    let dir = TempDir::new("archive-status");
    // "テスト.vi" as a machine with the Shift-JIS code page stores it.
    let legacy: &[u8] = b"\x83\x65\x83\x58\x83\x67.vi";
    dir.archive(
        "vi.lib/Utility/error.llb",
        &[b"Clear Errors.vi", b"simple error handler.VI", legacy],
    );
    dir.file("vi.lib/Utility/broken.llb");
    let mut roots = Roots::default();
    roots.add("vilib", &dir.0.join("vi.lib")).unwrap();
    let stored: String = legacy.iter().map(|&b| char::from(b)).collect();
    let found = report(
        vec![(
            at("main.vi"),
            vi(&[
                symbolic("<vilib>/Utility/error.llb"),
                symbolic("<vilib>/Utility/error.llb/Clear Errors.vi"),
                symbolic("<vilib>/Utility/error.llb/Simple Error Handler.vi"),
                symbolic("<vilib>/Utility/error.llb/Gone.vi"),
                symbolic("<vilib>/Utility/error.llb/sub/Clear Errors.vi"),
                symbolic(&format!("<vilib>/Utility/error.llb/{stored}")),
                symbolic("<vilib>/Utility/error.llb/テスト.vi"),
                symbolic("<vilib>/utility/ERROR.LLB/clear errors.vi"),
                symbolic("<vilib>/Utility/broken.llb"),
                symbolic("<vilib>/Utility/broken.llb/Any.vi"),
                symbolic("<vilib>/Utility/absent.llb/Any.vi"),
            ]),
        )],
        &roots,
    );
    let found: Vec<(String, Status)> = statuses(&found)
        .into_iter()
        .map(|(path, status)| (path.replace(&stored, "(legacy)"), status))
        .collect();
    let found: Vec<(&str, Status)> = found.iter().map(|(p, s)| (p.as_str(), *s)).collect();
    assert_eq!(
        found,
        [
            ("<vilib>/Utility/absent.llb/Any.vi", Status::Missing),
            ("<vilib>/Utility/broken.llb", Status::Found),
            ("<vilib>/Utility/broken.llb/Any.vi", Status::Unchecked),
            ("<vilib>/Utility/error.llb", Status::Found),
            ("<vilib>/Utility/error.llb/Clear Errors.vi", Status::Found),
            ("<vilib>/Utility/error.llb/Gone.vi", Status::Missing),
            (
                "<vilib>/Utility/error.llb/Simple Error Handler.vi",
                Status::Found
            ),
            (
                "<vilib>/Utility/error.llb/sub/Clear Errors.vi",
                Status::Missing
            ),
            ("<vilib>/Utility/error.llb/(legacy)", Status::Found),
            ("<vilib>/Utility/error.llb/テスト.vi", Status::Found),
        ]
    );
}

fn statuses(found: &DepsReport) -> Vec<(&str, Status)> {
    found
        .roots
        .iter()
        .flat_map(|root| &root.packages)
        .flat_map(|package| &package.targets)
        .map(|target| (target.path.as_str(), target.status))
        .collect()
}

#[test]
fn a_file_under_another_letter_case_is_found() {
    let dir = TempDir::new("case");
    dir.file("vi.lib/Utility/Real.vi");
    dir.archive(
        "vi.lib/addons/_JKI Toolkits/_JKI_lib_State_Machine.llb",
        &[b"ADD.vi"],
    );
    dir.file("vi.lib/Platform/TimedLoop/a.vi");
    let mut roots = Roots::default();
    roots.add("vilib", &dir.0.join("vi.lib")).unwrap();
    let found = report(
        vec![(
            at("main.vi"),
            vi(&[
                symbolic("<vilib>/utility/REAL.vi"),
                symbolic("<vilib>/Addons/_JKI Toolkits/_jki_lib_state_machine.llb/Add.vi"),
                symbolic("<vilib>/platform/timedloop/a.vi"),
                symbolic("<vilib>/platform/timedloop/b.vi"),
                symbolic("<vilib>/utility/real.vi/inside.ctl"),
                symbolic("<vilib>/utility/other.vi"),
            ]),
        )],
        &roots,
    );
    assert_eq!(
        statuses(&found),
        [
            (
                "<vilib>/Addons/_JKI Toolkits/_jki_lib_state_machine.llb/Add.vi",
                Status::Found
            ),
            ("<vilib>/platform/timedloop/a.vi", Status::Found),
            ("<vilib>/platform/timedloop/b.vi", Status::Missing),
            ("<vilib>/utility/REAL.vi", Status::Found),
            ("<vilib>/utility/other.vi", Status::Missing),
            ("<vilib>/utility/real.vi/inside.ctl", Status::Found),
        ]
    );
}

/// `text` as a file saved it in `encoding` and a reader shows it: one
/// character per byte.
fn stored_in(encoding: &'static encoding_rs::Encoding, text: &str) -> String {
    let (bytes, _, lossy) = encoding.encode(text);
    assert!(!lossy, "{text} is not in the code page");
    bytes.iter().map(|&b| char::from(b)).collect()
}

#[test]
fn a_target_stored_in_a_legacy_code_page_is_found() {
    let dir = TempDir::new("code-page");
    dir.file("vi.lib/日本ysis/NI_AALBase.lvlib");
    dir.archive("vi.lib/日本ysis/1siggen.llb", &[b"Noise.vi"]);
    let mut roots = Roots::default();
    roots.add("vilib", &dir.0.join("vi.lib")).unwrap();
    let folder = stored_in(encoding_rs::SHIFT_JIS, "日本ysis");
    let found = report(
        vec![(
            at("main.vi"),
            vi(&[
                symbolic(&format!("<vilib>/{folder}/NI_AALBase.lvlib")),
                symbolic(&format!("<vilib>/{folder}/1siggen.llb/Noise.vi")),
                symbolic(&format!("<vilib>/{folder}/Gone.vi")),
            ]),
        )],
        &roots,
    );
    let found: Vec<(&str, Status)> = statuses(&found)
        .into_iter()
        .map(|(path, status)| (path.rsplit('/').next().unwrap_or(path), status))
        .collect();
    assert_eq!(
        found,
        [
            ("Gone.vi", Status::Missing),
            ("NI_AALBase.lvlib", Status::Found),
            ("Noise.vi", Status::Found),
        ]
    );
}

#[test]
fn a_relative_link_stored_in_a_legacy_code_page_is_not_missing() {
    let dir = TempDir::new("code-page-relative");
    dir.file("lib/日本.vi");
    let main = text_of(&dir.0.join("main.vi"));
    let found = report(
        vec![(
            main.clone(),
            vi(&[
                relative(&format!(
                    "/lib/{}",
                    stored_in(encoding_rs::SHIFT_JIS, "日本.vi")
                )),
                relative(&format!(
                    "/lib/{}",
                    stored_in(encoding_rs::SHIFT_JIS, "東京.vi")
                )),
            ]),
        )],
        &Roots::default(),
    );
    // The first is on disk under its Shift-JIS reading. The second is not
    // there under any reading.
    assert_eq!(found.missing.len(), 1, "{:?}", found.missing);
    assert_eq!(found.missing[0].referrers, [shown(&main)]);
}

#[test]
fn a_path_that_is_not_stored_as_absolute_is_not_looked_up() {
    let dir = TempDir::new("kinds");
    dir.file("vi.lib/abs.vi");
    dir.file("vi.lib/rel.vi");
    let mut roots = Roots::default();
    roots.add("vilib", &dir.0.join("vi.lib")).unwrap();
    let of_kind = |kind: PathKind, name: &str| LvPath {
        kind,
        components: vec!["<vilib>".to_owned(), name.to_owned()],
    };
    let found = report(
        vec![(
            at("main.vi"),
            vi(&[
                of_kind(PathKind::Absolute, "abs.vi"),
                // One target with the link above. The spelling that can be
                // looked up decides.
                of_kind(PathKind::Unc, "ABS.vi"),
                of_kind(PathKind::Relative, "rel.vi"),
                of_kind(PathKind::NotAPath, "nap.vi"),
                of_kind(PathKind::Unc, "unc.vi"),
            ]),
        )],
        &roots,
    );
    assert_eq!(
        statuses(&found),
        [
            ("<vilib>/ABS.vi", Status::Found),
            ("<vilib>/nap.vi", Status::Unchecked),
            ("<vilib>/rel.vi", Status::Unchecked),
            ("<vilib>/unc.vi", Status::Unchecked),
        ]
    );
}

#[test]
fn a_root_that_no_link_is_under_is_reported_as_unused() {
    let dir = TempDir::new("unused");
    let mut roots = Roots::default();
    for name in ["vilib", "vi.lib", "userlib"] {
        roots.add(name, &dir.0).unwrap();
    }
    let mut found = report(
        vec![(at("main.vi"), vi(&[symbolic("<vilib>/Utility/x.vi")]))],
        &roots,
    );
    assert_eq!(found.unused_roots, ["userlib", "vi.lib"]);
    found.map_paths(|_| "dir".to_owned());
    let text = to_text(&found);
    assert!(
        text.contains(
            "\nUnused roots (2)\n  userlib: no link is under <userlib>, so its directory was \
             not used\n  vi.lib: no link is under <vi.lib>, so its directory was not used\n\n\
             Scanned 1 file"
        ),
        "{text}"
    );
    assert!(overview_to_text(&found).contains("\nUnused roots (2)\n  userlib: "));
    for markdown in [to_markdown(&found), overview_to_markdown(&found)] {
        assert!(
            markdown.ends_with(
                "\n## Unused roots\n\n- userlib: no link is under <userlib>, so its directory \
                 was not used\n- vi.lib: no link is under <vi.lib>, so its directory was not \
                 used\n"
            ),
            "{markdown}"
        );
    }
    let value = serde_json::to_value(&found).unwrap();
    assert_eq!(
        value["unused_roots"],
        serde_json::json!(["userlib", "vi.lib"])
    );
    assert_eq!(
        serde_json::to_value(found.overview()).unwrap()["unused_roots"],
        value["unused_roots"]
    );
}

#[test]
fn root_specifications_are_checked() {
    let dir = TempDir::new("roots");
    assert_eq!(
        Roots::split("vilib=/opt/vi.lib"),
        Ok(("vilib", "/opt/vi.lib"))
    );
    // Only the first `=` separates. A directory name may hold one.
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
    dir.file("a file");
    let file = roots.add("vilib", &dir.0.join("a file")).unwrap_err();
    assert!(file.contains("is not a directory"), "{file}");
    roots.add("vilib", &dir.0).unwrap();
    // The same directory in another spelling is not a second directory,
    // and the first spelling stays the one that is reported.
    roots.add("vilib", &dir.0.join(".")).unwrap();
    fs::create_dir(dir.0.join("sub")).unwrap();
    roots.add("vilib", &dir.0.join("sub").join("..")).unwrap();
    assert_eq!(roots.names().collect::<Vec<_>>(), ["vilib"]);
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
    // `unused_roots` is left out when it is empty.
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
                vi(&[
                    symbolic("<vilib>/x/y.vi"),
                    LvPath::sibling("gone.vi"),
                    LvPath::sibling("lost.vi"),
                ]),
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
    let missing: Vec<&str> = found.missing.iter().map(|l| l.target.as_str()).collect();
    assert_eq!(missing, ["14-lost.vi", "19-gone.vi"]);
    assert_eq!(found.unparsed[0].path, "22-d.vi");
    assert_eq!(found.unparsed[1].path, "23-c.vi");
}

#[test]
fn the_overview_counts_what_the_report_lists() {
    let dir = TempDir::new("overview");
    dir.archive(
        "vi.lib/Utility/error.llb",
        &[b"Clear Errors.vi", b"Other.vi"],
    );
    let mut roots = Roots::default();
    roots.add("vilib", &dir.0.join("vi.lib")).unwrap();
    let mut found = report(
        vec![
            (
                at("a.vi"),
                vi(&[
                    symbolic("<vilib>/Utility/error.llb/Clear Errors.vi"),
                    symbolic("<vilib>/Utility/error.llb/Other.vi"),
                    symbolic("<vilib>/Utility/Gone.vi"),
                    symbolic("<userlib>/x/y.vi"),
                    LvPath::absolute(&["C", "far.vi"]),
                    LvPath::sibling("gone.vi"),
                ]),
            ),
            (
                at("b.vi"),
                vi(&[
                    symbolic("<vilib>/Utility/error.llb/Clear Errors.vi"),
                    symbolic("<vilib>/Utility//odd.vi"),
                    LvPath::sibling("gone.vi"),
                ]),
            ),
            (at("bad.vi"), FileState::Failed("short".to_owned())),
        ],
        &roots,
    );
    found.map_paths(|path| {
        path.replace(&text_of(&dir.0), "install")
            .replace(DIR, "ws")
            .replace('\\', "/")
    });
    let overview = found.overview();
    assert_eq!(
        serde_json::to_value(&overview).unwrap(),
        serde_json::json!({
            "files": 3,
            "roots": [
                { "root": "userlib", "packages": [
                    { "name": "x", "files": 1, "found": 0, "missing": 0, "unchecked": 1, "referrers": 1 },
                ] },
                { "root": "vilib", "directory": "install/vi.lib", "packages": [
                    { "name": "Utility", "files": 2, "found": 0, "missing": 1, "unchecked": 1, "referrers": 2 },
                    { "name": "Utility/error.llb", "files": 2, "found": 2, "missing": 0, "unchecked": 0, "referrers": 2 },
                ] },
            ],
            "absolute": [{ "target": "C:/far.vi", "referrers": 1 }],
            "missing": [{ "target": "ws/gone.vi", "referrers": 2 }],
            "unparsed": [{ "path": "ws/bad.vi", "message": "short" }],
        })
    );
    assert_eq!(
        overview_to_text(&found),
        "<userlib> (not checked: no directory was given for root userlib)
  x: 1 file (1 unchecked), used by 1 file

<vilib> (checked in install/vi.lib: 2 found, 1 missing, 1 unchecked)
  Utility: 2 files (1 missing, 1 unchecked), used by 2 files
  Utility/error.llb: 2 files (2 found), used by 2 files

Machine-absolute links (1)
  C:/far.vi (used by 1 file)

Missing relative links (1)
  ws/gone.vi (used by 2 files)

Unparsed files (1)
  ws/bad.vi: cannot analyze file: short

Scanned 3 files: 5 external files in 3 packages under 2 symbolic roots (2 found, 1 missing, 2 \
         unchecked), 1 machine-absolute link, 1 missing relative link, 1 unparsed file.
"
    );
    let markdown = overview_to_markdown(&found);
    assert!(markdown.starts_with("# External dependencies\n\nScanned 3 files"));
    assert!(markdown.contains(
        "\n## `<vilib>`\n\nChecked in install/vi.lib: 2 found, 1 missing, 1 unchecked.\n\n\
         | Package | Files | Status | Used by |\n| --- | --- | --- | --- |\n\
         | `Utility` | 2 | 1 missing, 1 unchecked | 2 files |\n\
         | `Utility/error.llb` | 2 | 2 found | 2 files |\n"
    ));
    assert!(markdown.contains("| `ws/gone.vi` | 2 files |"));
    assert!(markdown.contains("| `ws/bad.vi` | short |"));
}

/// The reports print a space for a character that would add a line or
/// reorder the text of one.
#[test]
fn a_name_cannot_add_a_line_or_reorder_one() {
    let name = "a\nb\u{9b}c\u{202e}d\u{2028}e|`f";
    assert_eq!(clean(name), "a b c d e|`f");
    assert_eq!(code(name), "`a b c d e  f`");
}
