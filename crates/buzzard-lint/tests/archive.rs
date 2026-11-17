//! `.llb` archives in a workspace: their members are files, links run
//! through them, and nothing in an archive can stand for another file.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use buzzard_lint::deps::{self, Roots};
use buzzard_lint::graph::CaseSensitivity;
use buzzard_lint::verify::report::Verified;
use buzzard_lint::{archive, lint, DependencyGraph, Diagnostic, FileState, LintConfig, Workspace};
use buzzard_rsrc::builder::{LlbBuilder, RsrcBuilder};
use buzzard_rsrc::{LvPath, PathKind};

const V23: u32 = 0x2300_8000;
/// Flag words for a clean file: compiled code separated, nothing else set.
const CLEAN: [u32; 6] = [0, 0x400, 0, 0, 0, 0];
/// Flag words of a file that keeps its compiled code, which is `BZ001`.
const INLINE_CODE: [u32; 6] = [0; 6];

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("buzzard-archive-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the temp dir");
        Self(dir)
    }

    fn write(&self, rel: &str, bytes: &[u8]) {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().expect("a file has a parent")).expect("create parents");
        fs::write(path, bytes).expect("write the file");
    }

    fn at(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    fn scan(&self) -> Workspace {
        Workspace::scan(std::slice::from_ref(&self.0))
    }

    fn rel(&self, path: &Path) -> String {
        path.strip_prefix(&self.0)
            .expect("paths are under the temp dir")
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn keys(&self, workspace: &Workspace) -> Vec<String> {
        workspace.files().keys().map(|p| self.rel(p)).collect()
    }

    fn diagnostics(&self) -> Vec<Diagnostic> {
        let workspace = self.scan();
        let graph = DependencyGraph::build(&workspace);
        lint(&workspace, &graph, &LintConfig::default())
    }

    /// Every finding as `(path, code)`.
    fn codes(&self) -> Vec<(String, &'static str)> {
        self.diagnostics()
            .iter()
            .map(|d| (self.rel(&d.path), d.code))
            .collect()
    }

    fn names(&self, paths: &[&Path]) -> Vec<String> {
        paths.iter().map(|p| self.rel(p)).collect()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn relative(path: &str) -> LvPath {
    LvPath {
        kind: PathKind::Relative,
        components: path.split('/').map(str::to_owned).collect(),
    }
}

/// A VI with the given flag words and links. A link is written like a
/// path from the file itself: `/a.vi` is a sibling, `//a.vi` one level up.
fn vi(words: &[u32], links: &[&str]) -> Vec<u8> {
    let links: Vec<(&str, LvPath)> = links
        .iter()
        .map(|link| (link.rsplit('/').next().unwrap_or(link), relative(link)))
        .collect();
    RsrcBuilder::default()
        .save_record(V23, words)
        .links(&links)
        .build()
}

fn clean() -> Vec<u8> {
    vi(&CLEAN, &[])
}

fn project(urls: &[&str]) -> Vec<u8> {
    let mut xml = String::from("<Project Type=\"Project\" LVVersion=\"23008000\">\n");
    for url in urls {
        let name = url.rsplit('/').next().unwrap_or(url);
        let _ = writeln!(xml, "<Item Name=\"{name}\" Type=\"VI\" URL=\"{url}\"/>");
    }
    xml.push_str("</Project>\n");
    xml.into_bytes()
}

#[test]
fn members_are_files_of_the_workspace_and_every_rule_runs_on_them() {
    let dir = TempDir::new("members");
    dir.write("Main.vi", &clean());
    dir.write(
        "lib/util.llb",
        &LlbBuilder::default()
            .member("Inline.vi", &vi(&INLINE_CODE, &[]))
            .compressed("Clean.ctl", &clean())
            .compressed("Old.vi", &vi(&[0, 0x400], &[]))
            .member("readme.txt", b"not a resource file")
            .build(),
    );
    let workspace = dir.scan();
    assert_eq!(
        dir.keys(&workspace),
        [
            "Main.vi",
            "lib/util.llb",
            "lib/util.llb/Clean.ctl",
            "lib/util.llb/Inline.vi",
            "lib/util.llb/Old.vi",
        ]
    );
    let FileState::Archive(listing) = workspace.get(&dir.at("lib/util.llb")).unwrap() else {
        panic!("the archive is an archive")
    };
    // Directory order, and what is not a VI or control is listed only.
    let listed: Vec<&str> = listing.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(listed, ["Inline.vi", "Clean.ctl", "Old.vi", "readme.txt"]);
    assert_eq!(listing.opened(), ["Inline.vi", "Clean.ctl", "Old.vi"]);
    assert!(workspace
        .get(&dir.at("lib/util.llb/Inline.vi"))
        .and_then(FileState::metadata)
        .is_some());
    assert_eq!(
        dir.codes(),
        [("lib/util.llb/Inline.vi".to_owned(), "BZ001")]
    );
    assert_eq!(
        workspace
            .archive_of(&dir.at("lib/util.llb/Inline.vi"))
            .map(|(archive, name)| (dir.rel(archive), name)),
        Some(("lib/util.llb".to_owned(), "Inline.vi".to_owned()))
    );
    assert_eq!(workspace.archive_of(&dir.at("Main.vi")), None);
    assert_eq!(
        workspace.file_on_disk(&dir.at("lib/util.llb/Inline.vi")),
        dir.at("lib/util.llb")
    );
}

#[test]
fn links_run_through_an_archive_in_both_directions() {
    let dir = TempDir::new("links");
    // Main calls a member; the member calls a sibling in the archive and
    // a file beside the archive.
    dir.write("Main.vi", &vi(&CLEAN, &["/lib.llb/Entry.vi"]));
    dir.write("Outside.vi", &clean());
    dir.write(
        "lib.llb",
        &LlbBuilder::default()
            .member("Entry.vi", &vi(&CLEAN, &["/Helper.vi", "//Outside.vi"]))
            .compressed("Helper.vi", &clean())
            .build(),
    );
    let workspace = dir.scan();
    let graph = DependencyGraph::build(&workspace);
    let entry = dir.at("lib.llb/Entry.vi");
    assert_eq!(dir.names(&graph.callers(&entry)), ["Main.vi"]);
    assert_eq!(
        dir.names(&graph.callees(&entry)),
        ["Outside.vi", "lib.llb/Helper.vi"]
    );
    assert_eq!(
        dir.names(&graph.callees(&dir.at("Main.vi"))),
        ["lib.llb/Entry.vi"]
    );
    assert_eq!(
        dir.names(&graph.callers(&dir.at("Outside.vi"))),
        ["lib.llb/Entry.vi"]
    );
    assert_eq!(
        dir.names(&graph.callers(&dir.at("lib.llb/Helper.vi"))),
        ["lib.llb/Entry.vi"]
    );
    // The archive holds its members, as a library lists its own.
    assert_eq!(
        dir.names(&graph.members(&dir.at("lib.llb"))),
        ["lib.llb/Entry.vi", "lib.llb/Helper.vi"]
    );
    assert_eq!(dir.names(&graph.owners(&entry)), ["lib.llb"]);
    assert_eq!(dir.codes().len(), 0);
    // Nothing the workspace has is reported as missing.
    let report = deps::scan(&workspace, &graph, &Roots::default());
    assert!(report.missing.is_empty(), "{:?}", report.missing);
}

#[test]
fn a_link_into_an_archive_ignores_letter_case_where_the_file_system_does() {
    let dir = TempDir::new("case");
    dir.write("Main.vi", &vi(&CLEAN, &["/LIB.LLB/entry.VI"]));
    dir.write(
        "lib.llb",
        &LlbBuilder::default().member("Entry.vi", &clean()).build(),
    );
    let workspace = dir.scan();
    let callees = |case| {
        let graph = DependencyGraph::build_with(&workspace, case);
        dir.names(&graph.callees(&dir.at("Main.vi")))
    };
    assert_eq!(callees(CaseSensitivity::Insensitive), ["lib.llb/Entry.vi"]);
    assert_eq!(callees(CaseSensitivity::Sensitive).len(), 0);
}

#[test]
fn a_cycle_through_an_archive_is_found() {
    let dir = TempDir::new("cycle");
    dir.write("Main.vi", &vi(&CLEAN, &["/lib.llb/Entry.vi"]));
    dir.write(
        "lib.llb",
        &LlbBuilder::default()
            .member("Entry.vi", &vi(&CLEAN, &["/Deep.vi"]))
            .member("Deep.vi", &vi(&CLEAN, &["//Main.vi"]))
            .build(),
    );
    assert_eq!(
        dir.codes(),
        [
            ("Main.vi".to_owned(), "BZ005"),
            ("lib.llb/Deep.vi".to_owned(), "BZ005"),
            ("lib.llb/Entry.vi".to_owned(), "BZ005"),
        ]
    );
    let found = dir.diagnostics();
    assert!(found[0]
        .message
        .contains("Main.vi -> Entry.vi -> Deep.vi -> Main.vi"));
    assert_eq!(found[0].related.len(), 2);
}

/// What `check` (rule `BZ006`) and `deps` say about the members a project
/// file lists inside an archive.
fn missing_members(dir: &TempDir, roots: &[PathBuf]) -> (Vec<String>, Vec<String>) {
    let workspace = Workspace::scan(roots);
    let graph = DependencyGraph::build(&workspace);
    let flagged = lint(&workspace, &graph, &LintConfig::default())
        .into_iter()
        .filter(|d| d.code == "BZ006")
        .map(|d| d.message)
        .collect();
    let report = deps::scan(&workspace, &graph, &Roots::default());
    let missing = report
        .missing
        .iter()
        .map(|link| dir.rel(Path::new(&link.target)))
        .collect();
    (flagged, missing)
}

#[test]
fn a_member_the_archive_does_not_hold_is_missing_once_for_check_and_for_deps() {
    let dir = TempDir::new("absent");
    dir.write(
        "lib.llb",
        &LlbBuilder::default().member("Present.vi", &clean()).build(),
    );
    dir.write("broken.llb", b"not an archive");
    dir.write(
        "App.lvproj",
        &project(&[
            "../lib.llb/Present.vi",
            "../lib.llb/present.VI",
            "../lib.llb/Gone.vi",
            "../lib.llb/Gone.vi",
            "../lib.llb/sub/Present.vi",
            "../broken.llb/Any.vi",
            "../absent.llb/Any.vi",
        ]),
    );
    let expected_flags = [
        "member `Any.vi` does not exist on disk (`../absent.llb/Any.vi`)",
        "member `Gone.vi` does not exist on disk (`../lib.llb/Gone.vi`)",
        "member `Present.vi` does not exist on disk (`../lib.llb/sub/Present.vi`)",
    ];
    let expected_missing = [
        "absent.llb/Any.vi",
        "lib.llb/Gone.vi",
        "lib.llb/sub/Present.vi",
    ];
    // With the archive in the workspace, and with only the project file
    // scanned so that the archive is looked up on disk.
    for roots in [vec![dir.0.clone()], vec![dir.at("App.lvproj")]] {
        let (flagged, missing) = missing_members(&dir, &roots);
        assert_eq!(flagged, expected_flags, "{roots:?}");
        assert_eq!(missing, expected_missing, "{roots:?}");
    }
}

#[test]
fn a_link_from_a_vi_to_an_absent_member_is_a_missing_relative_link() {
    let dir = TempDir::new("absent-link");
    dir.write(
        "Main.vi",
        &vi(
            &CLEAN,
            &[
                "/lib.llb/Present.vi",
                "/lib.llb/Gone.vi",
                "/broken.llb/Any.vi",
            ],
        ),
    );
    dir.write("Other.vi", &vi(&CLEAN, &["/lib.llb/Gone.vi"]));
    dir.write(
        "lib.llb",
        &LlbBuilder::default().member("Present.vi", &clean()).build(),
    );
    dir.write("broken.llb", b"not an archive");
    for roots in [
        vec![dir.0.clone()],
        vec![dir.at("Main.vi"), dir.at("Other.vi")],
    ] {
        let workspace = Workspace::scan(&roots);
        let graph = DependencyGraph::build(&workspace);
        let report = deps::scan(&workspace, &graph, &Roots::default());
        // One target, however many files link to it. The archive that
        // cannot be read says nothing about its members.
        assert_eq!(report.missing.len(), 1, "{:?}", report.missing);
        assert_eq!(
            dir.rel(Path::new(&report.missing[0].target)),
            "lib.llb/Gone.vi"
        );
        assert_eq!(report.missing[0].referrers.len(), 2);
    }
}

#[test]
fn damage_to_one_member_does_not_hide_the_others() {
    let dir = TempDir::new("damage");
    let bytes = LlbBuilder::default()
        .member("First.vi", &vi(&INLINE_CODE, &[]))
        .compressed("Second.vi", &vi(&INLINE_CODE, &[]))
        .member("Third.vi", &vi(&INLINE_CODE, &[]))
        .member("Fourth.vi", b"RSRC but not a file")
        .build();
    // Break the zlib stream of the second member.
    let stream = bytes
        .windows(2)
        .position(|pair| pair == [0x78, 0x9c])
        .expect("the archive holds a zlib stream");
    let mut damaged = bytes;
    for byte in &mut damaged[stream + 2..stream + 10] {
        *byte ^= 0xff;
    }
    dir.write("lib.llb", &damaged);
    assert_eq!(
        dir.codes(),
        [
            ("lib.llb/First.vi".to_owned(), "BZ001"),
            ("lib.llb/Fourth.vi".to_owned(), "BZ000"),
            ("lib.llb/Second.vi".to_owned(), "BZ000"),
            ("lib.llb/Third.vi".to_owned(), "BZ001"),
        ]
    );
    let report = {
        let workspace = dir.scan();
        let graph = DependencyGraph::build(&workspace);
        deps::scan(&workspace, &graph, &Roots::default())
    };
    let unparsed: Vec<String> = report
        .unparsed
        .iter()
        .map(|u| dir.rel(Path::new(&u.path)))
        .collect();
    assert_eq!(unparsed, ["lib.llb/Fourth.vi", "lib.llb/Second.vi"]);
}

#[test]
fn an_archive_whose_directory_cannot_be_read_is_one_finding() {
    let dir = TempDir::new("no-directory");
    dir.write("garbage.llb", b"this is not a resource file");
    let mut cut = LlbBuilder::default()
        .member("A.vi", &clean())
        .member("B.vi", &clean())
        .build();
    cut.truncate(cut.len() - 40);
    dir.write("cut.llb", &cut);
    // An archive of a built application: no directory to read.
    dir.write(
        "built.llb",
        &RsrcBuilder::new(buzzard_rsrc::header::file_type::LIBRARY)
            .resource(b"LVzp", 0, Some("embdZip"), vec![0; 64])
            .build(),
    );
    assert_eq!(
        dir.codes(),
        [
            ("built.llb".to_owned(), "BZ000"),
            ("cut.llb".to_owned(), "BZ000"),
            ("garbage.llb".to_owned(), "BZ000"),
        ]
    );
    assert_eq!(
        dir.keys(&dir.scan()),
        ["built.llb", "cut.llb", "garbage.llb"]
    );
}

/// A member name is data. Whatever it holds, no file of the workspace
/// gets a path outside its archive, and no member stands for a file on
/// disk.
#[test]
fn a_member_name_never_escapes_or_aliases_another_file() {
    let dir = TempDir::new("escape");
    dir.write("victim.vi", &clean());
    dir.write("ws/sub/x.vi", &clean());
    let hostile = vi(&INLINE_CODE, &[]);
    let names: [&[u8]; 9] = [
        b"../../victim.vi",
        b"../x.vi",
        b"..",
        b".",
        b"",
        b"sub/x.vi",
        b"sub\\x.vi",
        b"/etc/passwd.vi",
        b"nul\0.vi",
    ];
    let mut builder = LlbBuilder::default();
    for name in names {
        builder = builder.member_bytes(name, &hostile);
    }
    let archive_bytes = builder
        .member("C:drive.vi", &hostile)
        .member("Same.vi", &hostile)
        .member("Same.vi", &clean())
        .build();
    dir.write("ws/a.llb", &archive_bytes);

    let workspace = Workspace::scan(&[dir.at("ws")]);
    // Only names that are one path component became files, each inside
    // the archive.
    assert_eq!(
        dir.keys(&workspace),
        [
            "ws/a.llb",
            "ws/a.llb/C:drive.vi",
            "ws/a.llb/Same.vi",
            "ws/sub/x.vi"
        ]
    );
    for key in workspace.files().keys() {
        assert!(key.starts_with(dir.at("ws")), "{}", key.display());
    }
    // The real files are what they were: clean, not the hostile member.
    let graph = DependencyGraph::build(&workspace);
    let found = lint(&workspace, &graph, &LintConfig::default());
    let shown: Vec<(String, &str)> = found.iter().map(|d| (dir.rel(&d.path), d.code)).collect();
    let mut expected = vec![("ws/a.llb".to_owned(), "BZ000"); 10];
    expected.push(("ws/a.llb/C:drive.vi".to_owned(), "BZ001"));
    expected.push(("ws/a.llb/Same.vi".to_owned(), "BZ001"));
    assert_eq!(shown, expected);
    assert!(found
        .iter()
        .any(|d| d.message == "cannot open member `../../victim.vi`: its name is not a file name"));
    assert!(found
        .iter()
        .any(|d| d.message == "cannot open a second member named `Same.vi`: links name the first"));
    assert!(found.iter().all(|d| !d.message.contains('\0')));
    assert!(workspace.get(&dir.at("victim.vi")).is_none());

    // Asking for such a member by its path reads the file on disk, or
    // nothing: never the member.
    for path in ["ws/a.llb/../sub/x.vi", "ws/a.llb/../../victim.vi"] {
        assert!(
            archive::read(&dir.at(path)).map_or(true, |bytes| bytes == clean()),
            "{path}"
        );
        assert_eq!(archive::split(&dir.at(path)), None, "{path}");
    }
    assert!(archive::read(&dir.at("ws/a.llb/sub/x.vi")).is_err());
    assert!(archive::read(&dir.at("ws/a.llb/..")).is_err());
    assert_eq!(archive::read(&dir.at("ws/a.llb/Same.vi")).unwrap(), hostile);
    assert_eq!(archive::split(&dir.at("ws/a.llb/sub/x.vi")), None);
    assert_eq!(
        archive::split(&dir.at("ws/a.llb/Same.vi")),
        Some((dir.at("ws/a.llb"), "Same.vi".to_owned()))
    );
}

#[test]
fn an_archive_inside_an_archive_is_listed_and_not_opened() {
    let dir = TempDir::new("nested");
    let inner = LlbBuilder::default()
        .member("Deep.vi", &vi(&INLINE_CODE, &[]))
        .build();
    dir.write(
        "outer.llb",
        &LlbBuilder::default()
            .member("inner.llb", &inner)
            .member("Top.vi", &clean())
            .build(),
    );
    let workspace = dir.scan();
    assert_eq!(dir.keys(&workspace), ["outer.llb", "outer.llb/Top.vi"]);
    let listing = workspace
        .get(&dir.at("outer.llb"))
        .and_then(FileState::archive)
        .unwrap();
    assert_eq!(listing.members.len(), 2);
    assert_eq!(dir.codes().len(), 0);
    assert!(archive::read(&dir.at("outer.llb/inner.llb/Deep.vi")).is_err());
}

#[test]
fn a_directory_called_like_an_archive_is_a_directory() {
    let dir = TempDir::new("directory");
    dir.write("utility.llb/Inside.vi", &vi(&INLINE_CODE, &[]));
    dir.write("Main.vi", &vi(&CLEAN, &["/utility.llb/Inside.vi"]));
    let workspace = dir.scan();
    assert_eq!(dir.keys(&workspace), ["Main.vi", "utility.llb/Inside.vi"]);
    assert_eq!(workspace.archive_of(&dir.at("utility.llb/Inside.vi")), None);
    assert_eq!(archive::split(&dir.at("utility.llb/Inside.vi")), None);
    let graph = DependencyGraph::build(&workspace);
    assert_eq!(
        dir.names(&graph.callees(&dir.at("Main.vi"))),
        ["utility.llb/Inside.vi"]
    );
}

#[test]
fn a_member_path_is_a_scan_root() {
    let dir = TempDir::new("root");
    dir.write(
        "lib.llb",
        &LlbBuilder::default()
            .member("Inline.vi", &vi(&INLINE_CODE, &[]))
            .member("Other.vi", &vi(&INLINE_CODE, &[]))
            .build(),
    );
    let member = dir.at("lib.llb/Inline.vi");
    let workspace = Workspace::scan(std::slice::from_ref(&member));
    assert_eq!(dir.keys(&workspace), ["lib.llb/Inline.vi"]);
    assert!(workspace.get(&member).unwrap().metadata().is_some());
    // With its archive among the roots it is still one file.
    let both = Workspace::scan(&[member.clone(), dir.0.clone()]);
    assert_eq!(
        dir.keys(&both),
        ["lib.llb", "lib.llb/Inline.vi", "lib.llb/Other.vi"]
    );
    // A member the archive does not hold is a file that cannot be read.
    let absent = dir.at("lib.llb/Absent.vi");
    assert!(archive::split(&absent).is_some());
    assert!(!archive::exists(&absent));
    assert!(archive::exists(&member));
    assert!(matches!(
        FileState::load(&absent),
        FileState::Failed(reason) if reason.contains("no member `Absent.vi`")
    ));
}

#[test]
fn refreshing_an_archive_re_reads_its_members() {
    let dir = TempDir::new("refresh");
    dir.write(
        "lib.llb",
        &LlbBuilder::default()
            .member("Old.vi", &clean())
            .member("Stays.vi", &clean())
            .build(),
    );
    let mut workspace = dir.scan();
    assert_eq!(
        dir.keys(&workspace),
        ["lib.llb", "lib.llb/Old.vi", "lib.llb/Stays.vi"]
    );
    dir.write(
        "lib.llb",
        &LlbBuilder::default()
            .member("Stays.vi", &vi(&INLINE_CODE, &[]))
            .member("New.vi", &clean())
            .build(),
    );
    workspace.refresh(&dir.at("lib.llb"));
    assert_eq!(
        dir.keys(&workspace),
        ["lib.llb", "lib.llb/New.vi", "lib.llb/Stays.vi"]
    );
    assert_eq!(workspace, dir.scan());
    // An archive that stops being one loses its members.
    dir.write("lib.llb", b"garbage");
    workspace.refresh(&dir.at("lib.llb"));
    assert_eq!(dir.keys(&workspace), ["lib.llb"]);
    dir.write(
        "lib.llb",
        &LlbBuilder::default().member("Back.vi", &clean()).build(),
    );
    workspace.refresh(&dir.at("lib.llb"));
    assert_eq!(dir.keys(&workspace), ["lib.llb", "lib.llb/Back.vi"]);
    fs::remove_file(dir.at("lib.llb")).unwrap();
    workspace.refresh(&dir.at("lib.llb"));
    assert_eq!(workspace.files().len(), 0);
}

#[test]
fn scanning_an_archive_is_deterministic() {
    let dir = TempDir::new("deterministic");
    let mut builder = LlbBuilder::default();
    for i in (0..40).rev() {
        let words = if i % 3 == 0 { INLINE_CODE } else { CLEAN };
        builder = builder.compressed(&format!("Member {i:02}.vi"), &vi(&words, &[]));
    }
    dir.write("big.llb", &builder.build());
    let first = dir.scan();
    assert_eq!(first.files().len(), 41);
    let listing = first
        .get(&dir.at("big.llb"))
        .and_then(FileState::archive)
        .unwrap();
    // The listing keeps the order of the archive; the files are sorted.
    assert_eq!(listing.members[0].name, "Member 39.vi");
    assert_eq!(dir.keys(&first)[1], "big.llb/Member 00.vi");
    let diagnostics = dir.diagnostics();
    assert_eq!(diagnostics.len(), 14);
    for _ in 0..5 {
        assert_eq!(dir.scan(), first);
        assert_eq!(dir.diagnostics(), diagnostics);
    }
}

#[test]
fn sarif_locates_a_member_finding_at_its_archive() {
    let dir = TempDir::new("sarif");
    dir.write("Loose.vi", &vi(&INLINE_CODE, &[]));
    dir.write(
        "my lib.llb",
        &LlbBuilder::default()
            .member("In side.vi", &vi(&INLINE_CODE, &["//Loose.vi"]))
            .build(),
    );
    dir.write("Back.vi", &vi(&CLEAN, &["/my lib.llb/Cycle.vi"]));
    let workspace = dir.scan();
    let graph = DependencyGraph::build(&workspace);
    let diagnostics = lint(&workspace, &graph, &LintConfig::default());
    let sarif = buzzard_lint::sarif::to_sarif(&workspace, &diagnostics, "1.2.3");
    let results = sarif["runs"][0]["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    let uri = |result: &serde_json::Value| {
        result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    // Every artifact is a file that exists.
    for result in results {
        let path = uri(result);
        let path = path.strip_prefix("file://").unwrap().replace("%20", " ");
        assert!(Path::new(&path).is_file(), "{path}");
    }
    let member = results
        .iter()
        .find(|r| uri(r).ends_with("my%20lib.llb"))
        .expect("the member's finding is at the archive");
    assert_eq!(member["ruleId"], "BZ001");
    assert!(member["message"]["text"]
        .as_str()
        .unwrap()
        .starts_with("In side.vi: compiled code is not separated"));
    let logical = &member["locations"][0]["logicalLocations"][0];
    assert_eq!(logical["name"], "In side.vi");
    assert_eq!(logical["kind"], "member");
    assert!(logical["fullyQualifiedName"]
        .as_str()
        .unwrap()
        .ends_with("my lib.llb/In side.vi"));
    let loose = results
        .iter()
        .find(|r| uri(r).ends_with("Loose.vi"))
        .unwrap();
    assert!(loose["locations"][0].get("logicalLocations").is_none());
    assert!(loose["message"]["text"]
        .as_str()
        .unwrap()
        .starts_with("compiled code"));
}

#[test]
fn a_finding_moved_to_its_archive_names_the_member() {
    let dir = TempDir::new("on-disk");
    dir.write("Main.vi", &vi(&CLEAN, &["/lib.llb/Entry.vi"]));
    dir.write(
        "lib.llb",
        &LlbBuilder::default()
            .member("Entry.vi", &vi(&CLEAN, &["/Deep.vi"]))
            .member("Deep.vi", &vi(&CLEAN, &["//Main.vi"]))
            .build(),
    );
    let workspace = dir.scan();
    let graph = DependencyGraph::build(&workspace);
    let shown: Vec<(String, String, Vec<String>)> =
        lint(&workspace, &graph, &LintConfig::default())
            .iter()
            .map(|d| d.on_disk(&workspace).0)
            .map(|d| {
                (
                    dir.rel(&d.path),
                    d.message.split(':').next().unwrap().to_owned(),
                    d.related.iter().map(|p| dir.rel(p)).collect(),
                )
            })
            .collect();
    assert_eq!(
        shown,
        [
            (
                "Main.vi".to_owned(),
                "part of a circular dependency".to_owned(),
                // Both other files of the cycle are in one archive.
                vec!["lib.llb".to_owned()]
            ),
            (
                "lib.llb".to_owned(),
                "Deep.vi".to_owned(),
                vec!["Main.vi".to_owned()]
            ),
            (
                "lib.llb".to_owned(),
                "Entry.vi".to_owned(),
                vec!["Main.vi".to_owned()]
            ),
        ]
    );
}

#[test]
fn verify_checks_the_state_machines_of_members() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify");
    let machine = fs::read(fixtures.join("jki-state-machine/new-support-vis-examples.vi")).unwrap();
    let dir = TempDir::new("verify");
    dir.write("loose/Machine.vi", &machine);
    dir.write(
        "packed.llb",
        &LlbBuilder::default()
            .compressed("Machine.vi", &machine)
            .member("Plain.vi", &clean())
            .member("Broken.vi", b"RSRC")
            .build(),
    );
    let verified = Verified::scan(std::slice::from_ref(&dir.0));
    let files: Vec<String> = verified.files.iter().map(|f| dir.rel(&f.path)).collect();
    assert_eq!(files, ["loose/Machine.vi", "packed.llb/Machine.vi"]);
    // The member gives what the same file gives on disk.
    assert_eq!(verified.files[0].findings, verified.files[1].findings);
    assert_ne!(verified.files[1].findings.len(), 0);
    let unreadable: Vec<String> = verified
        .unreadable
        .iter()
        .map(|(path, _)| dir.rel(path))
        .collect();
    assert_eq!(unreadable, ["packed.llb/Broken.vi"]);
    // And the rules of `check` report the same findings on the member.
    let codes = dir.codes();
    let on = |path: &str| -> Vec<&str> {
        codes
            .iter()
            .filter(|(p, code)| p == path && code.starts_with("BZ2"))
            .map(|(_, code)| *code)
            .collect()
    };
    assert_eq!(on("loose/Machine.vi"), on("packed.llb/Machine.vi"));
    assert_ne!(on("packed.llb/Machine.vi").len(), 0);
}
