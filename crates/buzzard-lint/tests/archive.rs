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
use buzzard_rsrc::header::file_type;
use buzzard_rsrc::{FileKind, FourCc, LvPath, PathKind};

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
        panic!("`lib/util.llb` was not read as an archive")
    };
    // The listing is in directory order. A member that is not a VI or a
    // control is listed and not opened.
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
            .map(|(archive, name)| (dir.rel(&archive), name)),
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
    // Main calls a member. The member calls a sibling in the archive and
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
    // The first roots put the archive in the workspace. The second are
    // only the project file, so that the archive is looked up on disk.
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
        // A target is reported once, however many files link to it.
        // Nothing is known about the members of the archive that cannot
        // be read.
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
    // An archive of a built application has no directory to read. It is
    // a resource file like any other, and nothing about it is a problem.
    dir.write(
        "built.llb",
        &RsrcBuilder::new(file_type::LIBRARY)
            .resource(b"LVzp", 0, Some("embdZip"), vec![0; 64])
            .build(),
    );
    assert_eq!(
        dir.codes(),
        [
            ("cut.llb".to_owned(), "BZ000"),
            ("garbage.llb".to_owned(), "BZ000"),
        ]
    );
    let workspace = dir.scan();
    assert_eq!(
        dir.keys(&workspace),
        ["built.llb", "cut.llb", "garbage.llb"]
    );
    let built = workspace.get(&dir.at("built.llb")).unwrap();
    assert_eq!(built.metadata().unwrap().kind, FileKind::Library);
    assert_eq!(archive::list(&dir.at("built.llb")), Ok(None));
    assert!(archive::list(&dir.at("cut.llb")).is_err());
    let verified = Verified::scan(std::slice::from_ref(&dir.0));
    let unreadable: Vec<String> = verified
        .unreadable
        .iter()
        .map(|(path, _)| dir.rel(path))
        .collect();
    assert_eq!(unreadable, ["cut.llb", "garbage.llb"]);
    // A link into it is not known to be there or to be missing.
    dir.write("ws/Main.vi", &vi(&CLEAN, &["//built.llb/Any.vi"]));
    let workspace = Workspace::scan(&[dir.at("ws")]);
    let graph = DependencyGraph::build(&workspace);
    assert_eq!(
        deps::scan(&workspace, &graph, &Roots::default()).missing,
        []
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
    // On Windows a name with a colon is not a file name either, because
    // appended to a path it would name a drive.
    let mut keys = vec!["ws/a.llb", "ws/a.llb/Same.vi", "ws/sub/x.vi"];
    if !cfg!(windows) {
        keys.insert(1, "ws/a.llb/C:drive.vi");
    }
    assert_eq!(dir.keys(&workspace), keys);
    for key in workspace.files().keys() {
        assert!(key.starts_with(dir.at("ws")), "{}", key.display());
    }
    // The real files are unchanged and clean. The hostile member did not
    // replace them.
    let graph = DependencyGraph::build(&workspace);
    let found = lint(&workspace, &graph, &LintConfig::default());
    let shown: Vec<(String, &str)> = found.iter().map(|d| (dir.rel(&d.path), d.code)).collect();
    let mut expected = vec![("ws/a.llb".to_owned(), "BZ000"); 10];
    if cfg!(windows) {
        expected.push(("ws/a.llb".to_owned(), "BZ000"));
    } else {
        expected.push(("ws/a.llb/C:drive.vi".to_owned(), "BZ001"));
    }
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

    // Asking for such a member by its path reads the file on disk or
    // nothing, and never the member.
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
    // A name that the scan refuses is also refused as a path, including
    // on a system where a backslash is not a separator.
    let backslash = dir.at("ws/a.llb").join("sub\\x.vi");
    assert_eq!(archive::split(&backslash), None);
    assert!(archive::read(&backslash).is_err());
    assert!(!archive::exists(&backslash));
    assert!(Workspace::scan(std::slice::from_ref(&backslash))
        .files()
        .values()
        .all(|state| state.metadata().is_none()));
    assert_eq!(
        archive::split(&dir.at("ws/a.llb/Same.vi")),
        Some((dir.at("ws/a.llb"), "Same.vi".to_owned()))
    );
}

