use std::fmt::Write;

use super::*;
use buzzard_rsrc::PathKind;

/// Real `ParentClassLinkInfo` values with the saving version and the parent
/// they name. See `tests/fixtures/project/README.md` for where they come
/// from.
const REAL_LINKS: [(&str, u32, &[&str], &str); 5] = [
    (
        r#"%Q#!!!!!!!)!"1!&!!!-!%!!!@````]!!!!"!!%!!!!O!!!!!1N445]O<(:D<'&T=V"53$!!!!!7!!%!"!!!!V..4QN445]O<(:D<'&T=Q!!!!!"#,
        0x1300_8000,
        &["SMO.lvclass"],
        "../../SMO/SMO.lvclass",
    ),
    (
        r#"%Q#!!!!!!!)!"1!&!!!-!%!!!@````]!!!!"!!%!!!!U!!!!!1N445]O<(:D<'&T=V"53$!!!!!=!!%!"Q!!!!!$=X*D!V..4QN445]O<(:D<'&T=Q!!!!!"#,
        0x1300_8000,
        &["SMO.lvclass"],
        "../../../../src/SMO/SMO.lvclass",
    ),
    (
        r#"%Q#!!!!!!!)!"1!&!!!-!%!!!@````]!!!!"!!%!!!"?!!!!!2N445]O5W6M:F2F=GVJ<G&U;7ZH,GRW9WRB=X.16%AQ!!!!.A!"!!1!!".445]O5W6M:F2F=GVJ<G&U;7ZH'V..4SZ4:7RG6'6S<7FO982J<G=O<(:D<'&T=Q!!!!!"#,
        0x1300_8000,
        &["SMO.SelfTerminating.lvclass"],
        "../../SMO.SelfTerminating/SMO.SelfTerminating.lvclass",
    ),
    // The name has an even length here, so with its length byte a padding
    // byte is needed before the path.
    (
        r#"%Q#!!!!!!!)!"1!&!!!-!%!!!@````]!!!!"!!%!!!!_!!!!!1Z445]O65EO<(:D<'&T=Q"16%AQ!!!!)A!"!!=!!!!!!X.S9Q:445]O65E/5UV0,F6*,GRW9WRB=X-!!!!!"#,
        0x1300_8000,
        &["SMO.UI.lvclass"],
        "../../../../src/SMO.UI/SMO.UI.lvclass",
    ),
    // A parent that is in a library installed under <vilib>.
    (
        r#"'1#!!!!!!!)!"1!&!!!-!%!!!@````]!!!!"!!%!!!#$!!!!!BNH5F"$,8.F=H:J9W6S,8*F<'6B=W5O<(:M;7)45W6S>GFD:5*B=W5O<(:D<'&T=V"53$!!!!"(!!!!"1=]>GFM;7)_"'>35%-64'&C6EF&6S"H5F"$)&.F=H:J9W6S#V.F=H:J9W6#98.F%V.F=H:J9W6#98.F,GRW9WRB=X-!!!!!"#,
        0x1900_8000,
        &["gRPC-servicer-release.lvlib", "ServiceBase.lvclass"],
        "<vilib>/gRPC/LabVIEW gRPC Servicer/ServiceBase/ServiceBase.lvclass",
    ),
];

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/project")
        .join(rel)
}

fn class_xml(parent_link: &str) -> String {
    format!(
        "<?xml version='1.0' encoding='UTF-8'?>\n<LVClass LVVersion=\"19008000\">\n\t\
         <Property Name=\"{PARENT_LINK_PROPERTY}\" Type=\"Bin\">{}</Property>\n</LVClass>\n",
        parent_link.replace('&', "&amp;").replace('<', "&lt;")
    )
}

#[test]
fn decodes_real_parent_links() {
    for (text, version, names, link) in REAL_LINKS {
        let bytes = decode_bin(text).unwrap();
        assert_eq!(bytes[..4], version.to_be_bytes());
        let (qualified_name, path) = parse_parent_link(&bytes).unwrap();
        assert_eq!(qualified_name, names);
        assert_eq!(path.to_string(), link);
    }
}

#[test]
fn encoder_reproduces_real_values_exactly() {
    for (text, version, names, _) in REAL_LINKS {
        let bytes = decode_bin(text).unwrap();
        assert_eq!(encode_bin(&bytes), text);
        let (_, path) = parse_parent_link(&bytes).unwrap();
        assert_eq!(encode_parent_link(version, names, &path), text);
    }
}

