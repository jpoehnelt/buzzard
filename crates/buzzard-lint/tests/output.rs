//! Two properties of what buzzard prints, on real files.
//!
//! The summary of an inspection stands for the whole JSON and says how
//! large that is, so a reader can decide before it asks. And a file name
//! is data: no report for a person lets one add a line or reach the
//! terminal as an escape sequence, while JSON holds the name itself.

use std::path::{Path, PathBuf};

use buzzard_lint::verify::report::{self, Verified};
use buzzard_lint::{
    budget, check, lint, summary, DependencyGraph, Inspection, LintConfig, Workspace,
};
use buzzard_rsrc::builder::RsrcBuilder;
use buzzard_rsrc::LvPath;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify/jki-smo/process.vi")
}

fn inspection() -> Inspection {
    Inspection::from_path(&fixture()).expect("parse the fixture")
}

/// A name that would add a finding line to a report, and one that would
/// clear a terminal.
const FORGED: &str = "a.vi\nforged.vi: warning[BZ001]: compiled code is stored in the file";
const ESCAPE: &str = "\u{1b}[2Jb.vi";

#[test]
fn the_summary_is_small_and_says_how_large_the_whole_json_is() {
    let inspection = inspection();
    let full = inspection.full_json();
    assert_eq!(inspection.full_json_bytes(), full.len() as u64);
    assert_eq!(
        budget::json_bytes(&inspection.full(), true) + 1,
        full.len() as u64
    );

    let summary = inspection.summary();
    assert_eq!(summary.detail, "summary");
    assert_eq!(summary.full_json_bytes, full.len() as u64);
    let json = summary.to_json();
    // The whole JSON of this file is over a megabyte, more than 30 times
    // its summary, which fits the default budget twice over.
    assert!(full.len() > 1_000_000, "{}", full.len());
    assert!(json.len() < budget::DEFAULT_MAX_BYTES / 2, "{}", json.len());
    assert!(json.len() * 30 < full.len(), "{}", json.len());
    assert_eq!(json, inspection.summary().to_json());

    // The summary has every field of the metadata, as the whole JSON
    // has them, and the outline in place of the diagram.
    let whole: serde_json::Value = serde_json::from_str(&full).unwrap();
    let short: serde_json::Value = serde_json::from_str(&json).unwrap();
    for (key, value) in whole.as_object().unwrap() {
        if key != "diagram" {
            assert_eq!(&short[key], value, "{key}");
        }
    }
    assert!(whole["diagram"]["root"]["nodes"].is_array());
    assert!(short["diagram"]["root"].is_null());
    assert_eq!(short["detail"], "summary");
    assert_eq!(short["full_json_bytes"], full.len());
}

#[test]
fn the_outline_counts_what_the_diagram_holds() {
    let inspection = inspection();
    let diagram = inspection.diagram.as_ref().unwrap();
    let outline = summary::outline(diagram);
    assert_eq!(outline.nodes, diagram.nodes().count());
    assert_eq!(outline.wires, diagram.wires().count());
    assert_eq!(outline.diagrams, diagram.diagrams().len());
    assert_eq!(
        outline.terminals,
        diagram
            .nodes()
            .map(|node| node.terminals.len())
            .sum::<usize>()
    );
    assert_eq!(outline.node_classes.values().sum::<usize>(), outline.nodes);
    assert!(outline.complete);
    assert_eq!(outline.losses, Vec::new());
    let (low, high) = outline.node_ids.unwrap();
    assert!(diagram.nodes().all(|node| (low..=high).contains(&node.uid)));

    // Every frame of every structure is a diagram, and the top-level
    // diagram is the one more.
    let frames: usize = outline.structures.iter().map(|s| s.frames.len()).sum();
    assert_eq!(frames + 1, outline.diagrams);
    let nodes_in_frames: usize = outline
        .structures
        .iter()
        .flat_map(|s| &s.frames)
        .map(|frame| frame.nodes)
        .sum();
    assert_eq!(nodes_in_frames + diagram.root.nodes.len(), outline.nodes);

    // The state machine of the fixture: a while loop at the top with a
    // case structure in it.
    let outer = &outline.structures[0];
    assert_eq!((outer.depth, outer.parent), (0, None));
    let cases = outline
        .structures
        .iter()
        .find(|s| s.uid == 3023)
        .expect("the case structure of the states");
    assert_eq!(cases.description, "case structure");
    assert_eq!(cases.class, "select");
    let labels: Vec<&str> = cases.frames.iter().map(|f| f.label.as_str()).collect();
    assert_eq!(labels, ["\"Init\"", "\"Idle\", Default"]);
    let indexes: Vec<usize> = cases.frames.iter().map(|f| f.index).collect();
    assert_eq!(indexes, [0, 1]);
    assert!(cases.depth > 0);
    let parent = cases.parent.expect("a nested structure has a parent");
    assert!(outline.structures.iter().any(|s| s.uid == parent));
    for structure in &outline.structures {
        match structure.parent {
            None => assert_eq!(structure.depth, 0),
            Some(_) => assert!(structure.depth > 0),
        }
    }

    // Callees are named once, with the number of calls.
    let subvi_calls = diagram
        .nodes()
        .filter(|n| n.name.is_some() && matches!(n.class.as_str(), "iUse" | "polyIUse" | "dynIUse"))
        .count();
    assert!(subvi_calls > 0);
    assert_eq!(
        outline.callees.iter().map(|c| c.calls).sum::<usize>(),
        subvi_calls
    );
    let mut sorted = outline.callees.clone();
    sorted.sort();
    sorted.dedup_by(|a, b| a.name == b.name && a.class == b.class);
    assert_eq!(sorted, outline.callees);
}