/// A name is printed on one line and as the archive spells it. Its
/// control characters are escaped, and nothing else is. A name with a
/// control character is not the name of a file, so it never starts a line
/// of a report.
#[test]
fn a_member_name_is_printed_on_one_line() {
    let dir = TempDir::new("one-line");
    let hostile = vi(&INLINE_CODE, &[]);
    let names: [&[u8]; 5] = [
        b"new\nline.vi",
        b"\x1b[31mred.vi",
        b"tab\there.vi",
        b"sub\\x.vi",
        b"nul\0.vi",
    ];
    let mut builder = LlbBuilder::default();
    for name in names {
        builder = builder.member_bytes(name, &hostile);
    }
    dir.write("ws/a.llb", &builder.build());
    let workspace = Workspace::scan(&[dir.at("ws")]);
    assert_eq!(dir.keys(&workspace), ["ws/a.llb"]);
    let graph = DependencyGraph::build(&workspace);
    let found = lint(&workspace, &graph, &LintConfig::default());
    assert_eq!(found.len(), 5);
    let messages: Vec<&str> = found.iter().map(|d| d.message.as_str()).collect();
    for expected in [
        "cannot open member `new\\nline.vi`: its name is not a file name",
        "cannot open member `\\u{1b}[31mred.vi`: its name is not a file name",
        "cannot open member `tab\\there.vi`: its name is not a file name",
        "cannot open member `sub\\x.vi`: its name is not a file name",
        "cannot open member `nul\\u{0}.vi`: its name is not a file name",
    ] {
        assert!(messages.contains(&expected), "{expected}: {messages:?}");
    }
    for d in &found {
        let path = d.path.to_string_lossy();
        assert!(
            !d.message
                .chars()
                .chain(path.chars())
                .any(|c| c.is_ascii_control()),
            "{d:?}"
        );
    }
    let listing = buzzard_lint::summary::archive_to_markdown(
        "a.llb",
        workspace
            .get(&dir.at("ws/a.llb"))
            .unwrap()
            .archive()
            .unwrap(),
    );
    assert!(
        listing.contains("\nNot opened: member `sub\\x.vi`: its name is not a file name.\n"),
        "{listing}"
    );
    assert!(listing.contains("| new\\nline.vi | VI"), "{listing}");
    assert_eq!(listing.lines().filter(|l| l.contains("line.vi")).count(), 2);
    // A member that is not opened by a scan is not opened by its path
    // either (a backslash is an ordinary character of a path on Unix).
    #[cfg(unix)]
    for name in ["sub\\x.vi", "new\nline.vi", "\u{1b}[31mred.vi"] {
        let path = dir.at("ws/a.llb").join(name);
        assert_eq!(archive::split(&path), None, "{name:?}");
        assert!(!archive::exists(&path), "{name:?}");
        assert!(archive::read(&path).is_err(), "{name:?}");
        let alone = Workspace::scan(std::slice::from_ref(&path));
        assert!(
            alone
                .files()
                .values()
                .all(|s| matches!(s, FileState::Failed(_))),
            "{name:?}"
        );
    }
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
    // Asked for by its path, the inner archive is a resource file. It is
    // parsed as one, and its members are not listed.
    let inner = FileState::load_all(&dir.at("outer.llb/inner.llb"));
    assert_eq!(inner.len(), 1);
    assert_eq!(
        inner[0].1.metadata().map(|m| m.kind),
        Some(FileKind::Library)
    );
}