#[test]
fn bin_encoding_round_trips_every_length() {
    let data: Vec<u8> = (0..=255u8).chain((0..=255u8).rev()).collect();
    for len in 0..data.len() {
        let text = encode_bin(&data[..len]);
        assert!(text.bytes().all(|b| (0x21..=0x60).contains(&b)));
        assert_eq!(decode_bin(&text).unwrap(), &data[..len]);
    }
}

#[test]
fn bin_decoder_rejects_foreign_characters_and_stray_bits() {
    assert_eq!(decode_bin(""), Some(Vec::new()));
    assert_eq!(decode_bin("!!"), Some(vec![0]));
    // Lowercase letters, space, and newline are outside the alphabet.
    for bad in ["a!", "! !", "!\n!", "!\u{e9}", "~~"] {
        assert_eq!(decode_bin(bad), None, "{bad:?}");
    }
    // Twelve bits make one byte. The four left over must be zero.
    assert_eq!(decode_bin("!\""), None);
}

#[test]
fn damaged_parent_links_never_panic_and_never_guess() {
    let (text, _, names, link) = REAL_LINKS[4];
    let bytes = decode_bin(text).unwrap();
    // The value ends with four bytes that follow the array.
    let needed = bytes.len() - 4;
    for len in 0..bytes.len() {
        let parsed = parse_parent_link(&bytes[..len]);
        if len < needed {
            assert_eq!(parsed, None, "prefix of {len} bytes");
        } else {
            let (qualified_name, path) = parsed.unwrap();
            assert_eq!(qualified_name, names);
            assert_eq!(path.to_string(), link);
        }
    }
    for at in 0..bytes.len() {
        for flip in [0x01, 0x80, 0xff] {
            let mut damaged = bytes.clone();
            damaged[at] ^= flip;
            let _ = parse_parent_link(&damaged);
        }
    }
    // A type description other than the one observed is not interpreted.
    let mut other_type = bytes.clone();
    other_type[8] ^= 1;
    assert_eq!(parse_parent_link(&other_type), None);
    // Truncating the text itself must not panic either.
    for len in 0..text.len() {
        let _ = parent_from_text(Path::new("a/B.lvclass"), &text[..len]);
    }
    let huge = "!".repeat(MAX_PARENT_LINK_CHARS + 1);
    assert_eq!(parent_from_text(Path::new("a/B.lvclass"), &huge), None);
}

#[test]
fn parses_real_library() {
    let path = fixture("jki-smo/URI.lvlib");
    let lib = ProjectFile::from_path(&path).unwrap();
    assert_eq!(lib.kind, ProjectKind::Library);
    assert_eq!(lib.name, "URI.lvlib");
    assert_eq!(lib.lv_version.as_deref(), Some("13.0.0"));
    assert_eq!(lib.parent_class, None);
    let names: Vec<&str> = lib.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Construct URI.vi",
            "Parse URI.vi",
            "EscapeURI.vi",
            "UnescapeURI.vi"
        ]
    );
    let first = &lib.members[0];
    assert_eq!(first.item_type, "VI");
    assert_eq!(first.url, "../Construct URI.vi");
    assert_eq!(
        first.path.as_deref(),
        Some(normalize(&fixture("jki-smo/Construct URI.vi")).as_path())
    );
    assert!(lib.members.iter().all(|m| !m.dependency));
}

#[test]
fn parses_real_class_with_parent() {
    let path = fixture("jki-smo/SMO.SelfTerminating.lvclass");
    let class = ProjectFile::from_path(&path).unwrap();
    assert_eq!(class.kind, ProjectKind::Class);
    assert_eq!(class.lv_version.as_deref(), Some("13.0.0"));
    let parent = class.parent_class.as_ref().unwrap();
    assert_eq!(parent.name, "SMO.lvclass");
    assert_eq!(parent.qualified_name, ["SMO.lvclass"]);
    assert_eq!(parent.link.kind, PathKind::Relative);
    assert_eq!(parent.link.to_string(), "../../../../src/SMO/SMO.lvclass");
    assert_eq!(
        parent.path.as_deref(),
        Some(normalize(&fixture("jki-smo/../../../src/SMO/SMO.lvclass")).as_path())
    );
    // The private data control is stored inside the class file.
    let urls: Vec<(&str, &str, bool)> = class
        .members
        .iter()
        .map(|m| (m.item_type.as_str(), m.url.as_str(), m.path.is_some()))
        .collect();
    assert_eq!(
        urls,
        [
            ("Class Private Data", "SMO.SelfTerminating.ctl", false),
            ("VI", "../Process.vi", true)
        ]
    );
}

