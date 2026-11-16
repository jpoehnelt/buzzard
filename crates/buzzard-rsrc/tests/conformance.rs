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

use buzzard_rsrc::blocks::heap::{Heap, HeapNode};
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

// State machine facts. The files live with the tests that use them most,
// `crates/buzzard-lint/tests/fixtures/verify`; its README records where
// each comes from and its licence.

fn machine_fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../buzzard-lint/tests/fixtures/verify")
        .join(name);
    fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn machine_diagram(name: &str) -> BlockDiagram {
    let bytes = machine_fixture(name);
    let file = RsrcFile::parse(&bytes).unwrap();
    BlockDiagram::from_rsrc(&file).unwrap().unwrap()
}

/// `process.vi` (saved by version 13): a while loop with four shift
/// registers around a string case structure with the cases `"Init"` and
/// `"Idle", Default`. The expected ids were read off the raw heap.
#[test]
fn diagram_of_a_state_machine() {
    use buzzard_rsrc::{SelectorKind, SelectorMatch, ShiftRegister, TunnelMode, Value};

    let bd = machine_diagram("jki-smo/process.vi");
    let node = |uid: i32| bd.nodes().find(|n| n.uid == uid).unwrap();

    let looping = node(3379);
    assert_eq!(looping.class, "whileLoop");
    let info = looping.loop_info.as_ref().unwrap();
    let stop = info.stop.unwrap();
    assert_eq!((stop.dco, stop.stop_if_true), (3398, true));
    assert_eq!(info.shift_registers.len(), 4);
    assert_eq!(
        info.shift_registers[0],
        ShiftRegister {
            left: vec![2645],
            right: 2561
        }
    );
    // Each side of the state register has an outer terminal on the loop
    // and an inner one on the boundary of the loop's diagram.
    let boundary = &looping.diagrams[0].nodes[0];
    for (dco, class) in [(2645, "lSR"), (2561, "rSR")] {
        for owner in [looping, boundary] {
            let found: Vec<_> = owner
                .terminals
                .iter()
                .filter(|t| t.dco == Some(dco))
                .collect();
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].dco_class.as_deref(), Some(class));
            assert_eq!(found[0].data_type, Some(DataType::String));
        }
    }
    let condition = boundary
        .terminals
        .iter()
        .find(|t| t.dco == Some(3398))
        .unwrap();
    assert_eq!(condition.dco_class.as_deref(), Some("lTst"));
    assert_eq!(condition.data_type, Some(DataType::Boolean));

    let case = node(3023);
    let selector = case.selector.as_ref().unwrap();
    assert_eq!(selector.kind, SelectorKind::String);
    assert_eq!(
        selector.frames,
        [
            vec![SelectorMatch::Text("Init".to_owned())],
            vec![SelectorMatch::Text("Idle".to_owned())]
        ]
    );
    assert_eq!(selector.default_frame, Some(1));
    assert_eq!(selector.label(0), "\"Init\"");
    assert_eq!(selector.label(1), "\"Idle\", Default");
    assert_eq!(selector.label(2), "");

    // The next-state tunnel: an output that some frame leaves unwired.
    let next = case.terminals.iter().find(|t| t.uid == 3189).unwrap();
    let tunnel = next.tunnel.unwrap();
    assert!(tunnel.output && tunnel.default_if_unwired && !tunnel.conditional);
    assert_eq!(tunnel.mode, TunnelMode::Plain);
    let into = case.terminals.iter().find(|t| t.uid == 3052).unwrap();
    assert_eq!(into.dco_class.as_deref(), Some("caseSel"));
    assert_eq!(into.tunnel, None);

    // An enumeration case structure with a default frame without values.
    let nested = node(3934).selector.as_ref().unwrap();
    assert_eq!(nested.kind, SelectorKind::Enum);
    assert_eq!(
        nested.enum_items[..],
        ["unknown", "Created", "Started", "Stopped"]
    );
    assert_eq!(nested.default_frame, Some(0));
    assert_eq!(
        [nested.label(0), nested.label(1), nested.label(2)],
        ["Default", "\"unknown\"", "\"Stopped\""]
    );
    // An error cluster and a Boolean selector have no default frame.
    let error = node(303).selector.as_ref().unwrap();
    assert_eq!(error.kind, SelectorKind::ErrorCluster);
    assert_eq!(error.default_frame, None);
    assert_eq!([error.label(0), error.label(1)], ["No Error", "Error"]);
    let boolean = node(2740).selector.as_ref().unwrap();
    assert_eq!([boolean.label(0), boolean.label(1)], ["False", "True"]);

    // Constants of each simple type.
    let constant = |uid: i32| {
        bd.nodes()
            .flat_map(|n| &n.terminals)
            .find(|t| t.uid == uid)
            .unwrap()
            .constant
            .clone()
    };
    assert_eq!(constant(4888), Some(Value::String("Init".to_owned())));
    assert_eq!(constant(2430), Some(Value::Integer(-1)));
    assert_eq!(constant(1244), Some(Value::Boolean(true)));
    assert_eq!(
        constant(2958),
        Some(Value::Enum {
            value: 3,
            name: Some("Stopped".to_owned())
        })
    );
    // Every constant of a simple type decodes; the rest stay `None`.
    for terminal in bd.nodes().flat_map(|n| &n.terminals) {
        let is_constant = terminal.dco_class.as_deref() == Some("bDConstDCO");
        let simple = matches!(
            terminal.data_type,
            Some(DataType::Boolean | DataType::Numeric | DataType::String)
        );
        assert_eq!(
            terminal.constant.is_some(),
            is_constant && simple,
            "#{}",
            terminal.uid
        );
    }
}