/// An archive in an archive is never opened, so a link into one is
/// neither found nor missing.
#[test]
fn a_link_into_an_archive_in_an_archive_is_not_known_to_be_missing() {
    let dir = TempDir::new("nested-link");
    let inner = LlbBuilder::default().member("In.vi", &clean()).build();
    dir.write(
        "vilib/Utility/error.llb",
        &LlbBuilder::default()
            .member("inner.llb", &inner)
            .member("Top.vi", &clean())
            .build(),
    );
    let vilib = |rest: &str| LvPath {
        kind: PathKind::Absolute,
        components: format!("<vilib>/Utility/error.llb/{rest}")
            .split('/')
            .map(str::to_owned)
            .collect(),
    };
    let links = [
        ("In.vi", vilib("inner.llb/In.vi")),
        ("Top.vi", vilib("Top.vi")),
        ("Gone.vi", vilib("Gone.vi")),
        ("Deep.vi", vilib("Top.vi/Deep.vi")),
        (
            "In.vi",
            relative("//vilib/Utility/error.llb/inner.llb/In.vi"),
        ),
        ("X.vi", relative("//vilib/Utility/error.llb/Top.vi/X.vi")),
    ];
    dir.write(
        "ws/Main.vi",
        &RsrcBuilder::default()
            .save_record(V23, &CLEAN)
            .links(
                &links
                    .iter()
                    .map(|(n, p)| (*n, p.clone()))
                    .collect::<Vec<_>>(),
            )
            .build(),
    );
    dir.write(
        "ws/App.lvproj",
        &project(&[
            "../../vilib/Utility/error.llb/inner.llb/In.vi",
            "../../vilib/Utility/error.llb/Top.vi/X.vi",
        ]),
    );
    let archives = archive::Archives::default();
    let outer = dir.at("vilib/Utility/error.llb");
    assert_eq!(archives.contains(&outer, &["inner.llb", "In.vi"]), None);
    assert_eq!(archives.contains(&outer, &["INNER.LLB", "x", "y"]), None);
    assert_eq!(archives.contains(&outer, &["Top.vi", "X.vi"]), Some(false));
    assert_eq!(
        archives.contains(&outer, &["absent.llb", "X.vi"]),
        Some(false)
    );
    assert_eq!(archives.contains(&outer, &["inner.llb"]), Some(true));

    let workspace = Workspace::scan(&[dir.at("ws")]);
    let graph = DependencyGraph::build(&workspace);
    let flagged: Vec<String> = lint(&workspace, &graph, &LintConfig::default())
        .into_iter()
        .filter(|d| d.code == "BZ006")
        .map(|d| d.message)
        .collect();
    assert_eq!(
        flagged,
        ["member `X.vi` does not exist on disk (`../../vilib/Utility/error.llb/Top.vi/X.vi`)"]
    );
    let mut roots = Roots::default();
    roots.add("vilib", &dir.at("vilib")).unwrap();
    let report = deps::scan(&workspace, &graph, &roots);
    let missing: Vec<String> = report
        .missing
        .iter()
        .map(|link| dir.rel(Path::new(&link.target)))
        .collect();
    assert_eq!(missing, ["vilib/Utility/error.llb/Top.vi/X.vi"]);
    let statuses: Vec<(String, String)> = report
        .roots
        .iter()
        .flat_map(|root| &root.packages)
        .flat_map(|package| &package.targets)
        .map(|target| {
            let status = serde_json::to_value(target.status).unwrap();
            (target.path.clone(), status.as_str().unwrap().to_owned())
        })
        .collect();
    let status = |rest: &str| {
        let wanted = format!("<vilib>/Utility/error.llb/{rest}");
        let found = statuses.iter().find(|(path, _)| *path == wanted);
        found
            .unwrap_or_else(|| panic!("{wanted}: {statuses:?}"))
            .1
            .clone()
    };
    assert_eq!(status("inner.llb/In.vi"), "unchecked");
    assert_eq!(status("Top.vi"), "found");
    assert_eq!(status("Gone.vi"), "missing");
    assert_eq!(status("Top.vi/Deep.vi"), "missing");
}