#[test]
fn parses_real_project_and_marks_dependencies() {
    let path = fixture("grpc-labview/helloworld.lvproj");
    let project = ProjectFile::from_path(&path).unwrap();
    assert_eq!(project.kind, ProjectKind::Project);
    assert_eq!(project.lv_version.as_deref(), Some("19.0.0"));
    let own: Vec<&Member> = project.members.iter().filter(|m| !m.dependency).collect();
    assert_eq!(own.len(), 2);
    assert_eq!(own[0].item_type, "Library");
    assert_eq!(own[0].url, "../helloworld_client/helloworld_client.lvlib");
    let expected = fixture("grpc-labview/helloworld_client/helloworld_client.lvlib");
    assert_eq!(own[0].path.as_deref(), Some(normalize(&expected).as_path()));
    let dependencies: Vec<&Member> = project.members.iter().filter(|m| m.dependency).collect();
    assert_eq!(dependencies.len(), 7);
    assert!(dependencies
        .iter()
        .all(|m| m.url.starts_with("/<vilib>/") && m.path.is_none()));
}

#[test]
fn parent_link_resolves_against_the_class_file() {
    for (text, _, names, _) in REAL_LINKS {
        let file = Path::new("repo/tests/support/Child/Child.lvclass");
        let class = ProjectFile::parse(file, &class_xml(text)).unwrap();
        let parent = class.parent_class.unwrap();
        assert_eq!(parent.qualified_name, names);
        assert_eq!(parent.name, *names.last().unwrap());
    }
    let file = Path::new("repo/src/Child/Child.lvclass");
    let class = ProjectFile::parse(file, &class_xml(REAL_LINKS[0].0)).unwrap();
    assert_eq!(
        class.parent_class.unwrap().path.as_deref(),
        Some(Path::new("repo/src/SMO/SMO.lvclass"))
    );
    // <vilib> is a location inside the installation, not on disk here.
    let class = ProjectFile::parse(file, &class_xml(REAL_LINKS[4].0)).unwrap();
    assert_eq!(class.parent_class.unwrap().path, None);
    // An undecodable value means "unknown", not an error.
    let class = ProjectFile::parse(file, &class_xml("not the encoding")).unwrap();
    assert_eq!(class.parent_class, None);
}

#[test]
fn resolves_urls_against_the_file_itself() {
    let file = Path::new("proj/lib/My.lvlib");
    let resolved = |url: &str| resolve_url(file, url);
    assert_eq!(resolved("../a.vi").unwrap(), Path::new("proj/lib/a.vi"));
    assert_eq!(
        resolved("../sub/b.vi").unwrap(),
        Path::new("proj/lib/sub/b.vi")
    );
    assert_eq!(resolved("../../c.vi").unwrap(), Path::new("proj/c.vi"));
    assert_eq!(resolved("../../../../d.vi").unwrap(), Path::new("../d.vi"));
    assert_eq!(resolved(".././e.vi").unwrap(), Path::new("proj/lib/e.vi"));
    // Installation roots and other absolute URLs are left alone.
    assert_eq!(resolved("/<vilib>/Utility/error.llb/x.vi"), None);
    assert_eq!(resolved("/<userlib>/x.vi"), None);
    assert_eq!(resolved("/<instrlib>/x.vi"), None);
    assert_eq!(resolved("/<resource>/x.vi"), None);
    assert_eq!(resolved("/C/Users/me/x.vi"), None);
    assert_eq!(resolved("C:/Users/me/x.vi"), None);
    assert_eq!(resolved("..\\x.vi"), None);
    // Stored inside the XML file.
    assert_eq!(resolved("My.ctl"), None);
    assert_eq!(resolved("x/../My.ctl"), None);
    assert_eq!(resolved(""), None);
    assert_eq!(resolved("../My.lvlib/inner.vi"), None);
}

