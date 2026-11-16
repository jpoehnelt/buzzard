//! Conformance suite: every real binary under `tests/fixtures` is parsed and
//! compared against a golden JSON snapshot in `tests/snapshots`.
//!
//! To add coverage for a new editor version or file kind, drop the file into
//! `tests/fixtures/<source>/` and run:
//!
//! ```bash
//! BUZZARD_BLESS=1 cargo test -p buzzard-rsrc --test conformance
//! ```
//!
//! then review the generated snapshot before committing it.

use std::fs;
use std::path::{Path, PathBuf};

use buzzard_rsrc::blocks::zlib;
use buzzard_rsrc::{
    BlockDiagram, DataType, Direction, FileKind, Reentrancy, RsrcFile, Stage, ViMetadata,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn fixtures() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e != "md") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&root().join("fixtures"), &mut out);
    out.sort();
    assert!(
        out.len() >= 30,
        "fixtures went missing: found {}",
        out.len()
    );
    out
}

fn fixture(name: &str) -> ViMetadata {
    ViMetadata::from_path(root().join("fixtures").join(name)).unwrap()
}

/// Compare `actual` with a snapshot file, or rewrite the file when blessing.
fn check(snapshot: &Path, actual: String, failures: &mut Vec<String>) {
    if std::env::var_os("BUZZARD_BLESS").is_some() {
        fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        fs::write(snapshot, actual).unwrap();
    } else if fs::read_to_string(snapshot).ok().as_deref() != Some(actual.as_str()) {
        failures.push(snapshot.display().to_string());
    }
}

#[test]
fn snapshots_match() {
    let mut failures = Vec::new();
    for path in fixtures() {
        let rel = path.strip_prefix(root().join("fixtures")).unwrap();
        let snapshot = root()
            .join("snapshots")
            .join(format!("{}.json", rel.display()));
        let meta =
            ViMetadata::from_path(&path).unwrap_or_else(|e| panic!("{}: {e}", rel.display()));
        let actual = serde_json::to_string_pretty(&meta).unwrap() + "\n";
        check(&snapshot, actual, &mut failures);

        // The block diagram, where the file has one, gets its own snapshot.
        let bytes = fs::read(&path).unwrap();
        let file = RsrcFile::parse(&bytes).unwrap();
        let diagram = BlockDiagram::from_rsrc(&file)
            .unwrap_or_else(|e| panic!("{}: diagram: {e}", rel.display()));
        if let Some(diagram) = diagram {
            let snapshot = root()
                .join("snapshots")
                .join(format!("{}.diagram.json", rel.display()));
            let actual = serde_json::to_string_pretty(&diagram).unwrap() + "\n";
            check(&snapshot, actual, &mut failures);
        }
    }
    assert!(
        failures.is_empty(),
        "snapshot mismatch (re-run with BUZZARD_BLESS=1 and review the diff): {failures:#?}"
    );
}

#[test]
fn parsing_is_deterministic() {
    for path in fixtures() {
        let bytes = fs::read(&path).unwrap();
        let a = ViMetadata::from_bytes(&bytes).unwrap();
        let b = ViMetadata::from_bytes(&bytes).unwrap();
        assert_eq!(a, b, "{}", path.display());
    }
}

/// Every resource payload must lie inside the data section, and together the
/// directory must account for every type the file declares.
#[test]
fn directory_is_consistent() {
    for path in fixtures() {
        let bytes = fs::read(&path).unwrap();
        let file = RsrcFile::parse(&bytes).unwrap();
        let data_start = file.header.data_offset as usize;
        let data_end = data_start + file.header.data_size as usize;
        let base = bytes.as_ptr() as usize;
        for r in file.resources() {
            let start = r.data.as_ptr() as usize - base;
            assert!(
                start >= data_start && start + r.data.len() <= data_end,
                "{}: {} #{} escapes the data section",
                path.display(),
                r.ty,
                r.id
            );
        }
    }
}