/// The parameter index identifies the inputs of the Select function
/// whatever the order of its terminals, and a subVI call's terminals
/// carry their connector pane slot.
#[test]
fn terminals_carry_their_parameter_index() {
    let bd = machine_diagram("jki-state-machine/new-support-vis-examples.vi");
    let selects: Vec<_> = bd.nodes().filter(|n| n.primitive == Some(1516)).collect();
    assert!(selects.len() >= 3);
    for select in selects {
        let mut params: Vec<_> = select.terminals.iter().map(|t| t.param).collect();
        params.sort();
        assert_eq!(params, [Some(0), Some(1), Some(2), Some(3)]);
        let by = |index| {
            select
                .terminals
                .iter()
                .find(|t| t.param == Some(index))
                .unwrap()
        };
        assert_eq!(by(0).direction, Direction::Output);
        assert_eq!(by(2).data_type, Some(DataType::Boolean));
        assert_eq!(by(1).data_type, by(3).data_type);
    }
    let call = bd
        .nodes()
        .find(|n| n.name.as_deref() == Some("Parse State Queue__JKI_lib_State_Machine.vi"))
        .unwrap();
    assert!(call.terminals.iter().all(|t| t.param.is_some()));
    let mut slots: Vec<_> = call.terminals.iter().filter_map(|t| t.param).collect();
    slots.sort();
    slots.dedup();
    assert_eq!(slots.len(), call.terminals.len());
}

