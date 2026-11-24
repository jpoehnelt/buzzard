//! The `inspect` subcommand: what one file holds.

use std::fmt::Write as _;
use std::path::Path;

use buzzard_lint::project::is_project_file;
use buzzard_lint::summary::{self, DiagramOutline};
use buzzard_lint::{archive, patharg, printable, render, ErrorKind, Failure};
use buzzard_lint::{Inspection, ProjectFile};
use buzzard_rsrc::{BlockDiagram, ViMetadata};
use clap::ValueEnum;

use crate::{to_json, Outcome, Report};

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Format {
    /// A summary of the file.
    Markdown,
    /// What buzzard reads from the file, as JSON.
    Json,
    /// The block diagram as a picture, laid out as in LabVIEW.
    Svg,
    /// The block diagram as a Graphviz graph.
    Dot,
    /// The block diagram as a Mermaid flowchart.
    Mermaid,
}

impl Format {
    fn is_picture(self) -> bool {
        matches!(self, Self::Svg | Self::Dot | Self::Mermaid)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Detail {
    /// The metadata, the counts, and an outline of the block diagram: its
    /// structures, node classes, callees, and losses. A small part of
    /// the full JSON.
    Summary,
    /// Every node, terminal, and wire of the block diagram.
    Full,
}

/// `buzzard inspect`: a project file, the directory of an archive, or a
/// binary file. A member path is tried first, because `x.llb/p.lvlib` is a
/// member of an archive and the member's name decides how it is read.
pub(crate) fn run(file: &Path, format: Format, detail: Detail) -> Outcome {
    patharg::check(file)?;
    if detail == Detail::Summary && format.is_picture() {
        return Err(Failure::bad_argument(
            "--detail summary applies to --format json and markdown; svg, dot, and mermaid \
             draw the whole block diagram",
        ));
    }
    // A member is shown under the name its archive has for it.
    let canonical = archive::canonical(file);
    let file = canonical.as_deref().unwrap_or(file);
    let is_member = archive::split(file).is_some();
    if is_project_file(file) && (is_member || file.is_file()) {
        project(file, format)
    } else if !is_member && archive::is_archive(file) && file.is_file() {
        archive(file, format, detail)
    } else if is_project_file(file) {
        project(file, format)
    } else {
        binary(file, format, detail)
    }
}

/// The error for a file that cannot be read or does not parse. A path that
/// names nothing has its own kind, with the same message.
fn unreadable(file: &Path, reason: impl std::fmt::Display) -> Failure {
    let mut failure = Failure::unreadable(file, reason);
    if !archive::exists(file) {
        failure.kind = ErrorKind::PathNotFound;
    }
    failure
}

/// `buzzard inspect` on a `.lvproj`, `.lvlib`, or `.lvclass` file.
fn project(file: &Path, format: Format) -> Outcome {
    let project = ProjectFile::from_path(file).map_err(|err| unreadable(file, err))?;
    let text = match format {
        Format::Json => to_json(&project),
        Format::Markdown => summary::project_to_markdown(&project.name, &project, None),
        Format::Svg | Format::Dot | Format::Mermaid => {
            return Err(Failure::unsupported(
                file,
                "a project file has no block diagram",
            ));
        }
    };
    Ok(Report::ok(text))
}

/// JSON shape of `inspect` on an `.llb` archive: what the file is, as for
/// any resource file, and its directory.
#[derive(serde::Serialize)]
struct ArchiveOutput<'a> {
    #[serde(flatten)]
    metadata: &'a ViMetadata,
    #[serde(flatten)]
    archive: &'a archive::ArchiveFile,
}

/// `buzzard inspect` on an `.llb` archive: the list of its members. A
/// member is inspected by the path of the archive followed by its name.
fn archive(file: &Path, format: Format, detail: Detail) -> Outcome {
    let opened = match archive::list(file) {
        Ok(Some(listing)) => listing,
        Ok(None) => {
            eprintln!(
                "note: {}: the archive was built into an application and has no directory of \
                 members; it is shown as the resource file it is",
                printable::path(file)
            );
            return binary(file, format, detail);
        }
        Err(err) => return Err(unreadable(file, err)),
    };
    let text = match format {
        Format::Json => {
            let inspection = Inspection::from_path(file).map_err(|err| unreadable(file, err))?;
            to_json(&ArchiveOutput {
                metadata: &inspection.metadata,
                archive: &opened,
            })
        }
        Format::Markdown => summary::archive_to_markdown(&title(file), &opened),
        Format::Svg | Format::Dot | Format::Mermaid => {
            let example = opened.opened().first().map_or_else(
                || "<member>".to_owned(),
                |name| printable::path(&archive::member_path(file, name)),
            );
            return Err(Failure::unsupported(
                file,
                format!(
                    "an archive has no block diagram; name one of its members, as in \
                     `buzzard inspect \"{example}\"`"
                ),
            ));
        }
    };
    Ok(Report::ok(text))
}

/// The last component of `file`, as the title of a Markdown report. The
/// report escapes it.
fn title(file: &Path) -> String {
    file.file_name()
        .unwrap_or(file.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// `buzzard inspect` on a binary file: its metadata and block diagram, or a
/// picture of the diagram. `file` may name a member of an archive.
fn binary(file: &Path, format: Format, detail: Detail) -> Outcome {
    let inspection = Inspection::from_path(file).map_err(|err| unreadable(file, err))?;
    let picture = |draw: fn(&BlockDiagram) -> String| match &inspection.diagram {
        Some(diagram) => Ok(draw(diagram)),
        None => Err(Failure::unsupported(file, "file has no block diagram")),
    };
    let text = match (format, detail) {
        (Format::Svg, _) => picture(render::to_svg)?,
        (Format::Dot, _) => picture(render::to_dot)?,
        (Format::Mermaid, _) => picture(render::to_mermaid)?,
        (Format::Json, Detail::Full) => inspection.full_json(),
        (Format::Json, Detail::Summary) => inspection.summary().to_json(),
        (Format::Markdown, detail) => {
            let mut text = summary::to_markdown(&title(file), &inspection.metadata, None);
            if let Some(diagram) = &inspection.diagram {
                text.push('\n');
                text.push_str(&match detail {
                    Detail::Full => summary::diagram_to_markdown(diagram),
                    Detail::Summary => outline_to_markdown(&summary::outline(diagram)),
                });
            }
            text
        }
    };
    Ok(Report::ok(text))
}

/// A name or label from the file, on one line and inside a code span.
fn code(text: &str) -> String {
    format!("`{}`", printable::text(text).replace('`', "'"))
}

/// The outline of a block diagram as Markdown: what `--detail summary`
/// prints in place of the list of every node.
fn outline_to_markdown(outline: &DiagramOutline) -> String {
    let mut out = String::from("## Block diagram\n\n");
    let _ = writeln!(
        out,
        "{} nodes, {} terminals, {} wires, {} diagrams.",
        outline.nodes, outline.terminals, outline.wires, outline.diagrams
    );
    if let Some((first, last)) = outline.node_ids {
        let _ = writeln!(out, "Node ids {first} to {last}.");
    }
    if !outline.complete {
        let losses: Vec<String> = outline
            .losses
            .iter()
            .map(|loss| match serde_json::to_value(loss) {
                Ok(serde_json::Value::String(name)) => name,
                _ => format!("{loss:?}"),
            })
            .collect();
        let _ = writeln!(
            out,
            "\nThe diagram did not decode completely, so a missing node or wire proves \
             nothing. Losses: {}.",
            losses.join(", ")
        );
    }
    if !outline.node_classes.is_empty() {
        out.push_str("\n### Node classes\n\n");
        for (class, count) in &outline.node_classes {
            let _ = writeln!(out, "- {}: {count}", code(class));
        }
    }
    if !outline.callees.is_empty() {
        out.push_str("\n### Callees\n\n");
        for callee in &outline.callees {
            let _ = writeln!(
                out,
                "- {} ({}): {} call{}",
                code(&callee.name),
                code(&callee.class),
                callee.calls,
                if callee.calls == 1 { "" } else { "s" }
            );
        }
    }
    if !outline.structures.is_empty() {
        out.push_str("\n### Structures\n\n");
        for structure in &outline.structures {
            // A damaged file may nest deeper than a list can show.
            let indent = "  ".repeat(structure.depth.min(16));
            let _ = writeln!(
                out,
                "{indent}- {} #{} ({}), {} frame{}",
                printable::text(&structure.description),
                structure.uid,
                code(&structure.class),
                structure.frames.len(),
                if structure.frames.len() == 1 { "" } else { "s" }
            );
            for frame in &structure.frames {
                let label = if frame.label.is_empty() {
                    String::new()
                } else {
                    format!(" {}", code(&frame.label))
                };
                let _ = writeln!(
                    out,
                    "{indent}  - frame {}{label}: {} nodes, {} wires",
                    frame.index, frame.nodes, frame.wires
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use buzzard_lint::summary::{CalleeOutline, FrameOutline, StructureOutline};

    use super::*;

    #[test]
    fn the_outline_puts_every_name_from_the_file_on_one_line() {
        let outline = DiagramOutline {
            nodes: 3,
            terminals: 4,
            wires: 2,
            diagrams: 2,
            complete: true,
            losses: Vec::new(),
            node_ids: Some((7, 90)),
            node_classes: [("prim".to_owned(), 2), ("a\nb".to_owned(), 1)].into(),
            callees: vec![CalleeOutline {
                name: "Init.vi\n## Forged".to_owned(),
                class: "iUse".to_owned(),
                calls: 1,
            }],
            structures: vec![StructureOutline {
                uid: 12,
                class: "select".to_owned(),
                description: "case structure".to_owned(),
                depth: 1,
                parent: Some(3),
                frames: vec![FrameOutline {
                    index: 0,
                    label: "\"Idle\"\u{1b}[31m`".to_owned(),
                    nodes: 2,
                    wires: 1,
                }],
            }],
        };
        assert_eq!(
            outline_to_markdown(&outline),
            "## Block diagram\n\n\
             3 nodes, 4 terminals, 2 wires, 2 diagrams.\n\
             Node ids 7 to 90.\n\
             \n### Node classes\n\n\
             - `a\\nb`: 1\n\
             - `prim`: 2\n\
             \n### Callees\n\n\
             - `Init.vi\\n## Forged` (`iUse`): 1 call\n\
             \n### Structures\n\n\
             \x20 - case structure #12 (`select`), 1 frame\n\
             \x20   - frame 0 `\"Idle\"\\u{1b}[31m'`: 2 nodes, 1 wires\n"
        );
    }
}
