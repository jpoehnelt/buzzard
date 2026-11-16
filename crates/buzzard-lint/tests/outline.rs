//! The diagram exports show case labels, and the outline constant values
//! (a constant has no position of its own in the decoded diagram, so the
//! pictures do not draw it). Checked on a
//! real file; `tests/fixtures/verify/README.md` records its source.

use std::path::Path;

use buzzard_lint::{render, summary, Inspection};

fn diagram() -> buzzard_rsrc::BlockDiagram {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify/jki-smo/process.vi");
    Inspection::from_path(&path)
        .expect("parse the fixture")
        .diagram
        .expect("the fixture has a block diagram")
}

#[test]
fn markdown_outline_names_cases_and_constants() {
    let text = summary::diagram_to_markdown(&diagram());
    // The state case structure and an enumeration one nested in it.
    assert!(text.contains("- frame 0: \"Init\"\n"));
    assert!(text.contains("- frame 1: \"Idle\", Default\n"));
    assert!(text.contains("- frame 2: \"Stopped\"\n"));
    assert!(text.contains("- frame 0: No Error\n"));
    // A loop has one unnamed frame.
    assert!(text.contains("- frame 0\n"));
    assert!(text.contains("- out #4888 string constant \"Init\" -> while loop #3379"));
    assert!(text.contains(" numeric constant -1 -> "));
    assert!(text.contains(" boolean constant True -> "));
    assert_eq!(text, summary::diagram_to_markdown(&diagram()));
}

#[test]
fn pictures_name_cases() {
    let diagram = diagram();
    let svg = render::to_svg(&diagram);
    assert!(svg.contains(">case structure #3023, frame 2 of 2: &quot;Idle&quot;, Default</text>"));
    let dot = render::to_dot(&diagram);
    assert!(dot.contains("label=\"frame 2: \\\"Idle\\\", Default\";"));
    assert!(dot.contains("label=\"frame 1\";"));
    let mermaid = render::to_mermaid(&diagram);
    assert!(mermaid.contains("[\"frame 2: 'Idle', Default\"]"));
    for draw in [render::to_svg, render::to_dot, render::to_mermaid] {
        assert_eq!(draw(&diagram), draw(&diagram));
    }
}
