//! Unit tests on synthetic diagrams: every change kind, both matching
//! passes, and input no real editor would write.

use buzzard_rsrc::{
    BlockDiagram, DataType, Diagram, Direction, FileKind, FourCc, LinkRef, LvPath, Node, Rect,
    Reentrancy, Terminal, Version, ViFlags, ViMetadata, Wire,
};

use super::*;

const NUM: DataType = DataType::Numeric;
const STR: DataType = DataType::String;
const ERR: DataType = DataType::ErrorCluster;

fn metadata() -> ViMetadata {
    ViMetadata {
        kind: FileKind::Vi,
        file_type: FourCc::new(b"LVIN"),
        saved_name: Some("Main.vi".to_owned()),
        version: Some(Version::from_u32(0x2100_8000)),
        flags: Some(ViFlags {
            separate_compiled_code: Some(true),
            auto_error_handling: Some(true),
            debuggable: Some(true),
            has_breakpoints: Some(false),
            breakpoint_count: Some(0),
            suspend_when_called: Some(false),
            run_on_open: Some(false),
            locked: Some(false),
            clear_indicators: Some(false),
            saved_for_previous: Some(false),
            reentrancy: Some(Reentrancy::NonReentrant),
            inline: Some(false),
        }),
        password_protected: Some(false),
        libraries: Vec::new(),
        links: Vec::new(),
        resource_types: Vec::new(),
    }
}

fn link(name: &str) -> LinkRef {
    LinkRef {
        link_type: None,
        qualified_name: vec![name.to_owned()],
        path: LvPath::sibling(name),
        heap_uids: Vec::new(),
    }
}

fn terminal(uid: i32, data_type: DataType) -> Terminal {
    Terminal {
        uid,
        bounds: None,
        direction: Direction::Input,
        wired: false,
        data_type: Some(data_type),
        dco: Some(uid + 10_000),
        dco_class: Some("parm".to_owned()),
        ..Terminal::default()
    }
}

fn rect(left: i16, top: i16) -> Rect {
    Rect {
        top,
        left,
        bottom: top + 32,
        right: left + 32,
    }
}

fn node(uid: i32, class: &str, terminals: Vec<Terminal>) -> Node {
    Node {
        uid,
        class: class.to_owned(),
        name: None,
        primitive: None,
        bounds: Some(rect(uid as i16, 0)),
        terminals,
        diagrams: Vec::new(),
        ..Node::default()
    }
}

fn sub_vi(uid: i32, name: &str, terminals: Vec<Terminal>) -> Node {
    Node {
        name: Some(name.to_owned()),
        ..node(uid, "iUse", terminals)
    }
}

fn function(uid: i32, id: u32, terminals: Vec<Terminal>) -> Node {
    Node {
        primitive: Some(id),
        ..node(uid, "prim", terminals)
    }
}

fn structure(uid: i32, class: &str, frames: Vec<Diagram>) -> Node {
    Node {
        diagrams: frames,
        ..node(uid, class, Vec::new())
    }
}

fn frame(uid: i32, nodes: Vec<Node>, wires: &[(i32, &[i32])]) -> Diagram {
    Diagram {
        uid,
        nodes,
        wires: wires
            .iter()
            .enumerate()
            .map(|(i, (source, sinks))| Wire {
                uid: uid * 1000 + i as i32,
                source: *source,
                sinks: sinks.to_vec(),
            })
            .collect(),
        losses: Vec::new(),
    }
}

fn file(nodes: Vec<Node>, wires: &[(i32, &[i32])]) -> Inspection {
    Inspection {
        metadata: metadata(),
        diagram: Some(BlockDiagram {
            root: frame(1, nodes, wires),
        }),
    }
}

fn text(old: &Inspection, new: &Inspection) -> String {
    to_text(&diff(Some(old), Some(new), DiffOptions::default()))
}