#[test]
fn collects_nested_items_in_document_order() {
    let xml = r#"<Project Type="Project" LVVersion="23008000">
        <Item Name="My Computer" Type="My Computer">
            <Item Name="src" Type="Folder" URL="../src">
                <Property Name="NI.DISK" Type="Bool">true</Property>
                <Item Name="Main.vi" Type="VI" URL="../src/Main.vi"/>
            </Item>
            <Item Name="A &amp; B.lvlib" Type="Library" URL="../A &amp; B.lvlib"/>
            <Item Name="Dependencies" Type="Dependencies">
                <Item Name="vi.lib" Type="Folder">
                    <Item Name="x.vi" Type="VI" URL="/&lt;vilib&gt;/x.vi"/>
                </Item>
                <Item Name="stale.vi" Type="VI" URL="../gone/stale.vi"/>
            </Item>
            <Item Name="Build Specifications" Type="Build">
                <Item Name="late.vi" Type="VI" URL="../late.vi"/>
            </Item>
        </Item>
    </Project>"#;
    let project = ProjectFile::parse(Path::new("p/App.lvproj"), xml).unwrap();
    assert_eq!(project.lv_version.as_deref(), Some("23.0.0"));
    let seen: Vec<(&str, &str, Option<&str>, bool)> = project
        .members
        .iter()
        .map(|m| {
            (
                m.name.as_str(),
                m.item_type.as_str(),
                m.path.as_deref().and_then(Path::to_str),
                m.dependency,
            )
        })
        .collect();
    let sep = std::path::MAIN_SEPARATOR_STR;
    let native = |p: &str| p.replace('/', sep);
    let expected = [
        ("src", "Folder", Some(native("p/src")), false),
        ("Main.vi", "VI", Some(native("p/src/Main.vi")), false),
        (
            "A & B.lvlib",
            "Library",
            Some(native("p/A & B.lvlib")),
            false,
        ),
        ("x.vi", "VI", None, true),
        ("stale.vi", "VI", Some(native("p/gone/stale.vi")), true),
        ("late.vi", "VI", Some(native("p/late.vi")), false),
    ];
    assert_eq!(seen.len(), expected.len());
    for (got, want) in seen.iter().zip(&expected) {
        assert_eq!((got.0, got.1, got.3), (want.0, want.1, want.3));
        assert_eq!(got.2, want.2.as_deref());
    }
}

#[test]
fn accepts_minimal_and_unusual_but_valid_files() {
    let empty = ProjectFile::parse(Path::new("App.lvproj"), "<Project/>").unwrap();
    assert_eq!(empty.kind, ProjectKind::Project);
    assert_eq!((empty.lv_version, empty.members.len()), (None, 0));
    // The kind follows the extension. A class file with a library root is
    // still listed as a class.
    let class = ProjectFile::parse(Path::new("M.LVCLASS"), "<Library/>").unwrap();
    assert_eq!(class.kind, ProjectKind::Class);
    // A byte order mark, as real files have.
    let bom = ProjectFile::parse(Path::new("L.lvlib"), "\u{feff}<Library LVVersion='x'/>");
    assert_eq!(bom.unwrap().lv_version.as_deref(), Some("x"));
    // Bytes that are not UTF-8 are read as Latin-1 instead of failing.
    let latin1 = b"<Library><Item Name='Caf\xe9.vi' Type='VI' URL='../Caf\xe9.vi'/></Library>";
    let lib = ProjectFile::parse_bytes(Path::new("d/L.lvlib"), latin1).unwrap();
    assert_eq!(lib.members[0].name, "Caf\u{e9}.vi");
    assert_eq!(
        lib.members[0].path.as_deref(),
        Some(Path::new("d").join("Caf\u{e9}.vi").as_path())
    );
}

#[test]
fn rejects_malformed_xml_with_a_reason() {
    let path = Path::new("App.lvproj");
    for (text, expected) in [
        ("", "invalid XML"),
        ("not xml at all", "invalid XML"),
        ("<Project>", "invalid XML"),
        ("<Project></Library>", "invalid XML"),
        (
            "<Project><Item Name='a' Name='b'/></Project>",
            "invalid XML",
        ),
        ("<Project>&undefined;</Project>", "invalid XML"),
        ("<html/>", "unexpected root element <html>"),
    ] {
        let err = ProjectFile::parse(path, text).unwrap_err();
        assert!(err.contains(expected), "{text:?}: {err}");
    }
    let err = ProjectFile::parse(Path::new("a.vi"), "<Project/>").unwrap_err();
    assert!(err.contains("not a .lvproj"), "{err}");
    assert!(ProjectFile::from_path(Path::new("no/such/file.lvproj")).is_err());
}

