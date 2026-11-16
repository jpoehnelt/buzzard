//! The semantic diff on real files: fixture pairs that differ by one known
//! change, and pairs of consecutive git revisions.

use std::fs;
use std::path::{Path, PathBuf};

use buzzard_lint::diff::{diff, to_markdown, to_text, DiffOptions};
use buzzard_lint::Inspection;

fn pylavi(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../buzzard-rsrc/tests/fixtures/pylavi")
        .join(name)
}

fn revisions(stem: &str) -> (Inspection, Inspection) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/diff");
    let load = |side: &str| {
        Inspection::from_path(&dir.join(format!("{stem}.{side}.vi"))).expect("read the revision")
    };
    (load("old"), load("new"))
}

fn text(old: &str, new: &str) -> String {
    let old = Inspection::from_path(&pylavi(old)).expect("read the old fixture");
    let new = Inspection::from_path(&pylavi(new)).expect("read the new fixture");
    to_text(&diff(Some(&old), Some(&new), DiffOptions::default()))
}

#[test]
fn fixtures_that_differ_by_one_setting() {
    for (new, change) in [
        (
            "empty_separate_code.vi",
            "compiled code separated off -> on",
        ),
        ("empty_no_auto_err.vi", "automatic error handling on -> off"),
        ("empty_no_debugging.vi", "debugging on -> off"),
        ("empty_locked.vi", "locked off -> on"),
        ("empty_run_on_open.vi", "run when opened off -> on"),
        (
            "empty_suspend_when_called.vi",
            "suspend when called off -> on",
        ),
        (
            "empty_clear_indicators.vi",
            "clear indicators when called off -> on",
        ),
    ] {
        // Each was saved under its own name, which the file records.
        let saved_name = if new == "empty_separate_code.vi" {
            "empty copy.vi"
        } else {
            new
        };
        assert_eq!(
            text("empty.vi", new),
            format!(
                "2 settings changed\n\
                 changed: saved name `empty.vi` -> `{saved_name}`\n\
                 changed: {change}\n"
            ),
            "{new}"
        );
    }
    assert_eq!(
        text("empty.vi", "empty_dev.vi"),
        "1 setting changed\nchanged: saved version 21.0.0 -> 21.0.0d0\n"
    );
    assert_eq!(
        text("empty.vi", "empty_password.vi"),
        "3 settings changed\n\
         changed: saved name `empty.vi` -> `empty_password.vi`\n\
         changed: locked off -> on\n\
         changed: password protection off -> on\n"
    );
    assert_eq!(
        text(
            "reentrancy/23.0/Non-Reentrant.vi",
            "reentrancy/26.0/Shared-Reentrant.vi"
        ),
        "3 settings changed\n\
         changed: saved name `Non-Reentrant.vi` -> `Shared-Reentrant.vi`\n\
         changed: saved version 23.0.0 -> 26.3.0\n\
         changed: reentrancy non-reentrant -> shared clones\n"
    );
}

#[test]
fn breakpoints_are_a_setting_and_leave_the_diagram_alone() {
    assert_eq!(
        text("add.vi", "add_breakpoints.vi"),
        "3 settings changed\n\
         changed: saved name `add.vi` -> `add_breakpoints.vi`\n\
         changed: breakpoints off -> on\n\
         changed: breakpoint count 0 -> 3\n"
    );
}

#[test]
fn fixtures_that_differ_in_their_diagram() {
    assert_eq!(
        text("empty.vi", "add.vi"),
        "1 node added, 1 changed; 3 wires added; 1 setting changed\n\
         changed: saved name `empty.vi` -> `add.vi`\n\
         added: built-in function `Add` #43\n\
         changed: diagram boundary #7: terminals 0 -> 3 (+3 numeric)\n\
         added: numeric wire from built-in function `Add` #43 to diagram boundary #7\n\
         added: numeric wire from diagram boundary #7 to built-in function `Add` #43\n\
         added: numeric wire from diagram boundary #7 to built-in function `Add` #43\n"
    );
    assert_eq!(
        text("add.vi", "empty.vi"),
        "1 node removed, 1 changed; 3 wires removed; 1 setting changed\n\
         changed: saved name `add.vi` -> `empty.vi`\n\
         removed: built-in function `Add` #43\n\
         changed: diagram boundary #7: terminals 3 -> 0 (-3 numeric)\n\
         removed: numeric wire from built-in function `Add` #43 to diagram boundary #7\n\
         removed: numeric wire from diagram boundary #7 to built-in function `Add` #43\n\
         removed: numeric wire from diagram boundary #7 to built-in function `Add` #43\n"
    );
    assert_eq!(
        text("empty.vi", "relative_link.vi"),
        "1 node added; 1 setting changed; 1 dependency added\n\
         changed: saved name `empty.vi` -> `relative_link.vi`\n\
         added: dependency `../empty.vi`\n\
         added: subVI `empty.vi` #43\n"
    );
}