/// Give every object a different id, as a renumbering save would.
fn renumber(file: &Inspection, by: i32) -> Inspection {
    fn shift(diagram: &mut Diagram, by: i32) {
        let add = |uid: &mut i32| *uid = uid.wrapping_add(by);
        add(&mut diagram.uid);
        for wire in &mut diagram.wires {
            add(&mut wire.uid);
            add(&mut wire.source);
            wire.sinks.iter_mut().for_each(add);
        }
        for node in &mut diagram.nodes {
            add(&mut node.uid);
            node.terminals.iter_mut().for_each(|t| add(&mut t.uid));
            node.diagrams.iter_mut().for_each(|d| shift(d, by));
        }
    }
    let mut out = file.clone();
    shift(&mut out.diagram.as_mut().unwrap().root, by);
    out
}

/// A small but complete diagram: two calls in a row inside a loop.
fn sample() -> Inspection {
    file(
        vec![
            node(7, "sRN", vec![terminal(8, STR)]),
            structure(
                12,
                "forLoop",
                vec![frame(
                    13,
                    vec![
                        node(14, "sRN", Vec::new()),
                        sub_vi(
                            20,
                            "Open Config Data.vi",
                            vec![terminal(21, STR), terminal(22, ERR)],
                        ),
                        sub_vi(
                            30,
                            "Read Key.vi",
                            vec![terminal(31, ERR), terminal(32, NUM)],
                        ),
                    ],
                    &[(22, &[31])],
                )],
            ),
        ],
        &[],
    )
}

/// The loop of [`sample`], for editing its contents.
fn inner(file: &mut Inspection) -> &mut Diagram {
    &mut file.diagram.as_mut().unwrap().root.nodes[1].diagrams[0]
}

#[test]
fn identical_files_have_no_differences() {
    let result = diff(Some(&sample()), Some(&sample()), DiffOptions::default());
    assert!(result.identical);
    assert_eq!(result.summary, Summary::default());
    assert_eq!(to_text(&result), "no differences\n");
    assert_eq!(to_markdown(&result), "**no differences**\n");
}

#[test]
fn every_setting_is_compared() {
    let old = sample();
    let mut new = sample();
    new.metadata.file_type = FourCc::new(b"LVCC");
    new.metadata.saved_name = Some("Other.vi".to_owned());
    new.metadata.version = Some(Version::from_u32(0x2300_8000));
    new.metadata.libraries = vec!["Outer.lvlib".to_owned(), "Inner.lvclass".to_owned()];
    new.metadata.password_protected = Some(true);
    new.metadata.flags = Some(ViFlags {
        separate_compiled_code: Some(false),
        auto_error_handling: Some(false),
        debuggable: Some(false),
        has_breakpoints: Some(true),
        breakpoint_count: Some(2),
        suspend_when_called: Some(true),
        run_on_open: Some(true),
        locked: Some(true),
        clear_indicators: Some(true),
        saved_for_previous: None,
        reentrancy: Some(Reentrancy::PreallocatedClones),
        inline: Some(true),
    });
    assert_eq!(
        text(&old, &new),
        "17 settings changed\n\
         changed: file type LVIN -> LVCC\n\
         changed: saved name `Main.vi` -> `Other.vi`\n\
         changed: saved version 21.0.0 -> 23.0.0\n\
         changed: owning library none -> `Outer.lvlib > Inner.lvclass`\n\
         changed: compiled code separated on -> off\n\
         changed: automatic error handling on -> off\n\
         changed: debugging on -> off\n\
         changed: breakpoints off -> on\n\
         changed: suspend when called off -> on\n\
         changed: run when opened off -> on\n\
         changed: clear indicators when called off -> on\n\
         changed: saved for previous version off -> unknown\n\
         changed: inlining off -> on\n\
         changed: locked off -> on\n\
         changed: breakpoint count 0 -> 2\n\
         changed: reentrancy non-reentrant -> preallocated clones\n\
         changed: password protection off -> on\n"
    );
}

#[test]
fn a_missing_password_record_is_no_password() {
    let old = sample();
    let mut new = sample();
    new.metadata.password_protected = None;
    assert!(diff(Some(&old), Some(&new), DiffOptions::default()).identical);
}