/// Corrupt the fields the state machine decoders read: selector tables,
/// constant values, shift register links, the loop condition, tunnel
/// flags and parameter indices. Each is overwritten in the decoded heap,
/// where a byte-level corruption of the file cannot aim.
#[test]
fn hostile_state_machine_fields_never_panic() {
    use buzzard_rsrc::blocks::heap::{tag_id, Content, Heap, HeapNode};
    use buzzard_rsrc::blocks::types::TypeTable;
    use std::collections::HashMap;

    const FIELDS: [&str; 18] = [
        "SelectRangeArray32",
        "SelectStringArray",
        "SelectDefaultCase",
        "caseSelDCO",
        "diagramList",
        "loopTestDCO",
        "srDCOList",
        "lsrDCOList",
        "rsrDCO",
        "ConstValue",
        "TunnelType",
        "IsConditional",
        "parmIndex",
        "paramIdx",
        "objFlags",
        "typeDesc",
        "termList",
        "dco",
    ];
    const CONTENTS: [&[u8]; 6] = [
        &[],
        &[0xff],
        &[0x7f, 0xff, 0xff, 0xff],
        &[0x80, 0, 0, 0],
        &[0, 0, 0, 9, b'x'],
        &[0xff; 9],
    ];
    type Change = Box<dyn for<'a, 'b> Fn(&'b mut HeapNode<'a>)>;

    /// Apply `change` to the `target`-th node with one of the tags, or to
    /// the `target`-th selector table entry field. Returns whether there
    /// was one.
    fn nth(
        node: &mut HeapNode<'_>,
        tags: &[i32],
        in_entry: bool,
        target: usize,
        seen: &mut usize,
        change: &Change,
    ) -> bool {
        // Class 930 is a selector table entry; its fields are tags 0 to 4.
        let entry = node.class() == Some(930);
        if tags.contains(&node.tag) || (in_entry && (0..5).contains(&node.tag)) {
            if *seen == target {
                change(node);
                return true;
            }
            *seen += 1;
        }
        node.children
            .iter_mut()
            .any(|child| nth(child, tags, entry, target, seen, change))
    }

    let mut changes: Vec<Change> = Vec::new();
    for bytes in CONTENTS {
        changes.push(Box::new(move |node| node.content = Content::Bytes(bytes)));
    }
    changes.push(Box::new(|node| node.children.clear()));
    changes.push(Box::new(|node| node.content = Content::Bool(true)));
    // Point every reference below at one object.
    changes.push(Box::new(|node| {
        for child in &mut node.children {
            child.attrs = vec![(-3, 1)];
        }
        node.attrs = vec![(-3, 3023)];
    }));
    changes.push(Box::new(|node| {
        let copies = node.children.clone();
        node.children.extend(copies);
    }));
    let nothing: Change = Box::new(|_| {});

    let tags: Vec<i32> = FIELDS.iter().map(|name| tag_id(name).unwrap()).collect();
    let names = HashMap::new();
    for name in [
        "jki-smo/process.vi",
        "jki-state-machine/new-support-vis-examples.vi",
    ] {
        let bytes = machine_fixture(name);
        let file = RsrcFile::parse(&bytes).unwrap();
        let heap = zlib::decompress(file.first(b"BDHb").unwrap()).unwrap();
        let vctp = zlib::decompress(file.first(b"VCTP").unwrap()).unwrap();
        let dthp = file.first(b"DTHP");
        let types = TypeTable::parse(&vctp, dthp).unwrap();
        let parsed = Heap::parse(&heap).unwrap();
        let intact = BlockDiagram::from_heap(&parsed, &types, &names).unwrap();
        assert!(intact.nodes().any(|n| n.selector.is_some()));
        assert!(intact.nodes().any(|n| n.loop_info.is_some()));

        // Count the fields, then corrupt a spread of them in every way.
        let mut total = 0;
        for root in &mut parsed.clone().roots {
            nth(root, &tags, false, usize::MAX, &mut total, &nothing);
        }
        assert!(total > 1_000, "{name}: only {total} fields");
        for target in (0..total).step_by((total / 150).max(1)) {
            for change in &changes {
                let mut copy = parsed.clone();
                let mut seen = 0;
                let hit = copy
                    .roots
                    .iter_mut()
                    .any(|root| nth(root, &tags, false, target, &mut seen, change));
                assert!(hit);
                if let Some(diagram) = BlockDiagram::from_heap(&copy, &types, &names) {
                    for node in diagram.nodes() {
                        if let Some(selector) = &node.selector {
                            for frame in 0..=selector.frames.len() {
                                let _ = selector.label(frame);
                            }
                        }
                    }
                }
            }
        }

        // The decompressed heap and type table, byte by byte in samples.
        for pos in (0..heap.len()).step_by((heap.len() / 400).max(1)) {
            for value in [0x00, 0x7f, 0xff] {
                let mut copy = heap.clone();
                copy[pos] = value;
                if let Ok(parsed) = Heap::parse(&copy) {
                    let _ = BlockDiagram::from_heap(&parsed, &types, &names);
                }
            }
        }
        for pos in (0..vctp.len()).step_by((vctp.len() / 400).max(1)) {
            for value in [0x00, 0x7f, 0xff] {
                let mut copy = vctp.clone();
                copy[pos] = value;
                let Ok(table) = TypeTable::parse(&copy, dthp) else {
                    continue;
                };
                for id in 0..table.len() as u32 + 2 {
                    let _ = table.heap_type_info(id);
                    for bytes in CONTENTS {
                        let _ = table.constant(id, bytes);
                    }
                }
                let _ = BlockDiagram::from_heap(&parsed, &table, &names);
            }
        }
    }
}