#[test]
fn compressed_blocks_inflate() {
    let mut inflated = 0;
    for path in fixtures() {
        let bytes = fs::read(&path).unwrap();
        let file = RsrcFile::parse(&bytes).unwrap();
        for ty in [b"BDHb", b"FPHb", b"VCTP"] {
            for r in file.of_type(ty) {
                zlib::decompress(r.data)
                    .unwrap_or_else(|e| panic!("{}: {}: {e}", path.display(), r.ty));
                inflated += 1;
            }
        }
    }
    assert!(
        inflated >= 60,
        "only {inflated} compressed blocks were checked"
    );
}

/// Truncated or corrupted input must produce an error or a value, never a
/// panic, hang or out-of-bounds read.
#[test]
fn hostile_input_never_panics() {
    for path in fixtures() {
        let bytes = fs::read(&path).unwrap();
        let analyze = |bytes: &[u8]| {
            let _ = ViMetadata::from_bytes(bytes);
            if let Ok(file) = RsrcFile::parse(bytes) {
                let _ = BlockDiagram::from_rsrc(&file);
            }
        };
        for len in (0..bytes.len()).step_by(7) {
            analyze(&bytes[..len]);
        }
        // Deterministic corruption: overwrite each byte of the directory and
        // a sample of the data section with hostile values.
        let info = u32::from_be_bytes(bytes[16..20].try_into().unwrap()) as usize;
        let positions = (0..64)
            .chain((64..info).step_by(13))
            .chain(info..bytes.len());
        for pos in positions {
            for value in [0x00, 0x7f, 0xff] {
                let mut copy = bytes.clone();
                copy[pos] = value;
                analyze(&copy);
            }
        }
    }
}

/// Corrupt the *decompressed* heap and type table, which the test above
/// cannot reach through the zlib checksum.
#[test]
fn hostile_heap_and_types_never_panic() {
    use buzzard_rsrc::blocks::heap::Heap;
    use buzzard_rsrc::blocks::types::TypeTable;
    use std::collections::HashMap;

    for name in ["pylavi/add.vi", "pylavi/absolute_link_PTH.vi"] {
        let bytes = fs::read(root().join("fixtures").join(name)).unwrap();
        let file = RsrcFile::parse(&bytes).unwrap();
        let heap = zlib::decompress(file.first(b"BDHb").unwrap()).unwrap();
        let vctp = zlib::decompress(file.first(b"VCTP").unwrap()).unwrap();
        let dthp = file.first(b"DTHP");
        let types = TypeTable::parse(&vctp, dthp).unwrap();
        let names = HashMap::new();

        for len in 0..heap.len() {
            if let Ok(parsed) = Heap::parse(&heap[..len]) {
                let _ = BlockDiagram::from_heap(&parsed, &types, &names);
            }
        }
        for pos in 0..heap.len() {
            for value in [0x00, 0x13, 0x7f, 0xff] {
                let mut copy = heap.clone();
                copy[pos] = value;
                if let Ok(parsed) = Heap::parse(&copy) {
                    let _ = BlockDiagram::from_heap(&parsed, &types, &names);
                }
            }
        }
        for len in 0..vctp.len() {
            let _ = TypeTable::parse(&vctp[..len], dthp);
        }
        for pos in 0..vctp.len() {
            for value in [0x00, 0x7f, 0xff] {
                let mut copy = vctp.clone();
                copy[pos] = value;
                if let Ok(table) = TypeTable::parse(&copy, dthp) {
                    for id in 0..64 {
                        let _ = table.heap_type(id);
                    }
                }
            }
        }
    }
}

fn diagram(name: &str) -> BlockDiagram {
    let bytes = fs::read(root().join("fixtures").join(name)).unwrap();
    let file = RsrcFile::parse(&bytes).unwrap();
    BlockDiagram::from_rsrc(&file).unwrap().unwrap()
}