#[test]
fn dependencies_are_compared_by_path_and_name() {
    let mut old = sample();
    let mut new = sample();
    old.metadata.links = vec![link("Kept.vi"), link("Gone.vi")];
    // The same file linked twice, from other heap objects: still one.
    let mut again = link("Kept.vi");
    again.link_type = Some(FourCc::new(b"VIVI"));
    again.heap_uids = vec![40, 41];
    let mut renamed = link("New.vi");
    renamed.qualified_name = vec!["Lib.lvlib".to_owned(), "Item".to_owned()];
    new.metadata.links = vec![link("Kept.vi"), again, renamed];
    assert_eq!(
        text(&old, &new),
        "1 dependency added, 1 removed\n\
         added: dependency `Lib.lvlib:Item` at `../New.vi`\n\
         removed: dependency `../Gone.vi`\n"
    );
}

#[test]
fn added_and_removed_nodes_are_located_by_their_structures() {
    let old = sample();
    let mut new = sample();
    inner(&mut new).nodes.remove(2);
    inner(&mut new)
        .nodes
        .push(sub_vi(40, "Close Config Data.vi", vec![terminal(41, ERR)]));
    inner(&mut new).wires[0].sinks = vec![41];
    assert_eq!(
        text(&old, &new),
        "1 node added, 1 removed; 1 wire added, 1 removed\n\
         added: subVI `Close Config Data.vi` #40 in for loop #12\n\
         removed: subVI `Read Key.vi` #30 in for loop #12\n\
         added: error cluster wire from subVI `Open Config Data.vi` #20 to \
         subVI `Close Config Data.vi` #40 in for loop #12\n\
         removed: error cluster wire from subVI `Open Config Data.vi` #20 to \
         subVI `Read Key.vi` #30 in for loop #12\n"
    );
}

#[test]
fn frames_are_named_when_a_structure_has_several() {
    let cases = |second: Vec<Node>| {
        file(
            vec![structure(
                50,
                "select",
                vec![frame(51, Vec::new(), &[]), frame(52, second, &[])],
            )],
            &[],
        )
    };
    let old = cases(Vec::new());
    let new = cases(vec![structure(
        60,
        "whileLoop",
        vec![frame(61, vec![function(70, 1050, Vec::new())], &[])],
    )]);
    assert_eq!(
        text(&old, &new),
        "2 nodes added\n\
         added: built-in function `Add` #70 in case structure #50 frame 1 > while loop #60\n\
         added: while loop #60 in case structure #50 frame 1\n"
    );
}

#[test]
fn changes_to_a_matched_node_are_listed() {
    let old = sample();
    let mut new = sample();
    // Relinked to another file.
    inner(&mut new).nodes[1].name = Some("Open Config Data (compatibility).vi".to_owned());
    // One terminal more, and one of another type.
    inner(&mut new).nodes[2].terminals =
        vec![terminal(31, ERR), terminal(32, STR), terminal(33, STR)];
    // The loop gains a frame (not possible in the editor; counts are shown).
    let root = &mut new.diagram.as_mut().unwrap().root;
    root.nodes[1].diagrams.push(frame(99, Vec::new(), &[]));
    assert_eq!(
        text(&old, &new),
        "3 nodes changed\n\
         changed: for loop #12: frames 1 -> 2\n\
         changed: subVI `Open Config Data (compatibility).vi` #20 in for loop #12 frame 0: \
         name `Open Config Data.vi` -> `Open Config Data (compatibility).vi`\n\
         changed: subVI `Read Key.vi` #30 in for loop #12 frame 0: \
         terminals 2 -> 3 (-1 numeric, +2 string)\n"
    );
}

#[test]
fn same_count_of_terminals_with_other_types_is_a_change() {
    let old = file(vec![function(10, 5, vec![terminal(11, NUM)])], &[]);
    let mut new = file(vec![function(10, 6, vec![terminal(11, STR)])], &[]);
    assert_eq!(
        text(&old, &new),
        "1 node changed\n\
         changed: built-in function 6 #10: function id 5 -> 6; \
         terminal types changed (-1 numeric, +1 string)\n"
    );
    new.diagram.as_mut().unwrap().root.nodes[0].terminals[0].data_type = None;
    assert_eq!(
        text(&old, &new),
        "1 node changed\n\
         changed: built-in function 6 #10: function id 5 -> 6; \
         terminal types changed (-1 numeric, +1 unknown type)\n"
    );
}