// What the decoder cannot carry over. The state machine verification
// treats a missing wire as evidence that a tunnel is unwired, which is
// only sound when nothing was lost on the way.

/// The first node, depth first, that satisfies a condition.
fn first_mut<'t, 'a>(
    node: &'t mut HeapNode<'a>,
    wanted: &dyn Fn(&HeapNode<'a>) -> bool,
) -> Option<&'t mut HeapNode<'a>> {
    if wanted(node) {
        return Some(node);
    }
    node.children
        .iter_mut()
        .find_map(|child| first_mut(child, wanted))
}

/// The first list field with the given tag that has two elements or more.
fn list_mut<'t, 'a>(root: &'t mut HeapNode<'a>, tag: i32) -> &'t mut HeapNode<'a> {
    first_mut(root, &|n| n.tag == tag && n.children.len() >= 2).unwrap()
}

/// The elements of an object's own list field with the given tag.
fn own_list<'t, 'a>(object: &'t mut HeapNode<'a>, tag: i32) -> &'t mut Vec<HeapNode<'a>> {
    let list = object.children.iter_mut().find(|c| c.tag == tag);
    &mut list.unwrap().children
}

/// The fields of the first object that has a field with the given tag.
fn fields_of_owner<'t, 'a>(root: &'t mut HeapNode<'a>, tag: i32) -> &'t mut Vec<HeapNode<'a>> {
    let owner = first_mut(root, &|n| {
        n.class().is_some() && n.children.iter().any(|c| c.tag == tag)
    });
    &mut owner.unwrap().children
}

/// Attribute ids of a heap object: its class and its unique id.
const CLASS: i32 = -2;
const UID: i32 = -3;
/// Attribute of a list field: how many elements it has.
const ELEMENTS: i32 = -5;
/// An id no object of the fixtures has.
const NOWHERE: i32 = 0x7fff_0000;
/// A field tag nothing uses.
const NO_FIELD: i32 = 30_000;

fn set_attr(node: &mut HeapNode<'_>, id: i32, value: i32) {
    for attr in &mut node.attrs {
        if attr.0 == id {
            attr.1 = value;
        }
    }
}

/// Make an element a reference to an object that does not exist.
fn dangle(element: &mut HeapNode<'_>) {
    element.attrs = vec![(UID, NOWHERE)];
    element.children.clear();
}

#[test]
fn every_loss_of_the_decoder_is_recorded() {
    use buzzard_rsrc::blocks::heap::tag_id;
    use buzzard_rsrc::blocks::types::TypeTable;
    use buzzard_rsrc::Loss;
    use std::collections::HashMap;

    let bytes = machine_fixture("jki-smo/process.vi");
    let file = RsrcFile::parse(&bytes).unwrap();
    let heap = zlib::decompress(file.first(b"BDHb").unwrap()).unwrap();
    let vctp = zlib::decompress(file.first(b"VCTP").unwrap()).unwrap();
    let types = TypeTable::parse(&vctp, file.first(b"DTHP")).unwrap();
    let names = HashMap::new();
    let parsed = Heap::parse(&heap).unwrap();
    let tag = |name: &str| tag_id(name).unwrap();
    let (nodes, wires) = (tag("nodeList"), tag("signalList"));
    let (terms, frames) = (tag("termList"), tag("diagramList"));

    // The untouched file decodes completely, by either entry point.
    let intact = BlockDiagram::from_heap(&parsed, &types, &names).unwrap();
    assert!(intact.is_complete());
    let whole = BlockDiagram::from_inflated_heap(&file, &heap)
        .unwrap()
        .unwrap();
    assert!(whole.is_complete());
    assert_eq!(Some(whole), BlockDiagram::from_rsrc(&file).unwrap());

    // Every kind of loss in every diagram after a change to the heap.
    let losses = |change: &dyn Fn(&mut HeapNode<'_>)| -> Vec<Loss> {
        let mut copy = parsed.clone();
        change(&mut copy.roots[0]);
        let diagram = BlockDiagram::from_heap(&copy, &types, &names).unwrap();
        let mut all: Vec<Loss> = diagram
            .diagrams()
            .iter()
            .flat_map(|d| d.losses.clone())
            .collect();
        all.sort();
        all.dedup();
        assert_eq!(diagram.is_complete(), all.is_empty());
        all
    };
    assert_eq!(losses(&|_| {}), []);

    // A signal the decoder cannot resolve: the wire is dropped. This is
    // what one changed byte did to a real state machine, where the
    // dropped wire fed the next-state tunnel.
    assert_eq!(
        losses(&|root| dangle(&mut list_mut(root, wires).children[0])),
        [Loss::Wire]
    );
    // A signal without an id, one with a single end, one with an end
    // that has no id.
    assert_eq!(
        losses(&|root| list_mut(root, wires).children[0]
            .attrs
            .retain(|a| a.0 != UID)),
        [Loss::Wire]
    );
    assert_eq!(
        losses(&|root| own_list(&mut list_mut(root, wires).children[0], terms).truncate(1)),
        [Loss::Wire]
    );
    assert_eq!(
        losses(
            &|root| own_list(&mut list_mut(root, wires).children[0], terms)[1]
                .attrs
                .clear()
        ),
        [Loss::Wire]
    );
    // A wire end that names no terminal of its diagram: the other thing
    // one changed byte did, by renumbering the terminal of a tunnel.
    assert_eq!(
        losses(&|root| {
            let ends = own_list(&mut list_mut(root, wires).children[0], terms);
            set_attr(&mut ends[1], UID, NOWHERE);
        }),
        [Loss::WireEnd]
    );
    // The same signal listed twice. One of the two definitions is also
    // a signal that no decoded wire stands for.
    assert_eq!(
        losses(&|root| {
            let list = &mut list_mut(root, wires).children;
            list.push(list[0].clone());
        }),
        [
            Loss::Wire,
            Loss::RepeatedWire,
            Loss::WireConflict,
            Loss::RepeatedObject
        ]
    );
    // Two wires into one terminal.
    assert_eq!(
        losses(&|root| {
            let list = &mut list_mut(root, wires).children;
            let sink = own_list(&mut list[0], terms)[1].clone();
            own_list(&mut list[1], terms)[1] = sink;
        }),
        [Loss::WireConflict]
    );
    // A whole signal list out of sight behind a damaged field tag: no
    // element fails to resolve, but the signals are not accounted for,
    // and the tag is not one an object has.
    assert_eq!(
        losses(&|root| list_mut(root, wires).tag = NO_FIELD),
        [Loss::Wire, Loss::Field]
    );

    // An element gone from a list that still states its old length: the
    // heap no longer holds the object at all, so only the length tells.
    assert_eq!(
        losses(&|root| drop(list_mut(root, wires).children.pop())),
        [Loss::Wire]
    );
    let found = losses(&|root| drop(list_mut(root, nodes).children.pop()));
    assert!(found.contains(&Loss::Node), "{found:?}");
    let found = losses(&|root| drop(list_mut(root, frames).children.pop()));
    assert!(found.contains(&Loss::Frame), "{found:?}");
    let found = losses(&|root| {
        own_list(&mut list_mut(root, nodes).children[0], terms).pop();
    });
    assert!(found.contains(&Loss::Terminal), "{found:?}");
    let found = losses(&|root| {
        own_list(&mut list_mut(root, wires).children[0], terms).push(HeapNode {
            tag: 0,
            attrs: vec![(UID, NOWHERE)],
            content: buzzard_rsrc::blocks::heap::Content::None,
            children: Vec::new(),
        });
    });
    assert!(found.contains(&Loss::Wire), "{found:?}");

    // A field behind a damaged tag, and a field twice. The first signal
    // list sits in a diagram object.
    assert_eq!(
        losses(&|root| {
            let fields = fields_of_owner(root, wires);
            let last = fields.len() - 1;
            fields[last].tag = NO_FIELD;
        })
        .last(),
        Some(&Loss::Field)
    );
    let found = losses(&|root| {
        let fields = fields_of_owner(root, wires);
        let again = fields[0].clone();
        fields.push(again);
    });
    assert!(found.contains(&Loss::Field), "{found:?}");

    // A heap whose byte stream is irregular. One changed byte can make
    // the count of a node's attributes larger, and the entries that
    // follow are then read as attributes: the tree is whole, the nodes
    // are gone, and nothing in the tree tells. Here the first list
    // element with a class and an id (`10 19 02 fe`: open, two
    // attributes, the first of them the class) gets a count of 18.
    let streamed = |change: &dyn Fn(&mut Vec<u8>)| -> Vec<Loss> {
        let mut copy = heap.clone();
        change(&mut copy);
        let diagram = BlockDiagram::from_inflated_heap(&file, &copy);
        let diagram = diagram.unwrap().unwrap();
        let mut all: Vec<Loss> = diagram
            .diagrams()
            .iter()
            .flat_map(|d| d.losses.clone())
            .collect();
        all.sort();
        all.dedup();
        all
    };
    assert_eq!(streamed(&|_| {}), []);
    let element = heap
        .windows(4)
        .position(|bytes| bytes == [0x10, 0x19, 0x02, 0xfe])
        .unwrap();
    let found = streamed(&|bytes| bytes[element + 2] = 0x12);
    assert!(found.contains(&Loss::Stream), "{found:?}");
    // A node the decoder cannot resolve takes its terminals with it.
    let found = losses(&|root| dangle(&mut list_mut(root, nodes).children[1]));
    assert!(found.contains(&Loss::Node), "{found:?}");
    assert!(found.contains(&Loss::Terminal), "{found:?}");
    // A node listed twice.
    let found = losses(&|root| {
        let list = &mut list_mut(root, nodes).children;
        list.push(list[1].clone());
    });
    assert!(found.contains(&Loss::RepeatedNode), "{found:?}");
    // A node list out of sight: its terminals are not accounted for.
    let found = losses(&|root| list_mut(root, nodes).tag = NO_FIELD);
    assert!(found.contains(&Loss::Terminal), "{found:?}");

    // A terminal the decoder cannot resolve, one without an id, one
    // listed twice and something that is not a terminal. The first node
    // of a diagram defines its terminals in its own list.
    let found = losses(&|root| {
        dangle(&mut own_list(&mut list_mut(root, nodes).children[0], terms)[0]);
    });
    assert!(found.contains(&Loss::Terminal), "{found:?}");
    let found = losses(&|root| {
        own_list(&mut list_mut(root, nodes).children[0], terms)[0]
            .attrs
            .retain(|a| a.0 != UID);
    });
    assert!(found.contains(&Loss::Terminal), "{found:?}");
    let found = losses(&|root| {
        let list = own_list(&mut list_mut(root, nodes).children[0], terms);
        list.push(list[0].clone());
    });
    assert!(found.contains(&Loss::RepeatedTerminal), "{found:?}");
    assert!(found.contains(&Loss::RepeatedObject), "{found:?}");
    let found = losses(&|root| {
        let list = own_list(&mut list_mut(root, nodes).children[0], terms);
        // Class 47 is a built-in function.
        set_attr(&mut list[0], CLASS, 47);
    });
    assert!(found.contains(&Loss::Terminal), "{found:?}");

    // A frame of a structure that the decoder cannot resolve, the frames
    // of a structure out of sight, and a frame listed twice.
    let found = losses(&|root| dangle(&mut list_mut(root, frames).children[1]));
    assert!(found.contains(&Loss::Frame), "{found:?}");
    let found = losses(&|root| list_mut(root, frames).tag = NO_FIELD);
    assert!(found.contains(&Loss::Frame), "{found:?}");
    let found = losses(&|root| {
        let list = &mut list_mut(root, frames).children;
        list.push(list[0].clone());
    });
    assert!(found.contains(&Loss::Frame), "{found:?}");
}

/// The enumeration case structure #3934 of `process.vi`: three frames, the
/// first of them a default frame without values, on an enumeration of
/// four items.
#[test]
fn a_selector_table_that_is_not_whole_is_absent() {
    use buzzard_rsrc::blocks::heap::tag_id;
    use buzzard_rsrc::blocks::types::TypeTable;
    use buzzard_rsrc::CaseSelector;
    use std::collections::HashMap;

    let bytes = machine_fixture("jki-smo/process.vi");
    let file = RsrcFile::parse(&bytes).unwrap();
    let heap = zlib::decompress(file.first(b"BDHb").unwrap()).unwrap();
    let vctp = zlib::decompress(file.first(b"VCTP").unwrap()).unwrap();
    let types = TypeTable::parse(&vctp, file.first(b"DTHP")).unwrap();
    let names = HashMap::new();
    let parsed = Heap::parse(&heap).unwrap();
    let table = tag_id("SelectRangeArray32").unwrap();

    let selector = |change: &dyn Fn(&mut HeapNode<'_>)| -> Option<CaseSelector> {
        let mut copy = parsed.clone();
        let case = first_mut(&mut copy.roots[0], &|n| {
            n.uid() == Some(3934) && n.class().is_some()
        });
        let list = case.unwrap().children.iter_mut().find(|c| c.tag == table);
        change(list.unwrap());
        let diagram = BlockDiagram::from_heap(&copy, &types, &names).unwrap();
        let node = diagram.nodes().find(|n| n.uid == 3934).unwrap();
        node.selector.clone()
    };
    let intact = selector(&|_| {}).unwrap();
    assert_eq!(intact.frames.len(), 3);
    assert_eq!(intact.default_frame, Some(0));
    assert!(intact.frames[0].is_empty());
    assert!(intact.frames[1..].iter().all(|frame| frame.len() == 1));
    // The type is an instance of a type definition, which is named.
    assert_eq!(
        intact.type_definition.as_deref(),
        Some("SMOState --Enum.ctl")
    );

    // An entry that lost its frame index used to read as frame 0, which
    // moved its value to another case and left its own frame without
    // any: the table was accepted and a state was made up for the frame.
    assert_eq!(
        selector(&|entries| entries.children[0].children.retain(|f| f.tag != 4)),
        None
    );
    // Any other field missing, a field twice, a field too many.
    for tag in 0..4 {
        assert_eq!(
            selector(&|entries| entries.children[1].children.retain(|f| f.tag != tag)),
            None
        );
    }
    assert_eq!(
        selector(&|entries| {
            let again = entries.children[1].children[0].clone();
            entries.children[1].children.push(again);
        }),
        None
    );
    assert_eq!(
        selector(&|entries| {
            let mut other = entries.children[1].children[0].clone();
            other.tag = 5;
            entries.children[1].children.push(other);
        }),
        None
    );
    // A frame that is not the default and that no entry selects, with
    // the list's stated length kept right.
    let resize = |entries: &mut HeapNode<'_>| {
        let length = entries.children.len() as i32;
        set_attr(entries, ELEMENTS, length);
    };
    assert_eq!(
        selector(&|entries| {
            entries.children.remove(0);
            resize(entries);
        }),
        None
    );
    assert_eq!(
        selector(&|entries| {
            entries.children.clear();
            resize(entries);
        }),
        None
    );
    // Two entries for one frame are fine as long as every frame has one.
    let doubled = selector(&|entries| {
        let again = entries.children[0].clone();
        entries.children.push(again);
        resize(entries);
    });
    let doubled = doubled.unwrap();
    assert_eq!(doubled.frames.iter().map(Vec::len).sum::<usize>(), 3);
    // But not when the list holds another number of entries than it
    // states: an entry was lost or made up on the way.
    assert_eq!(
        selector(&|entries| {
            let again = entries.children[0].clone();
            entries.children.push(again);
        }),
        None
    );
}