/// `add.vi`: two numeric controls wired into an Add primitive, whose
/// output is wired to an indicator.
#[test]
fn diagram_of_add_vi() {
    let bd = diagram("pylavi/add.vi");
    let classes: Vec<&str> = bd.root.nodes.iter().map(|n| n.class.as_str()).collect();
    assert_eq!(classes, ["sRN", "prim"]);
    assert_eq!(bd.root.wires.len(), 3);

    let boundary = &bd.root.nodes[0];
    let add = &bd.root.nodes[1];
    assert_eq!(add.terminals.len(), 3);
    assert!(add
        .terminals
        .iter()
        .all(|t| t.wired && t.direction_is_reliable()));
    assert!(add
        .terminals
        .iter()
        .all(|t| t.data_type == Some(DataType::Numeric)));
    let directions: Vec<Direction> = add.terminals.iter().map(|t| t.direction).collect();
    assert_eq!(
        directions,
        [Direction::Output, Direction::Input, Direction::Input]
    );

    // Every wire joins a front-panel terminal and a terminal of the primitive.
    let on = |node: &buzzard_rsrc::Node, uid: i32| node.terminals.iter().any(|t| t.uid == uid);
    for wire in &bd.root.wires {
        assert_eq!(wire.sinks.len(), 1);
        let (a, b) = (wire.source, wire.sinks[0]);
        assert!((on(boundary, a) && on(add, b)) || (on(add, a) && on(boundary, b)));
    }
    // Two controls feed the diagram, one indicator is fed by it.
    let outputs = |n: &buzzard_rsrc::Node| {
        n.terminals
            .iter()
            .filter(|t| t.direction == Direction::Output)
            .count()
    };
    assert_eq!((outputs(boundary), boundary.terminals.len()), (2, 3));
    // Their types come from the front-panel heap.
    for terminal in &boundary.terminals {
        assert_eq!(terminal.data_type, Some(DataType::Numeric));
        assert_eq!(terminal.dco_class.as_deref(), Some("fPDCO"));
    }
}

#[test]
fn diagram_names_subvi_calls() {
    let bd = diagram("pylavi/relative_link.vi");
    let call = bd.nodes().find(|n| n.class == "iUse").unwrap();
    assert_eq!(call.name.as_deref(), Some("empty.vi"));
    assert_eq!(call.terminals.len(), 12);
    assert!(call.terminals.iter().all(|t| !t.wired));

    let bd = diagram("pylavi/absolute_link_PTH.vi");
    let mut names: Vec<&str> = bd.nodes().filter_map(|n| n.name.as_deref()).collect();
    names.sort();
    assert_eq!(
        names,
        ["PTH empty PTH.vi", "Path To Command Line String.vi"]
    );
}

#[test]
fn empty_diagram_and_files_without_one() {
    let bd = diagram("pylavi/empty.vi");
    assert!(bd.root.wires.is_empty());
    assert!(bd
        .nodes()
        .all(|n| n.class == "sRN" && n.terminals.is_empty()));

    let bytes = fs::read(root().join("fixtures/pylabview/empty_libfile.llb")).unwrap();
    let file = RsrcFile::parse(&bytes).unwrap();
    assert!(BlockDiagram::from_rsrc(&file).unwrap().is_none());
}

// Hand-written expectations, independent of the snapshots. Each fixture was
// saved from the editor with exactly the named setting changed.

#[test]
fn baseline_vi() {
    let meta = fixture("pylavi/empty.vi");
    assert_eq!(meta.kind, FileKind::Vi);
    assert_eq!(meta.version.unwrap().to_string(), "21.0.0");
    assert_eq!(meta.password_protected, Some(false));
    assert!(meta.links.is_empty());
    let flags = meta.flags.unwrap();
    assert_eq!(flags.separate_compiled_code, Some(false));
    assert_eq!(flags.auto_error_handling, Some(true));
    assert_eq!(flags.debuggable, Some(true));
    assert_eq!(flags.has_breakpoints, Some(false));
    assert_eq!(flags.suspend_when_called, Some(false));
    assert_eq!(flags.run_on_open, Some(false));
    assert_eq!(flags.locked, Some(false));
    assert_eq!(flags.clear_indicators, Some(false));
    assert_eq!(flags.reentrancy, Some(Reentrancy::NonReentrant));
}