#[test]
fn a_node_that_enters_a_structure_is_changed_not_replaced() {
    let old = file(
        vec![
            function(10, 5, Vec::new()),
            structure(12, "whileLoop", vec![frame(13, Vec::new(), &[])]),
        ],
        &[],
    );
    let new = file(
        vec![structure(
            12,
            "whileLoop",
            vec![frame(13, vec![function(10, 5, Vec::new())], &[])],
        )],
        &[],
    );
    assert_eq!(
        text(&old, &new),
        "1 node changed\n\
         changed: built-in function 5 #10 in while loop #12: \
         enclosing structure top level -> while loop #12\n"
    );
    assert_eq!(
        text(&new, &old),
        "1 node changed\n\
         changed: built-in function 5 #10: \
         enclosing structure while loop #12 -> top level\n"
    );
}

#[test]
fn reordered_frames_do_not_relocate_their_contents() {
    let cases = |frames: Vec<Diagram>| file(vec![structure(50, "select", frames)], &[]);
    let first = || frame(51, vec![function(70, 1, Vec::new())], &[]);
    let second = || frame(52, vec![function(80, 2, Vec::new())], &[]);
    let old = cases(vec![first(), second()]);
    // A case is inserted in front; frames keep their ids.
    let new = cases(vec![frame(53, Vec::new(), &[]), first(), second()]);
    assert_eq!(
        text(&old, &new),
        "1 node changed\nchanged: case structure #50: frames 2 -> 3\n"
    );
}

#[test]
fn moves_are_separate_and_can_be_ignored() {
    let old = sample();
    let mut new = sample();
    inner(&mut new).nodes[1].bounds = Some(rect(120, 40));
    // Resized as well.
    inner(&mut new).nodes[2].bounds = Some(Rect {
        top: 0,
        left: 30,
        bottom: 48,
        right: 94,
    });
    // A boundary only mirrors its structure and is never "moved".
    inner(&mut new).nodes[0].bounds = Some(rect(-5, -5));
    assert_eq!(
        text(&old, &new),
        "2 nodes moved\n\
         moved: subVI `Open Config Data.vi` #20 in for loop #12 (20,0 -> 120,40)\n\
         moved: subVI `Read Key.vi` #30 in for loop #12 (30,0 32x32 -> 30,0 64x48)\n"
    );
    let ignored = diff(Some(&old), Some(&new), DiffOptions { ignore_moves: true });
    assert!(ignored.identical);
    assert_eq!(ignored.summary.moves_ignored, 2);
    assert_eq!(to_text(&ignored), "no differences (2 moves ignored)\n");
}

#[test]
fn a_changed_and_moved_node_is_in_both_lists() {
    let old = file(vec![sub_vi(10, "A.vi", Vec::new())], &[]);
    let mut new = file(vec![sub_vi(10, "B.vi", Vec::new())], &[]);
    new.diagram.as_mut().unwrap().root.nodes[0].bounds = Some(rect(1, 2));
    assert_eq!(
        text(&old, &new),
        "1 node changed, 1 moved\n\
         changed: subVI `B.vi` #10: name `A.vi` -> `B.vi`\n\
         moved: subVI `B.vi` #10 (10,0 -> 1,2)\n"
    );
    let ignored = diff(Some(&old), Some(&new), DiffOptions { ignore_moves: true });
    assert_eq!(
        to_text(&ignored),
        "1 node changed (1 move ignored)\n\
         changed: subVI `B.vi` #10: name `A.vi` -> `B.vi`\n"
    );
}

fn wiring(wires: &[(i32, &[i32])]) -> Inspection {
    file(
        vec![
            function(10, 1, vec![terminal(11, NUM), terminal(12, STR)]),
            function(
                20,
                2,
                vec![terminal(21, NUM), terminal(22, NUM), terminal(23, STR)],
            ),
            function(30, 3, vec![terminal(31, NUM)]),
        ],
        wires,
    )
}