#[test]
fn a_frame_and_a_range_of_nodes_are_parts_of_the_diagram() {
    let inspection = inspection();
    let diagram = inspection.diagram.as_ref().unwrap();
    let outline = summary::outline(diagram);
    for structure in &outline.structures {
        for frame in &structure.frames {
            let part = summary::frame(diagram, structure.uid, frame.index)
                .expect("a frame of the outline");
            assert_eq!(part.nodes.len(), frame.nodes);
            assert_eq!(part.wires.len(), frame.wires);
        }
        assert!(summary::frame(diagram, structure.uid, structure.frames.len()).is_none());
    }
    assert!(summary::frame(diagram, i32::MIN, 0).is_none());
    // One frame is much smaller than the diagram.
    let one = summary::frame(diagram, 3023, 0).unwrap();
    assert!(budget::json_bytes(one, true) < budget::json_bytes(diagram, true) / 2);

    let (low, high) = outline.node_ids.unwrap();
    assert_eq!(
        summary::nodes_in_range(diagram, low, high).len(),
        outline.nodes
    );
    let cases = summary::nodes_in_range(diagram, 3023, 3023);
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].class, "select");
    assert!(summary::nodes_in_range(diagram, high, low).is_empty() || low == high);
    assert_eq!(summary::nodes_in_range(diagram, i32::MIN, low - 1).len(), 0);
    assert_eq!(
        summary::nodes_in_range(diagram, i32::MIN, i32::MAX).len(),
        outline.nodes
    );
}

#[test]
fn a_file_without_a_block_diagram_has_a_summary_without_an_outline() {
    let bytes = RsrcBuilder::default()
        .save_record(0x2300_8000, &[0, 0x400])
        .build();
    let inspection = Inspection::from_bytes(&bytes).unwrap();
    let summary = inspection.summary();
    assert_eq!(summary.diagram, None);
    assert_eq!(summary.full_json_bytes, inspection.full_json().len() as u64);
    let json: serde_json::Value = serde_json::from_str(&summary.to_json()).unwrap();
    assert!(json["diagram"].is_null());
    assert!(json.get("version").is_some());
}

/// A workspace whose files have the hostile names. The files are in
/// memory only, because Windows has no file of such a name.
fn hostile_workspace() -> Workspace {
    let dir = Path::new("dir");
    let vi = |callee: &str| {
        let bytes = RsrcBuilder::default()
            .save_record(0x2300_8000, &[0, 0])
            .links(&[(callee, LvPath::sibling(callee))])
            .build();
        buzzard_lint::FileState::from_inspection(Inspection::from_bytes(&bytes))
    };
    let mut workspace = Workspace::default();
    // Each calls the other, so both are in a cycle that names them.
    workspace.insert(&dir.join(FORGED), vi(ESCAPE));
    workspace.insert(&dir.join(ESCAPE), vi(FORGED));
    workspace.insert(
        &dir.join("c.vi\rd.vi"),
        buzzard_lint::FileState::Failed("no such file".to_owned()),
    );
    workspace
}