#[test]
fn consecutive_revisions_of_a_vi_tree() {
    let (old, new) = revisions("jki-state-machine/vi-tree");
    let result = diff(Some(&old), Some(&new), DiffOptions::default());
    assert_eq!(
        to_text(&result),
        "1 node added, 1 removed, 2 moved; 2 wires added, 2 removed, 2 changed\n\
         added: subVI `Conditional State String.vi` #42\n\
         removed: subVI `Conditional State String.vi` #224\n\
         moved: subVI `Build State String with Arguments.vi` #178 (484,192 -> 524,192)\n\
         moved: subVI `Periodic State String.vi` #180 (486,314 -> 528,306)\n\
         added: boolean wire from diagram boundary #7 to \
         subVI `Conditional State String.vi` #42\n\
         added: string wire from diagram boundary #7 to \
         subVI `Conditional State String.vi` #42\n\
         removed: boolean wire from diagram boundary #7 to \
         subVI `Conditional State String.vi` #224\n\
         removed: string wire from diagram boundary #7 to \
         subVI `Conditional State String.vi` #224\n\
         changed: numeric wire from diagram boundary #7 to \
         subVI `Periodic State String.vi` #180: attached to a different terminal\n\
         changed: string wire from diagram boundary #7 to \
         subVI `Periodic State String.vi` #180: attached to a different terminal\n"
    );
    let ignoring = diff(Some(&old), Some(&new), DiffOptions { ignore_moves: true });
    assert_eq!(ignoring.summary.moves_ignored, 2);
    assert!(
        ignoring.nodes.moved.is_empty(),
        "expected nothing, got {:?}",
        ignoring.nodes.moved
    );
    assert_eq!(ignoring.nodes.added, result.nodes.added);
    assert_eq!(ignoring.wires, result.wires);
}

#[test]
fn consecutive_revisions_with_a_new_control() {
    let (old, new) = revisions("grpc-labview/register-reflection-descriptors");
    assert_eq!(
        to_markdown(&diff(Some(&old), Some(&new), DiffOptions::default())),
        "**1 node changed; 1 wire added; 1 dependency added**\n\
         \n\
         #### Dependencies\n\
         \n\
         - added: dependency `../../../Shared/grpcId.ctl`\n\
         \n\
         #### Block diagram\n\
         \n\
         - changed: diagram boundary #7: terminals 3 -> 5 (+2 other)\n\
         - added: wire from diagram boundary #7 to diagram boundary #7\n"
    );
}

fn every_fixture() -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![
        pylavi(""),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/diff"),
    ];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(dir).expect("list the fixture dir") {
            let path = entry.expect("read a fixture dir entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "vi") {
                files.push(path);
            }
        }
    }
    files.sort();
    assert!(files.len() >= 30, "{}", files.len());
    files
}

#[test]
fn a_file_never_differs_from_itself_and_output_is_deterministic() {
    let files: Vec<Inspection> = every_fixture()
        .iter()
        .map(|path| Inspection::from_path(path).unwrap())
        .collect();
    for file in &files {
        let same = diff(Some(file), Some(file), DiffOptions::default());
        assert!(same.identical);
        assert_eq!(to_text(&same), "no differences\n");
    }
    // Every file against every other, twice.
    for old in &files {
        for new in &files {
            let first = diff(Some(old), Some(new), DiffOptions::default());
            let second = diff(Some(old), Some(new), DiffOptions::default());
            assert_eq!(first, second);
            assert_eq!(to_text(&first), to_text(&second));
        }
    }
}

#[test]
fn damaged_files_never_panic() {
    let (old, _) = revisions("jki-state-machine/vi-tree");
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/diff/jki-state-machine/vi-tree.new.vi");
    let bytes = fs::read(path).unwrap();
    let mut parsed = 0;
    let mut check = |damaged: &[u8]| {
        if let Ok(new) = Inspection::from_bytes(damaged) {
            parsed += 1;
            for (a, b) in [(&old, &new), (&new, &old), (&new, &new)] {
                let result = diff(Some(a), Some(b), DiffOptions::default());
                let _ = (to_text(&result), to_markdown(&result));
            }
        }
    };
    // One byte changed at a time, across the whole file.
    for at in (0..bytes.len()).step_by(7) {
        let mut damaged = bytes.clone();
        damaged[at] ^= 0xa5;
        check(&damaged);
        damaged[at] = 0xff;
        check(&damaged);
    }
    for len in (0..bytes.len()).step_by(97) {
        check(&bytes[..len]);
    }
    // Most single-byte changes leave a file that still parses.
    assert!(parsed > 1000, "{parsed}");
}