#[test]
fn wires_are_compared_as_connections() {
    let old = wiring(&[(11, &[21]), (12, &[23])]);
    // A branch is added to the first wire; the second wire is deleted.
    let new = wiring(&[(11, &[21, 31])]);
    assert_eq!(
        text(&old, &new),
        "1 wire added, 1 removed\n\
         added: numeric wire from built-in function 1 #10 to built-in function 3 #30\n\
         removed: string wire from built-in function 1 #10 to built-in function 2 #20\n"
    );
}

#[test]
fn a_connection_that_changes_terminal_is_one_changed_wire() {
    let old = wiring(&[(11, &[21])]);
    let new = wiring(&[(11, &[22])]);
    let result = diff(Some(&old), Some(&new), DiffOptions::default());
    assert_eq!(
        to_text(&result),
        "1 wire changed\n\
         changed: numeric wire from built-in function 1 #10 to built-in function 2 #20: \
         attached to a different terminal\n"
    );
    let change = &result.wires.changed[0];
    assert_eq!((change.old_from_terminal, change.old_to_terminal), (11, 21));
    assert_eq!(
        (change.wire.from.terminal, change.wire.to.terminal),
        (11, 22)
    );
}

#[test]
fn rewiring_the_same_wire_object_is_not_a_change() {
    // Wire ids differ, the connections are the same.
    let old = wiring(&[(11, &[21]), (12, &[23])]);
    let new = wiring(&[(12, &[23]), (11, &[21])]);
    assert!(diff(Some(&old), Some(&new), DiffOptions::default()).identical);
}

#[test]
fn renumbered_ids_are_matched_by_signature() {
    let old = sample();
    let new = renumber(&old, 1000);
    let result = diff(Some(&old), Some(&new), DiffOptions::default());
    assert!(result.identical, "{}", to_text(&result));
}

#[test]
fn signature_matches_report_changes_and_leave_the_rest_unmatched() {
    let old = sample();
    let mut new = renumber(&old, 1000);
    inner(&mut new).nodes[1].bounds = Some(rect(300, 300));
    inner(&mut new).nodes.push(function(5000, 77, Vec::new()));
    let result = diff(Some(&old), Some(&new), DiffOptions::default());
    assert_eq!(
        to_text(&result),
        "1 node added, 1 moved\n\
         added: built-in function 77 #5000 in for loop #1012\n\
         moved: subVI `Open Config Data.vi` #1020 in for loop #1012 (20,0 -> 300,300)\n"
    );
    let moved = &result.nodes.moved[0];
    assert_eq!(
        (moved.old_uid, moved.matched_by),
        (20, MatchKind::Signature)
    );
}

#[test]
fn signature_does_not_match_across_structures_or_names() {
    // Same function, same ids nowhere: one at the top level, one in a loop.
    let old = file(vec![function(10, 5, Vec::new())], &[]);
    let new = file(
        vec![structure(
            112,
            "whileLoop",
            vec![frame(113, vec![function(110, 5, Vec::new())], &[])],
        )],
        &[],
    );
    assert_eq!(
        text(&old, &new),
        "2 nodes added, 1 removed\n\
         added: built-in function 5 #110 in while loop #112\n\
         added: while loop #112\n\
         removed: built-in function 5 #10\n"
    );
    let old = file(vec![sub_vi(10, "A.vi", Vec::new())], &[]);
    let new = file(vec![sub_vi(110, "B.vi", Vec::new())], &[]);
    assert_eq!(
        text(&old, &new),
        "1 node added, 1 removed\nadded: subVI `B.vi` #110\nremoved: subVI `A.vi` #10\n"
    );
}

#[test]
fn equal_signatures_are_paired_in_order_of_position() {
    let at = |uid: i32, left: i16| Node {
        bounds: Some(rect(left, 0)),
        ..function(uid, 5, Vec::new())
    };
    let old = file(vec![at(1, 100), at(2, 200), at(3, 300)], &[]);
    // Renumbered in another order; the middle one is gone.
    let new = file(vec![at(31, 300), at(11, 100)], &[]);
    let result = diff(Some(&old), Some(&new), DiffOptions::default());
    assert_eq!(
        to_text(&result),
        "1 node removed, 1 moved\n\
         removed: built-in function 5 #3\n\
         moved: built-in function 5 #31 (200,0 -> 300,0)\n"
    );
}