#[test]
fn a_file_name_cannot_forge_a_line_of_the_check_report() {
    let workspace = hostile_workspace();
    let graph = DependencyGraph::build(&workspace);
    let diagnostics = lint(&workspace, &graph, &LintConfig::default());
    assert!(
        diagnostics.iter().any(|d| d.code == "BZ005"),
        "{diagnostics:?}"
    );
    assert!(diagnostics.iter().any(|d| d.code == "BZ000"));
    let report = check::to_human(&diagnostics, workspace.files().len());

    // One line per finding and the summary line, and nothing else.
    assert_eq!(report.lines().count(), diagnostics.len() + 1, "{report}");
    assert!(!report.contains(['\u{1b}', '\r']));
    let dir = format!("dir{}", std::path::MAIN_SEPARATOR);
    for line in report.lines().take(diagnostics.len()) {
        assert!(line.starts_with(&dir), "{line}");
    }
    assert!(report.contains("a.vi\\nforged.vi: warning[BZ001]: compiled code"));
    assert!(report.contains("\\u{1b}[2Jb.vi"));
    assert!(report.contains("c.vi\\rd.vi: error[BZ000]"));
    // The cycle names its files in the message, on one line too.
    let cycle = diagnostics.iter().find(|d| d.code == "BZ005").unwrap();
    assert_eq!(cycle.message.lines().count(), 1, "{}", cycle.message);
    assert!(!cycle.message.contains('\u{1b}'));

    // JSON and SARIF hold the names themselves, escaped as JSON.
    let json = serde_json::to_string_pretty(&diagnostics).unwrap();
    assert!(!json.contains('\u{1b}'));
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let paths: Vec<&str> = parsed
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["path"].as_str().unwrap())
        .collect();
    assert!(paths.iter().any(|p| p.ends_with(FORGED)));
    assert!(paths.iter().any(|p| p.ends_with(ESCAPE)));
    let sarif = buzzard_lint::sarif::to_sarif(&workspace, &diagnostics, "0.0.0");
    let sarif = serde_json::to_string_pretty(&sarif).unwrap();
    assert!(!sarif.contains('\u{1b}'));
    assert!(sarif.contains("a.vi%0Aforged.vi"));
}

#[test]
fn a_file_name_cannot_forge_a_line_of_the_verify_report() {
    let verified = Verified::new(vec![
        (PathBuf::from("dir").join(FORGED), Ok(None)),
        (
            PathBuf::from(ESCAPE),
            Err("the file\nis truncated".to_owned()),
        ),
    ]);
    let honest = Verified::new(vec![
        (PathBuf::from("dir").join("a.vi"), Ok(None)),
        (
            PathBuf::from("b.vi"),
            Err("the file is truncated".to_owned()),
        ),
    ]);
    let text = report::to_text(&verified);
    assert_eq!(
        text.lines().count(),
        report::to_text(&honest).lines().count(),
        "{text}"
    );
    assert!(!text.contains('\u{1b}'));
    assert!(text.contains("\\u{1b}[2Jb.vi: cannot analyze file: the file\\nis truncated"));
    let dot = report::to_dot(&verified);
    assert!(!dot.contains('\u{1b}'));
    assert_eq!(dot.lines().count(), report::to_dot(&honest).lines().count());
    // JSON holds the name itself.
    let json = serde_json::to_string(&verified).unwrap();
    assert!(!json.contains('\u{1b}'));
    assert!(json.contains("\\u001b[2Jb.vi"));
}

#[test]
fn a_file_name_cannot_forge_a_line_of_a_summary() {
    let inspection = inspection();
    for name in [FORGED, ESCAPE] {
        let hostile = summary::to_markdown(name, &inspection.metadata, None);
        let honest = summary::to_markdown("a.vi", &inspection.metadata, None);
        assert_eq!(hostile.lines().count(), honest.lines().count());
        assert!(!hostile.contains('\u{1b}'));
    }
    assert!(
        summary::to_markdown(FORGED, &inspection.metadata, None).starts_with(
            "### a.vi\\nforged.vi: warning[BZ001]: compiled code is stored in the file\n"
        )
    );
    // A name without a control character is printed as it is.
    assert!(
        summary::to_markdown("My VI #1 50%.vi", &inspection.metadata, None)
            .starts_with("### My VI #1 50%.vi\n")
    );
}