/// What the directory says a member is does not decide whether it is
/// analyzed, so one changed type code cannot hide a VI.
#[test]
fn the_type_in_the_directory_does_not_hide_a_member() {
    let dir = TempDir::new("type");
    let inline = vi(&INLINE_CODE, &[]);
    let control = RsrcBuilder::new(file_type::CONTROL)
        .save_record(V23, &INLINE_CODE)
        .build();
    let inner = LlbBuilder::default().member("Deep.vi", &inline).build();
    dir.write(
        "odd.llb",
        &LlbBuilder::default()
            // The directory says archive. The name and the content say VI.
            .member_typed("x.vi", FourCc::new(b"LVAR"), &inline)
            .member_typed("y.vi", FourCc::new(b"TEXT"), &inline)
            // One member is a control by its type alone, and one by its
            // name alone.
            .member("Cluster typedef", &control)
            .member_typed("z.ctl", FourCc::new(b"????"), &control)
            // The directory says VI and the content is an archive. The
            // member is parsed as the resource file it is, and its members
            // are not opened.
            .member_typed("fake.vi", FourCc::new(b"LVIN"), &inner)
            // These two are real archives in the archive, one under each
            // type.
            .member("inner.llb", &inner)
            .member_typed("other.llb", FourCc::new(b"TEXT"), &inner)
            .build(),
    );
    let workspace = dir.scan();
    assert_eq!(
        dir.keys(&workspace),
        [
            "odd.llb",
            "odd.llb/Cluster typedef",
            "odd.llb/fake.vi",
            "odd.llb/x.vi",
            "odd.llb/y.vi",
            "odd.llb/z.ctl",
        ]
    );
    assert_eq!(
        dir.codes(),
        [
            ("odd.llb/Cluster typedef".to_owned(), "BZ001"),
            ("odd.llb/x.vi".to_owned(), "BZ001"),
            ("odd.llb/y.vi".to_owned(), "BZ001"),
            ("odd.llb/z.ctl".to_owned(), "BZ001"),
        ]
    );
    let kind = |name: &str| {
        let state = workspace.get(&dir.at("odd.llb").join(name)).unwrap();
        state.metadata().unwrap().kind
    };
    assert_eq!(kind("Cluster typedef"), FileKind::Control);
    assert_eq!(kind("fake.vi"), FileKind::Library);
}

fn library(urls: &[&str]) -> Vec<u8> {
    let mut xml = String::from("<Library LVVersion=\"23008000\">\n");
    for url in urls {
        let name = url.rsplit('/').next().unwrap_or(url);
        let _ = writeln!(xml, "<Item Name=\"{name}\" Type=\"VI\" URL=\"{url}\"/>");
    }
    xml.push_str("</Library>\n");
    xml.into_bytes()
}

/// A project library or class in an archive is a file of the workspace
/// like one on disk, with the same rules and the same links.
#[test]
fn project_files_in_an_archive_are_analyzed_like_files_on_disk() {
    let dir = TempDir::new("project-member");
    let lib = library(&["../Present.vi", "../Gone.vi", "../../Loose.vi"]);
    dir.write("loose.llb/Lib.lvlib", &lib);
    dir.write("loose.llb/Present.vi", &clean());
    dir.write("Loose.vi", &clean());
    dir.write(
        "packed.llb",
        &LlbBuilder::default()
            .member("Present.vi", &clean())
            .compressed("Lib.lvlib", &lib)
            .member("Bad.lvclass", b"<not xml")
            .build(),
    );
    let workspace = dir.scan();
    assert_eq!(
        dir.keys(&workspace),
        [
            "Loose.vi",
            "loose.llb/Lib.lvlib",
            "loose.llb/Present.vi",
            "packed.llb",
            "packed.llb/Bad.lvclass",
            "packed.llb/Lib.lvlib",
            "packed.llb/Present.vi",
        ]
    );
    let member = dir.at("packed.llb/Lib.lvlib");
    let on_disk = dir.at("loose.llb/Lib.lvlib");
    let urls = |path: &Path| -> Vec<String> {
        let project = workspace.get(path).unwrap().project().unwrap();
        project.members.iter().map(|m| m.url.clone()).collect()
    };
    assert_eq!(urls(&member), urls(&on_disk));
    assert_eq!(urls(&member).len(), 3);
    // The findings are those of the same file in a directory.
    let found = dir.diagnostics();
    let of = |prefix: &str| -> Vec<(String, &str, String)> {
        found
            .iter()
            .filter_map(|d| {
                let path = dir.rel(&d.path);
                let rest = path.strip_prefix(prefix)?.to_owned();
                (rest != "/Bad.lvclass" && !rest.is_empty())
                    .then(|| (rest, d.code, d.message.clone()))
            })
            .collect()
    };
    assert_eq!(of("packed.llb"), of("loose.llb"));
    assert_eq!(
        of("packed.llb"),
        [(
            "/Lib.lvlib".to_owned(),
            "BZ006",
            "member `Gone.vi` does not exist on disk (`../Gone.vi`)".to_owned()
        )]
    );
    assert!(found
        .iter()
        .any(|d| dir.rel(&d.path) == "packed.llb/Bad.lvclass" && d.code == "BZ000"));
    // The library contains its members across the archive boundary.
    let graph = DependencyGraph::build(&workspace);
    let report = deps::scan(&workspace, &graph, &Roots::default());
    let missing: Vec<String> = report
        .missing
        .iter()
        .map(|link| dir.rel(Path::new(&link.target)))
        .collect();
    assert_eq!(missing, ["loose.llb/Gone.vi", "packed.llb/Gone.vi"]);
    // Given by its path, the member is read as the project file it is.
    assert_eq!(
        FileState::load(&member).project().map(|p| p.members.len()),
        Some(3)
    );
    // It has no diagram, and `verify` neither lists nor reports it.
    let verified = Verified::scan(std::slice::from_ref(&dir.0));
    assert_eq!(verified.unreadable, []);
    assert_eq!(verified.files.len(), 0);
}