#[test]
fn a_reused_id_of_another_class_is_not_a_match() {
    let old = file(vec![function(10, 5, Vec::new())], &[]);
    let new = file(vec![sub_vi(10, "A.vi", Vec::new())], &[]);
    assert_eq!(
        text(&old, &new),
        "1 node added, 1 removed\nadded: subVI `A.vi` #10\nremoved: built-in function 5 #10\n"
    );
}

#[test]
fn terminals_with_new_ids_are_matched_by_type_in_order() {
    let old = wiring(&[(11, &[21]), (12, &[23])]);
    let mut new = wiring(&[(11, &[521]), (12, &[523])]);
    for terminal in &mut new.diagram.as_mut().unwrap().root.nodes[1].terminals {
        terminal.uid += 500;
    }
    let result = diff(Some(&old), Some(&new), DiffOptions::default());
    assert!(result.identical, "{}", to_text(&result));
}

#[test]
fn a_missing_side_is_all_added_or_all_removed() {
    let mut new = sample();
    new.metadata.links = vec![link("Dep.vi")];
    let added = diff(None, Some(&new), DiffOptions::default());
    assert_eq!(
        to_text(&added),
        "5 nodes added; 1 wire added; 1 dependency added\n\
         note: old file is missing: everything is reported as added\n\
         added: dependency `../Dep.vi`\n\
         added: diagram boundary #14 in for loop #12\n\
         added: diagram boundary #7\n\
         added: for loop #12\n\
         added: subVI `Open Config Data.vi` #20 in for loop #12\n\
         added: subVI `Read Key.vi` #30 in for loop #12\n\
         added: error cluster wire from subVI `Open Config Data.vi` #20 to \
         subVI `Read Key.vi` #30 in for loop #12\n"
    );
    let removed = diff(Some(&new), None, DiffOptions::default());
    assert_eq!(removed.summary.nodes_removed, 5);
    assert_eq!(removed.summary.wires_removed, 1);
    assert_eq!(removed.summary.dependencies_removed, 1);
    assert_eq!(
        removed.notes,
        ["new file is missing: everything is reported as removed"]
    );
    assert!(!removed.identical);
    assert!(diff(None, None, DiffOptions::default()).identical);
}

#[test]
fn files_without_a_block_diagram_compare_their_settings() {
    let control = || Inspection {
        metadata: metadata(),
        diagram: None,
    };
    assert!(diff(Some(&control()), Some(&control()), DiffOptions::default()).identical);
    let mut locked = control();
    locked.metadata.flags.as_mut().unwrap().locked = Some(true);
    assert_eq!(
        text(&control(), &locked),
        "1 setting changed\nchanged: locked off -> on\n"
    );
}

#[test]
fn a_diagram_on_one_side_only_is_noted() {
    let without = Inspection {
        metadata: metadata(),
        diagram: None,
    };
    let with = file(vec![function(10, 5, Vec::new())], &[]);
    assert_eq!(
        text(&without, &with),
        "1 node added\n\
         note: old file has no readable block diagram: every node and wire is reported as added\n\
         added: built-in function 5 #10\n"
    );
    assert_eq!(
        text(&with, &without),
        "1 node removed\n\
         note: new file has no readable block diagram: every node and wire is reported as removed\n\
         removed: built-in function 5 #10\n"
    );
}

#[test]
fn markdown_groups_changes_by_section() {
    let mut old = sample();
    let mut new = sample();
    old.metadata.links = vec![link("Gone.vi")];
    new.metadata.flags.as_mut().unwrap().auto_error_handling = Some(false);
    inner(&mut new).nodes.remove(2);
    inner(&mut new).wires.clear();
    assert_eq!(
        to_markdown(&diff(Some(&old), Some(&new), DiffOptions::default())),
        "**1 node removed; 1 wire removed; 1 setting changed; 1 dependency removed**\n\
         \n\
         #### Settings\n\
         \n\
         - changed: automatic error handling on -> off\n\
         \n\
         #### Dependencies\n\
         \n\
         - removed: dependency `../Gone.vi`\n\
         \n\
         #### Block diagram\n\
         \n\
         - removed: subVI `Read Key.vi` #30 in for loop #12\n\
         - removed: error cluster wire from subVI `Open Config Data.vi` #20 to \
         subVI `Read Key.vi` #30 in for loop #12\n"
    );
    let note = to_markdown(&diff(
        None,
        Some(&file(Vec::new(), &[])),
        DiffOptions::default(),
    ));
    assert_eq!(
        note,
        "**no differences**\n\n> old file is missing: everything is reported as added\n"
    );
}