#[test]
fn single_setting_fixtures() {
    let flags = |name: &str| fixture(name).flags.unwrap();
    assert_eq!(
        flags("pylavi/empty_separate_code.vi").separate_compiled_code,
        Some(true)
    );
    assert_eq!(
        flags("pylavi/empty_no_auto_err.vi").auto_error_handling,
        Some(false)
    );
    assert_eq!(
        flags("pylavi/empty_no_debugging.vi").debuggable,
        Some(false)
    );
    assert_eq!(
        flags("pylavi/empty_suspend_when_called.vi").suspend_when_called,
        Some(true)
    );
    assert_eq!(flags("pylavi/empty_run_on_open.vi").run_on_open, Some(true));
    assert_eq!(
        flags("pylavi/empty_clear_indicators.vi").clear_indicators,
        Some(true)
    );
    assert_eq!(flags("pylavi/empty_locked.vi").locked, Some(true));
    assert_eq!(flags("pylavi/add.vi").has_breakpoints, Some(false));
    assert_eq!(
        flags("pylavi/add_breakpoints.vi").has_breakpoints,
        Some(true)
    );
}

#[test]
fn password_fixtures() {
    assert_eq!(
        fixture("pylavi/empty_locked.vi").password_protected,
        Some(false)
    );
    let protected = fixture("pylavi/empty_password.vi");
    assert_eq!(protected.password_protected, Some(true));
    assert_eq!(protected.flags.unwrap().locked, Some(true));
}

#[test]
fn version_stage_fixtures() {
    let stage = |name: &str| fixture(name).version.unwrap().stage;
    assert_eq!(stage("pylavi/empty.vi"), Stage::Release);
    assert_eq!(stage("pylavi/empty_alpha.vi"), Stage::Alpha);
    assert_eq!(stage("pylavi/empty_beta.vi"), Stage::Beta);
    assert_eq!(stage("pylavi/empty_dev.vi"), Stage::Development);
    assert_eq!(
        fixture("pylabview/empty_vifile.vi").version.unwrap().major,
        14
    );
}

#[test]
fn link_fixtures() {
    let relative = fixture("pylavi/relative_link.vi");
    // The callee is linked once from the VI (`VIVI`) and once from the
    // diagram node that calls it (`IUVI`).
    let seen: Vec<(String, String)> = relative
        .links
        .iter()
        .map(|l| (l.link_type.unwrap().to_string(), l.path.to_string()))
        .collect();
    assert_eq!(
        seen,
        [
            ("IUVI".to_owned(), "../empty.vi".to_owned()),
            ("VIVI".to_owned(), "../empty.vi".to_owned())
        ]
    );
    assert_eq!(relative.links[0].qualified_name, ["empty.vi"]);
    assert!(!relative.links[0].path.is_machine_absolute());

    let absolute = fixture("pylavi/absolute_link_PTH.vi");
    let paths: Vec<String> = absolute.links.iter().map(|l| l.path.to_string()).collect();
    assert!(
        paths.contains(&"/Volumes/marcp/Desktop/PTH empty PTH.vi".to_owned()),
        "{paths:?}"
    );
    assert!(
        paths.contains(&"<vilib>/AdvancedString/Path To Command Line String.vi".to_owned()),
        "{paths:?}"
    );
    let pinned: Vec<_> = absolute
        .links
        .iter()
        .filter(|l| l.path.is_machine_absolute())
        .collect();
    assert_eq!(pinned.len(), 2);
    assert_eq!(pinned[0].name(), Some("PTH empty PTH.vi"));
}

#[test]
fn reentrancy_fixtures() {
    for version in ["23.0", "26.0"] {
        let get = |name: &str| {
            let meta = fixture(&format!("pylavi/reentrancy/{version}/{name}.vi"));
            assert_eq!(meta.version.unwrap().major.to_string(), &version[..2]);
            let flags = meta.flags.unwrap();
            (flags.reentrancy.unwrap(), flags.inline.unwrap())
        };
        assert_eq!(get("Non-Reentrant"), (Reentrancy::NonReentrant, false));
        assert_eq!(get("Shared-Reentrant"), (Reentrancy::SharedClones, false));
        assert_eq!(
            get("Preallocate-Reentrant"),
            (Reentrancy::PreallocatedClones, false)
        );
        assert_eq!(
            get("Inline - preallocate"),
            (Reentrancy::PreallocatedClones, true)
        );
        assert!(get("Inline").1);
    }
}

#[test]
fn llb_archives_parse_as_libraries() {
    assert_eq!(
        fixture("pylabview/empty_libfile.llb").kind,
        FileKind::Library
    );
}