#[test]
fn truncated_real_files_are_errors_not_panics() {
    for rel in [
        "jki-smo/URI.lvlib",
        "jki-smo/SMO.SelfTerminating.lvclass",
        "grpc-labview/helloworld.lvproj",
    ] {
        let path = fixture(rel);
        let bytes = std::fs::read(&path).unwrap();
        let whole = ProjectFile::parse_bytes(&path, &bytes).unwrap();
        // Everything after the root's closing tag is trailing whitespace.
        let end = bytes.iter().rposition(|&b| b == b'>').unwrap();
        let step = (bytes.len() / 400).max(1);
        for len in (0..end).step_by(step).chain(end.saturating_sub(40)..end) {
            assert!(
                ProjectFile::parse_bytes(&path, &bytes[..len]).is_err(),
                "{rel} cut to {len} bytes"
            );
        }
        assert_eq!(
            ProjectFile::parse_bytes(&path, &bytes[..=end]).unwrap(),
            whole
        );
        // Corrupt single bytes. Any outcome but a panic is acceptable.
        for at in (0..bytes.len()).step_by(step) {
            let mut damaged = bytes.clone();
            damaged[at] = b'<';
            let _ = ProjectFile::parse_bytes(&path, &damaged);
            damaged[at] = 0xff;
            let _ = ProjectFile::parse_bytes(&path, &damaged);
        }
    }
}

#[test]
fn entity_expansion_bombs_are_rejected_immediately() {
    let mut bomb = String::from("<?xml version=\"1.0\"?>\n<!DOCTYPE lolz [\n");
    bomb.push_str(" <!ENTITY lol0 \"lollollollollollollollollollol\">\n");
    for i in 1..10 {
        let prev = format!("&lol{};", i - 1).repeat(10);
        let _ = writeln!(bomb, " <!ENTITY lol{i} \"{prev}\">");
    }
    bomb.push_str("]>\n<Project><Item Name=\"&lol9;\" URL=\"&lol9;\"/>&lol9;</Project>");
    let started = std::time::Instant::now();
    let err = ProjectFile::parse(Path::new("App.lvproj"), &bomb).unwrap_err();
    assert!(err.contains("invalid XML"), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));

    // An external entity is never fetched, because the DTD is refused.
    let external = "<!DOCTYPE p [<!ENTITY x SYSTEM \"file:///etc/passwd\">]>\
                    <Project><Item Name=\"a\" URL=\"&x;\"/></Project>";
    assert!(ProjectFile::parse(Path::new("App.lvproj"), external).is_err());
}

#[test]
fn deep_nesting_does_not_overflow_the_stack() {
    const DEPTH: usize = 200_000;
    let mut xml = String::from("<Project>");
    for _ in 0..DEPTH {
        xml.push_str("<Item Type=\"Dependencies\" URL=\"../a.vi\">");
    }
    for _ in 0..DEPTH {
        xml.push_str("</Item>");
    }
    xml.push_str("</Project>");
    let started = std::time::Instant::now();
    let project = ProjectFile::parse(Path::new("p/App.lvproj"), &xml).unwrap();
    assert_eq!(project.members.len(), DEPTH);
    assert!(project.members.iter().all(|m| m.dependency));
    assert!(started.elapsed() < std::time::Duration::from_secs(20));

    // Unclosed nesting is an error, not a crash.
    let open = "<Item>".repeat(DEPTH);
    assert!(ProjectFile::parse(Path::new("App.lvproj"), &format!("<Project>{open}")).is_err());
}

#[test]
fn node_limit_bounds_memory() {
    let mut xml = String::from("<Library>");
    for _ in 0..=MAX_NODES {
        xml.push_str("<a/>");
    }
    xml.push_str("</Library>");
    let err = ProjectFile::parse(Path::new("L.lvlib"), &xml).unwrap_err();
    assert!(err.contains("nodes limit"), "{err}");
}

#[test]
fn output_is_identical_across_runs_and_serializes() {
    let path = fixture("jki-smo/SMO.SelfTerminating.lvclass");
    let first = ProjectFile::from_path(&path).unwrap();
    for _ in 0..3 {
        assert_eq!(ProjectFile::from_path(&path).unwrap(), first);
    }
    let json = serde_json::to_value(&first).unwrap();
    assert_eq!(json["kind"], "class");
    assert_eq!(json["parent_class"]["name"], "SMO.lvclass");
    assert_eq!(json["parent_class"]["link"]["kind"], "relative");
    assert_eq!(json["members"][1]["url"], "../Process.vi");
    assert_eq!(json["members"][0]["path"], serde_json::Value::Null);
}