/// A member is found whatever the letter case it is asked for in, and is
/// one file under the name its archive has for it.
#[test]
fn a_member_asked_for_in_another_letter_case_is_the_same_file() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify");
    let machine = fs::read(fixtures.join("jki-state-machine/new-support-vis-examples.vi")).unwrap();
    let dir = TempDir::new("letter-case");
    dir.write(
        "lib/util.llb",
        &LlbBuilder::default()
            .member("Helper.vi", &vi(&INLINE_CODE, &["/Gone.vi"]))
            .member("Machine.vi", &machine)
            .build(),
    );
    let archive_path = dir.at("lib/util.llb");
    let typed = dir.at("lib/util.llb/helper.VI");
    assert_eq!(
        archive::canonical(&typed),
        Some(dir.at("lib/util.llb/Helper.vi"))
    );
    assert_eq!(archive::canonical(&dir.at("lib/util.llb/absent.vi")), None);
    assert_eq!(archive::canonical(&archive_path), None);
    for roots in [
        vec![typed.clone()],
        vec![typed.clone(), dir.at("lib/util.llb/HELPER.vi")],
    ] {
        let workspace = Workspace::scan(&roots);
        assert_eq!(
            dir.keys(&workspace),
            ["lib/util.llb/Helper.vi"],
            "{roots:?}"
        );
    }
    let both = [archive_path.clone(), typed.clone()];
    let workspace = Workspace::scan(&both);
    assert_eq!(
        dir.keys(&workspace),
        [
            "lib/util.llb",
            "lib/util.llb/Helper.vi",
            "lib/util.llb/Machine.vi"
        ]
    );
    let graph = DependencyGraph::build(&workspace);
    let found = lint(&workspace, &graph, &LintConfig::default());
    let on_helper = |d: &&Diagnostic| d.code == "BZ001" && d.path.ends_with("Helper.vi");
    assert_eq!(found.iter().filter(on_helper).count(), 1);
    assert!(found.iter().all(|d| !d.path.ends_with("helper.VI")));
    let report = deps::scan(&workspace, &graph, &Roots::default());
    assert_eq!(report.files, 3);
    let gone: Vec<_> = report
        .missing
        .iter()
        .filter(|link| link.target.ends_with("Gone.vi"))
        .collect();
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0].referrers.len(), 1);
    // The workspace finds the member under either spelling and returns
    // its own spelling.
    assert_eq!(
        workspace.stored_path(&typed),
        Some(dir.at("lib/util.llb/Helper.vi"))
    );
    assert!(workspace.get(&typed).unwrap().metadata().is_some());
    assert_eq!(
        workspace.stored_path(&dir.at("lib/util.llb/absent.vi")),
        None
    );
    // `verify` lists the machine once.
    let verified = Verified::scan(&[archive_path, dir.at("lib/util.llb/machine.vi")]);
    let files: Vec<String> = verified.files.iter().map(|f| dir.rel(&f.path)).collect();
    assert_eq!(files, ["lib/util.llb/Machine.vi"]);
    let alone = Verified::scan(&[dir.at("lib/util.llb/machine.vi")]);
    assert_eq!(alone.files.len(), 1);
    assert_eq!(alone.summary.findings, verified.summary.findings);
}

