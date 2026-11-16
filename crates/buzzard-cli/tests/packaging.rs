//! What every crate must carry to be published: the licence text, a notice
//! that covers what the package holds and no more, and its own README.
//!
//! `cargo package` takes files from the crate's directory only, so each
//! crate has real copies (no symbolic links, which Windows checkouts lack).
//! These tests keep the copies from drifting apart.

use std::fs;
use std::path::PathBuf;

const CRATES: [&str; 6] = [
    "buzzard-rsrc",
    "buzzard-lint",
    "buzzard-ls",
    "buzzard-mcp",
    "buzzard-tdms",
    "buzzard-cli",
];

/// The one crate whose source holds third-party data (the LVKit table).
const WITH_LVKIT: &str = "buzzard-rsrc";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A text file, with the line endings a Windows checkout may have added
/// taken out.
fn read(relative: &str) -> String {
    let path = root().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .replace("\r\n", "\n")
}

/// The part of the root NOTICE from the line `start` up to the next section
/// heading (a line of `=` under a title), or to the end.
fn notice_section(notice: &str, title: &str) -> String {
    let start = notice
        .find(title)
        .unwrap_or_else(|| panic!("the root NOTICE has no section `{title}`"));
    let rest = &notice[start..];
    let lines: Vec<&str> = rest.lines().collect();
    let is_rule = |line: &str| line.len() > 3 && line.bytes().all(|b| b == b'=');
    let end = (2..lines.len())
        .find(|&i| is_rule(lines[i]))
        .map_or(lines.len(), |i| i - 1);
    lines[..end].join("\n").trim_end().to_owned()
}

#[test]
fn every_crate_carries_the_root_licence_unchanged() {
    let expected = read("LICENSE");
    assert!(expected.contains("Apache License"));
    for name in CRATES {
        assert!(
            read(&format!("crates/{name}/LICENSE")) == expected,
            "crates/{name}/LICENSE differs from the root LICENSE; copy the root file over it"
        );
    }
}

#[test]
fn every_notice_covers_what_its_package_holds_and_no_more() {
    let root_notice = read("NOTICE");
    let trademark = notice_section(&root_notice, "buzzard is not affiliated with");
    let lvkit = notice_section(&root_notice, "Built-in function names, from LVKit");
    assert!(lvkit.contains("Apache-2.0, section 4(d)"));
    assert!(!lvkit.contains("Zenodo"));

    for name in CRATES {
        let notice = read(&format!("crates/{name}/NOTICE"));
        assert!(
            notice.starts_with(&format!("{name}\nCopyright (c) ")),
            "crates/{name}/NOTICE starts with the crate's name and the copyright line"
        );
        assert!(
            notice.contains(&trademark),
            "crates/{name}/NOTICE repeats the trademark statement of the root NOTICE"
        );
        // The measurement file is a test fixture, and no package has tests.
        assert!(
            !notice.contains("Zenodo"),
            "crates/{name}/NOTICE names a fixture the package does not hold"
        );
        if name == WITH_LVKIT {
            assert!(
                notice.contains(&lvkit),
                "crates/{name}/NOTICE holds the LVKit section of the root NOTICE word for word"
            );
        } else {
            assert!(
                !notice.contains("LVKit"),
                "crates/{name}/NOTICE names LVKit, whose data is in {WITH_LVKIT} only"
            );
        }
    }
}

/// The package is `src` (and examples) with the three documents. A crate
/// that started to ship its tests would ship third-party fixtures without
/// their notices.
#[test]
fn every_package_is_its_source_and_its_documents() {
    for name in CRATES {
        let manifest = read(&format!("crates/{name}/Cargo.toml"));
        let include = manifest
            .lines()
            .find(|line| line.starts_with("include = "))
            .unwrap_or_else(|| panic!("crates/{name}/Cargo.toml has an `include` list"));
        // The leading slash matters: without it a pattern matches at any
        // depth, and `README.md` would take the READMEs of the fixtures.
        let mut patterns: Vec<&str> = include
            .trim_start_matches("include = [")
            .trim_end_matches(']')
            .split(", ")
            .map(|pattern| pattern.trim_matches('"'))
            .collect();
        patterns.retain(|pattern| *pattern != "/examples/**");
        assert_eq!(
            patterns,
            ["/src/**", "/README.md", "/LICENSE", "/NOTICE"],
            "{name}: `include` is the source and the three documents, each anchored with `/`"
        );
        assert!(
            manifest.contains("\nreadme = \"README.md\"\n"),
            "{name}: `readme` is the crate's own README"
        );
        let readme = read(&format!("crates/{name}/README.md"));
        assert!(readme.starts_with(&format!("# {name}\n")));
    }
}

/// The data in `primitives.rs` is why `buzzard-rsrc` has the longer notice;
/// if the table moves, the notices move with it.
#[test]
fn the_lvkit_table_is_where_the_notice_says() {
    let table = read(&format!(
        "crates/{WITH_LVKIT}/src/blocks/heap/primitives.rs"
    ));
    assert!(table.contains("LVKit"));
    assert!(read(&format!("crates/{WITH_LVKIT}/NOTICE")).contains("src/blocks/heap/primitives.rs"));
}
