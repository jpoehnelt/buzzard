//! The `inspect` subcommand: what one file holds.

use std::fmt::Write;
use std::path::Path;

use buzzard_lint::project::is_project_file;
use buzzard_lint::summary;
use buzzard_lint::{archive, html, patharg, printable, render, ErrorKind, Failure};
use buzzard_lint::{lint, DependencyGraph, LintConfig, Workspace};
use buzzard_lint::{Inspection, ProjectFile};
use buzzard_rsrc::{BlockDiagram, ViMetadata};
use clap::ValueEnum;
use sha2::{Digest, Sha256};

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
    /// One HTML page that holds everything it shows: the SHA-256 of the
    /// file, its metadata, a picture of the block diagram, the losses of
    /// the decoder, and the problems that check finds in the file alone.
    Html,
}

impl Format {
    fn is_picture(self) -> bool {
        matches!(self, Self::Svg | Self::Dot | Self::Mermaid)
    }

    /// Whether the format shows the whole block diagram, so that
    /// `--detail summary` does not apply to it.
    fn is_whole(self) -> bool {
        self.is_picture() || matches!(self, Self::Html)
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

/// A part of a block diagram that `inspect` prints in place of the whole:
/// the step between the outline and the full JSON.
#[derive(Clone, Copy)]
pub(crate) enum Part {
    /// One frame of one structure, as the outline names them.
    Frame { structure: i32, frame: usize },
    /// The nodes whose id is in a range, both ends included.
    Nodes { first: i32, last: i32 },
}

/// `buzzard inspect`: a project file, the directory of an archive, or a
/// binary file. A member path is tried first, because `x.llb/p.lvlib` is a
/// member of an archive and the member's name decides how it is read.
pub(crate) fn run(file: &Path, format: Format, detail: Detail, part: Option<Part>) -> Outcome {
    patharg::check(file)?;
    if detail == Detail::Summary && format.is_whole() {
        return Err(Failure::bad_argument(
            "--detail summary applies to --format json and markdown; svg, dot, mermaid, and \
             html show the whole block diagram",
        ));
    }
    if part.is_some() && !matches!(format, Format::Json) {
        return Err(Failure::bad_argument(
            "a frame or a range of nodes is printed as JSON; pass --format json",
        ));
    }
    if matches!(part, Some(Part::Nodes { .. })) && detail == Detail::Summary {
        return Err(Failure::bad_argument(
            "--detail summary applies to the whole diagram or to one frame, not to a range \
             of nodes",
        ));
    }
    if file.is_dir() {
        return Err(Failure::not_a_file(file));
    }
    // A member is shown under the name its archive has for it.
    let canonical = archive::canonical(file);
    let file = canonical.as_deref().unwrap_or(file);
    let is_member = archive::split(file).is_some();
    let is_project = is_project_file(file) && (is_member || file.is_file());
    let is_listing = !is_member && archive::is_archive(file) && file.is_file();
    if let Some(part) = part {
        return if is_project || (is_listing && archive::list(file).is_ok_and(|l| l.is_some())) {
            Err(Failure::unsupported(
                file,
                "a project file or an archive has no block diagram",
            ))
        } else {
            selected(file, part, detail)
        };
    }
    if is_project {
        project(file, format)
    } else if is_listing {
        archive(file, format, detail)
    } else if is_project_file(file) {
        project(file, format)
    } else {
        binary(file, format, detail)
    }
}

/// One frame or a range of nodes of the block diagram of `file`, as JSON.
fn selected(file: &Path, part: Part, detail: Detail) -> Outcome {
    let inspection = Inspection::from_path(file).map_err(|err| unreadable(file, err))?;
    let Some(diagram) = &inspection.diagram else {
        return Err(Failure::unsupported(file, "file has no block diagram"));
    };
    Ok(Report::ok(match part {
        Part::Frame { structure, frame } => {
            let Some(found) = summary::frame(diagram, structure, frame) else {
                return Err(Failure::bad_argument(format!(
                    "{}: no structure with the id {structure} has a frame {frame}; --detail \
                     summary lists the structures and their frames",
                    printable::path(file)
                ))
                .with_path(file));
            };
            match detail {
                Detail::Full => to_json(found),
                // The outline of a frame is the outline of the diagram
                // that the frame is.
                Detail::Summary => to_json(&summary::outline(&BlockDiagram {
                    root: found.clone(),
                })),
            }
        }
        Part::Nodes { first, last } => to_json(&summary::nodes_in_range(diagram, first, last)),
    }))
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
        Format::Svg | Format::Dot | Format::Mermaid | Format::Html => {
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
        Format::Svg | Format::Dot | Format::Mermaid | Format::Html => {
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
    // `Inspection::from_path`, with the bytes kept for the hash.
    let bytes = archive::read(file).map_err(|err| unreadable(file, err))?;
    let inspection = Inspection::from_bytes(&bytes).map_err(|err| unreadable(file, err))?;
    let picture = |draw: fn(&BlockDiagram) -> String| match &inspection.diagram {
        Some(diagram) => Ok(draw(diagram)),
        None => Err(Failure::unsupported(file, "file has no block diagram")),
    };
    let text = match (format, detail) {
        (Format::Html, _) => report(file, &bytes, &inspection),
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
                    Detail::Summary => summary::outline_to_markdown(&summary::outline(diagram)),
                });
            }
            text
        }
    };
    Ok(Report::ok(text))
}

/// `buzzard inspect --format html`: one page about one binary file. The
/// hash is that of `bytes`, the bytes read, which for a member are the
/// member's own bytes. The findings are those of the file checked alone.
fn report(file: &Path, bytes: &[u8], inspection: &Inspection) -> String {
    let sha256: String =
        Sha256::digest(bytes)
            .iter()
            .fold(String::with_capacity(64), |mut hex, byte| {
                let _ = write!(hex, "{byte:02x}");
                hex
            });
    let workspace = Workspace::scan(&[file.to_path_buf()]);
    let graph = DependencyGraph::build(&workspace);
    let findings = lint(&workspace, &graph, &LintConfig::default());
    html::to_html(&html::Page {
        path: file,
        sha256: &sha256,
        metadata: &inspection.metadata,
        diagram: inspection.diagram.as_ref(),
        findings: &findings,
    })
}