/// `verify` says which members of an archive it did not look at.
#[test]
fn verify_lists_an_archive_with_a_member_it_cannot_open() {
    let dir = TempDir::new("verify-problems");
    dir.write(
        "odd.llb",
        &LlbBuilder::default()
            .member("Fine.vi", &clean())
            .member("a/b.vi", &clean())
            .build(),
    );
    let verified = Verified::scan(std::slice::from_ref(&dir.0));
    let unreadable: Vec<(String, &str)> = verified
        .unreadable
        .iter()
        .map(|(path, reason)| (dir.rel(path), reason.as_str()))
        .collect();
    assert_eq!(
        unreadable,
        [(
            "odd.llb".to_owned(),
            "member `a/b.vi`: its name is not a file name"
        )]
    );
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
    // The listing keeps the order of the archive, and the files are sorted.
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
        // `file:///C:/x` has a slash in front of the drive.
        let path = if cfg!(windows) {
            path.trim_start_matches('/')
        } else {
            path.as_str()
        };
        assert!(Path::new(path).is_file(), "{path}");
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
    assert!(member["partialFingerprints"]["buzzardMember/v1"]
        .as_str()
        .unwrap()
        .ends_with("my lib.llb/In side.vi"));
    let loose = results
        .iter()
        .find(|r| uri(r).ends_with("Loose.vi"))
        .unwrap();
    assert!(loose["locations"][0].get("logicalLocations").is_none());
    assert!(loose.get("partialFingerprints").is_none());
    assert!(loose["message"]["text"]
        .as_str()
        .unwrap()
        .starts_with("compiled code"));
}

/// The findings of several members of one archive have one artifact and
/// no region, and a fingerprint tells them apart. The same holds when the
/// members were given by their own paths and the archive is not a file
/// of the workspace.
#[test]
fn sarif_tells_the_members_of_one_archive_apart() {
    let dir = &TempDir::new("sarif-members");
    dir.write(
        "many.llb",
        &LlbBuilder::default()
            .member("One.vi", &vi(&INLINE_CODE, &[]))
            .member("Two.vi", &vi(&INLINE_CODE, &[]))
            .member("Three.vi", &vi(&INLINE_CODE, &[]))
            .build(),
    );
    let mut counts = Vec::new();
    for roots in [
        vec![dir.at("many.llb")],
        vec![
            dir.at("many.llb/One.vi"),
            dir.at("many.llb/two.VI"),
            dir.at("many.llb/Three.vi"),
        ],
    ] {
        let workspace = Workspace::scan(&roots);
        let graph = DependencyGraph::build(&workspace);
        let diagnostics = lint(&workspace, &graph, &LintConfig::default());
        let sarif = buzzard_lint::sarif::to_sarif(&workspace, &diagnostics, "0");
        let results = sarif["runs"][0]["results"].as_array().unwrap();
        let mut prints = Vec::new();
        for result in results {
            let location = &result["locations"][0];
            let uri = location["physicalLocation"]["artifactLocation"]["uri"]
                .as_str()
                .unwrap();
            assert!(uri.ends_with("/many.llb"), "{roots:?}: {uri}");
            let name = location["logicalLocations"][0]["name"].as_str().unwrap();
            let text = result["message"]["text"].as_str().unwrap();
            assert!(
                text.starts_with(&format!("{name}: compiled code")),
                "{text}"
            );
            let print = result["partialFingerprints"]["buzzardMember/v1"]
                .as_str()
                .unwrap();
            assert!(print.starts_with("BZ001:"), "{print}");
            assert!(print.ends_with(&format!("many.llb/{name}")), "{print}");
            prints.push(print.to_owned());
        }
        prints.sort();
        prints.dedup();
        assert_eq!(prints.len(), results.len(), "{roots:?}");
        assert_eq!(
            sarif,
            buzzard_lint::sarif::to_sarif(&workspace, &diagnostics, "0")
        );
        counts.push(results.len());
    }
    assert_eq!(counts, [3, 3]);
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
    // The rules of `check` also report the same findings on the member.
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