#[test]
fn json_mirrors_the_report() {
    let old = wiring(&[(11, &[21])]);
    let mut new = wiring(&[(11, &[22])]);
    new.diagram.as_mut().unwrap().root.nodes[2].bounds = Some(rect(1, 1));
    let result = diff(Some(&old), Some(&new), DiffOptions::default());
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["identical"], false);
    assert_eq!(json["summary"]["nodes_moved"], 1);
    assert_eq!(json["summary"]["wires_changed"], 1);
    assert_eq!(
        json["nodes"]["moved"][0],
        serde_json::json!({
            "node": {
                "uid": 30,
                "class": "prim",
                "description": "built-in function 3",
                "location": [],
            },
            "old_uid": 30,
            "matched_by": "uid",
            "from": { "top": 0, "left": 30, "bottom": 32, "right": 62 },
            "to": { "top": 1, "left": 1, "bottom": 33, "right": 33 },
        })
    );
    assert_eq!(
        json["wires"]["changed"][0],
        serde_json::json!({
            "wire": {
                "data_type": "numeric",
                "from": { "terminal": 11, "node": "built-in function 1 #10" },
                "to": { "terminal": 22, "node": "built-in function 2 #20" },
                "location": [],
            },
            "old_from_terminal": 11,
            "old_to_terminal": 21,
        })
    );
}

#[test]
fn summary_line_counts_and_pluralizes() {
    let summary = Summary {
        settings_changed: 2,
        dependencies_added: 1,
        dependencies_removed: 3,
        nodes_added: 3,
        nodes_removed: 1,
        nodes_changed: 0,
        nodes_moved: 5,
        moves_ignored: 0,
        wires_added: 0,
        wires_removed: 1,
        wires_changed: 2,
    };
    assert_eq!(
        summary.line(),
        "3 nodes added, 1 removed, 5 moved; 1 wire removed, 2 changed; \
         2 settings changed; 1 dependency added, 3 removed"
    );
    let one = Summary {
        dependencies_removed: 2,
        moves_ignored: 1,
        ..Summary::default()
    };
    assert_eq!(one.line(), "2 dependencies removed (1 move ignored)");
}

#[test]
fn output_is_sorted_and_the_same_on_every_run() {
    let old = sample();
    let mut new = renumber(&old, 1000);
    for uid in [9005, 9001, 9003] {
        inner(&mut new).nodes.push(function(uid, 7, Vec::new()));
    }
    let first = diff(Some(&old), Some(&new), DiffOptions::default());
    for _ in 0..20 {
        let again = diff(Some(&old), Some(&new), DiffOptions::default());
        assert_eq!(again, first);
        assert_eq!(
            serde_json::to_string(&again).unwrap(),
            serde_json::to_string(&first).unwrap()
        );
    }
    assert_eq!(
        to_text(&first),
        "3 nodes added\n\
         added: built-in function 7 #9001 in for loop #1012\n\
         added: built-in function 7 #9003 in for loop #1012\n\
         added: built-in function 7 #9005 in for loop #1012\n"
    );
}

#[test]
fn names_cannot_break_the_line_format() {
    let old = file(Vec::new(), &[]);
    let mut new = file(
        vec![sub_vi(10, "evil\nadded: fake\r\t.vi", Vec::new())],
        &[],
    );
    new.metadata.saved_name = Some("a\nb".to_owned());
    new.metadata.links = vec![link("x\ny.vi")];
    let report = text(&old, &new);
    assert_eq!(
        report,
        "1 node added; 1 setting changed; 1 dependency added\n\
         changed: saved name `Main.vi` -> `a b`\n\
         added: dependency `../x y.vi`\n\
         added: subVI `evil added: fake  .vi` #10\n"
    );
}

#[test]
fn hostile_diagrams_do_not_panic() {
    // Duplicate node, terminal and frame ids; wires to terminals nobody
    // owns, to themselves and with extreme ids; nesting 200 deep.
    let mut deep = frame(1, vec![function(10, 5, vec![terminal(11, NUM)])], &[]);
    for level in 0..200 {
        deep = frame(
            1,
            vec![
                structure(10, "whileLoop", vec![deep.clone(), deep]),
                function(10, 5, vec![terminal(11, NUM), terminal(11, STR)]),
                node(level, "", Vec::new()),
            ],
            &[
                (11, &[11, 11, i32::MAX]),
                (i32::MIN, &[]),
                (12345, &[54321, 11]),
            ],
        );
        if level > 6 {
            // Keep the doubling above from exploding: one child from here.
            deep.nodes[0].diagrams.truncate(1);
        }
    }
    let hostile = Inspection {
        metadata: metadata(),
        diagram: Some(BlockDiagram { root: deep }),
    };
    let plain = sample();
    for (old, new) in [
        (&hostile, &hostile),
        (&hostile, &plain),
        (&plain, &hostile),
        (&renumber(&hostile, 7), &hostile),
    ] {
        let result = diff(Some(old), Some(new), DiffOptions::default());
        let _ = (to_text(&result), to_markdown(&result));
        serde_json::to_string(&result).unwrap();
    }
    assert!(diff(Some(&hostile), Some(&hostile), DiffOptions::default()).identical);
    let _ = diff(Some(&hostile), None, DiffOptions { ignore_moves: true });
}

#[test]
fn functions_are_named_where_the_table_knows_them() {
    // 1050 and 3915 are both called Add, so the ids stay in the detail.
    let old = file(vec![function(10, 1050, vec![terminal(11, NUM)])], &[]);
    let new = file(vec![function(10, 3915, vec![terminal(11, NUM)])], &[]);
    assert_eq!(
        text(&old, &new),
        "1 node changed\n\
         changed: built-in function `Add` #10: function id 1050 (Add) -> 3915 (Add)\n"
    );
    let new = file(vec![function(10, 999_999, vec![terminal(11, NUM)])], &[]);
    assert_eq!(
        text(&old, &new),
        "1 node changed\n\
         changed: built-in function 999999 #10: function id 1050 (Add) -> 999999\n"
    );
}

#[test]
fn changed_constants_and_case_labels_are_reported() {
    use buzzard_rsrc::{CaseSelector, SelectorKind, SelectorMatch, Value};

    let constant = |uid: i32, text: &str| Terminal {
        constant: Some(Value::String(text.to_owned())),
        dco_class: Some("bDConstDCO".to_owned()),
        ..terminal(uid, STR)
    };
    let case = |labels: &[&str]| Node {
        selector: Some(CaseSelector {
            kind: SelectorKind::String,
            enum_items: [].into(),
            type_definition: None,
            frames: labels
                .iter()
                .map(|label| vec![SelectorMatch::Text((*label).to_owned())])
                .collect(),
            default_frame: Some(0),
        }),
        ..structure(
            20,
            "select",
            (0..labels.len() as i32)
                .map(|i| frame(30 + i, Vec::new(), &[]))
                .collect(),
        )
    };
    let boundary = |values: &[&str]| {
        let terminals = values.iter().zip(11..).map(|(v, uid)| constant(uid, v));
        node(10, "sRN", terminals.collect())
    };
    let old = file(
        vec![boundary(&["Init", "Idle", "Idle"]), case(&["Init", "Idle"])],
        &[],
    );
    let new = file(
        vec![boundary(&["Init", "Idle", "Exit"]), case(&["Init", "Exit"])],
        &[],
    );
    assert_eq!(
        text(&old, &new),
        "2 nodes changed\n\
         changed: case structure #20: case labels [\"Init\", Default; \"Idle\"] -> \
         [\"Init\", Default; \"Exit\"]\n\
         changed: diagram boundary #10: constants -\"Idle\" +\"Exit\"\n"
    );
    assert!(diff(Some(&old), Some(&old), DiffOptions::default()).identical);
}
