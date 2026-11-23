//! The `buzzard` command: `check`, `inspect`, `deps`, `diff` and `verify` over the
//! analysis in `buzzard-lint`, `lsp` for the language server in `buzzard-ls`, `mcp`
//! for the Model Context Protocol server in `buzzard-mcp`, and `tdms` for
//! the measurement file reader in `buzzard-tdms`.

#![expect(
    clippy::print_stderr,
    reason = "a command-line tool reports its errors on stderr"
)]

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use buzzard_lint::archive;
use buzzard_lint::project::is_project_file;
use buzzard_lint::verify::report::{self, Verified};
use buzzard_lint::Inspection;
use buzzard_lint::{
    lint, render, sarif, summary, DependencyGraph, Diagnostic, LintConfig, ProjectFile, Severity,
    Workspace,
};
use buzzard_rsrc::{BlockDiagram, ViMetadata};
use clap::{Parser, Subcommand, ValueEnum};

mod deps_cmd;
mod diff_cmd;
mod tdms_cmd;

// `about` is the `description` of `Cargo.toml`.
#[derive(Parser)]
#[command(name = "buzzard", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Analyze files and directories and report problems.
    #[command(
        after_help = "Exit status: 0 when no problems are found or --exit-zero is set, \
        1 when problems are found, 2 for an unknown --ignore code, a path that does not exist, \
        or a report that cannot be written."
    )]
    Check {
        /// Files, directories, or member paths such as lib/util.llb/Init.vi.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = CheckFormat::Human)]
        format: CheckFormat,
        /// Rule code to skip, such as BZ004. May be repeated.
        #[arg(long, value_name = "CODE")]
        ignore: Vec<String>,
        /// Longest allowed path in characters.
        #[arg(long, default_value_t = 260)]
        max_path_length: usize,
        /// Exit with status 0 even when problems are found.
        #[arg(long)]
        exit_zero: bool,
    },
    /// Print what is known about a single file.
    #[command(
        after_help = "Exit status: 0, or 2 when the file cannot be read or has no block \
        diagram for the svg, dot or mermaid format."
    )]
    Inspect {
        /// A file, or a member path such as lib/util.llb/Init.vi.
        file: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = InspectFormat::Markdown)]
        format: InspectFormat,
    },
    /// List what the files need from outside the project.
    ///
    /// The report holds the installed files (under the symbolic roots
    /// vilib, instrlib, userlib and so on) grouped by package, the
    /// machine-absolute links, and the relative links whose file does not
    /// exist.
    #[command(
        after_help = "Exit status: 0 whatever the report holds, 2 for a path that does \
        not exist or a --root that cannot be used."
    )]
    Deps(deps_cmd::Args),
    /// Compare two versions of a file: settings, dependencies, nodes, wires.
    #[command(
        after_help = "Exit status: 0 when the two files are the same, 1 when they \
        differ, 2 on an error. With --git the status is always 0."
    )]
    Diff(diff_cmd::Args),
    /// Find the state machines on block diagrams and check them.
    ///
    /// The findings are states that can never be reached, loops that their
    /// condition terminal can never stop, and constants that no case
    /// handles. Each finding is labelled proven, structural or heuristic.
    #[command(
        after_help = "Exit status: 0 when there are no findings, 1 when there are any, \
        of any label, 2 on an error such as a path that does not exist. A file that cannot be parsed is listed \
        and does not change the status."
    )]
    Verify {
        /// Files, directories, or member paths such as lib/util.llb/Init.vi.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = VerifyFormat::Text)]
        format: VerifyFormat,
    },
    /// Describe a TDMS measurement file, or export its channels as a
    /// Parquet or CSV table.
    Tdms(tdms_cmd::Args),
    /// Run the language server over stdio.
    #[command(
        after_help = "Exit status: 0 after shutdown and then exit, 1 otherwise, 2 when \
        the server cannot start."
    )]
    Lsp,
    /// Run the Model Context Protocol server over stdio.
    ///
    /// AI agents call its tools to run the analysis.
    Mcp,
}

#[derive(Clone, Copy, ValueEnum)]
enum CheckFormat {
    Human,
    Json,
    Sarif,
}

#[derive(Clone, Copy, ValueEnum)]
enum VerifyFormat {
    Text,
    Json,
    /// The transition graphs as a Graphviz graph.
    Dot,
}

#[derive(Clone, Copy, ValueEnum)]
enum InspectFormat {
    Markdown,
    Json,
    /// Picture of the block diagram, laid out as in the editor.
    Svg,
    /// Block diagram as a Graphviz graph.
    Dot,
    /// Block diagram as a Mermaid flowchart.
    Mermaid,
}

/// JSON shape of `inspect`: the metadata fields plus the block diagram.
#[derive(serde::Serialize)]
struct InspectOutput<'a> {
    #[serde(flatten)]
    metadata: &'a ViMetadata,
    diagram: Option<&'a BlockDiagram>,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Check {
            paths,
            format,
            ignore,
            max_path_length,
            exit_zero,
        } => check(&paths, format, ignore, max_path_length, exit_zero),
        Command::Inspect { file, format } => {
            // A member is shown under the name its archive has for it.
            inspect(&archive::canonical(&file).unwrap_or(file), format)
        }
        Command::Verify { paths, format } => verify(&paths, format),
        Command::Deps(args) => match deps_cmd::run(&args) {
            Ok(rendered) => print(&rendered, ExitCode::SUCCESS),
            Err(message) => {
                eprintln!("error: {message}");
                ExitCode::from(2)
            }
        },
        Command::Diff(args) => match diff_cmd::run(&args) {
            Ok((rendered, status)) => print(&rendered, ExitCode::from(status)),
            Err(message) => {
                eprintln!("error: {message}");
                ExitCode::from(2)
            }
        },
        Command::Tdms(args) => match tdms_cmd::run(&args) {
            Ok(rendered) => print(&rendered, ExitCode::SUCCESS),
            Err(message) => {
                eprintln!("error: {message}");
                ExitCode::from(2)
            }
        },
        Command::Lsp => lsp(),
        Command::Mcp => match buzzard_mcp::run_stdio() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("error: mcp transport failed: {err}");
                ExitCode::from(2)
            }
        },
    }
}

/// `buzzard inspect`: a project file, the directory of an archive, or a
/// binary file. A member path is tried first: `x.llb/p.lvlib` is a member
/// of an archive, and what a member is called decides how it is read.
fn inspect(file: &Path, format: InspectFormat) -> ExitCode {
    let is_member = archive::split(file).is_some();
    if is_project_file(file) && (is_member || file.is_file()) {
        inspect_project(file, format)
    } else if !is_member && archive::is_archive(file) && file.is_file() {
        inspect_archive(file, format)
    } else if is_project_file(file) {
        inspect_project(file, format)
    } else {
        inspect_binary(file, format)
    }
}

/// `buzzard check`: lint everything under `paths` and print the findings.
fn check(
    paths: &[PathBuf],
    format: CheckFormat,
    ignore: Vec<String>,
    max_path_length: usize,
    exit_zero: bool,
) -> ExitCode {
    let unknown: Vec<&String> = ignore
        .iter()
        .filter(|code| buzzard_lint::rules::rule(code).is_none())
        .collect();
    if !unknown.is_empty() {
        eprintln!("error: unknown rule code(s): {unknown:?}");
        return ExitCode::from(2);
    }
    if let Some(missing) = paths.iter().find(|p| !archive::exists(p)) {
        eprintln!("error: path does not exist: {}", missing.display());
        return ExitCode::from(2);
    }
    let config = LintConfig {
        max_path_length,
        ignore: ignore.into_iter().collect(),
    };
    let workspace = Workspace::scan(paths);
    let graph = DependencyGraph::build(&workspace);
    let diagnostics = lint(&workspace, &graph, &config);
    let rendered = match format {
        CheckFormat::Human => render_human(&diagnostics, workspace.files().len()),
        CheckFormat::Json => to_json(&diagnostics),
        CheckFormat::Sarif => to_json(&sarif::to_sarif(
            &workspace,
            &diagnostics,
            env!("CARGO_PKG_VERSION"),
        )),
    };
    let failed = diagnostics.iter().any(|d| d.severity != Severity::Info);
    let status = if failed && !exit_zero {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    };
    print(&rendered, status)
}

/// `buzzard verify`: check the state machines under `paths`.
fn verify(paths: &[PathBuf], format: VerifyFormat) -> ExitCode {
    if let Some(missing) = paths.iter().find(|p| !archive::exists(p)) {
        eprintln!("error: path does not exist: {}", missing.display());
        return ExitCode::from(2);
    }
    let verified = Verified::scan(paths);
    let rendered = match format {
        VerifyFormat::Text => report::to_text(&verified),
        VerifyFormat::Json => to_json(&verified),
        VerifyFormat::Dot => report::to_dot(&verified),
    };
    let status = if verified.has_findings() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    };
    print(&rendered, status)
}

/// `buzzard inspect` on a `.lvproj`, `.lvlib` or `.lvclass` file.
fn inspect_project(file: &Path, format: InspectFormat) -> ExitCode {
    match ProjectFile::from_path(file) {
        Ok(project) => {
            let rendered = match format {
                InspectFormat::Json => to_json(&project),
                InspectFormat::Markdown => {
                    summary::project_to_markdown(&project.name, &project, None)
                }
                InspectFormat::Svg | InspectFormat::Dot | InspectFormat::Mermaid => {
                    eprintln!(
                        "error: {}: a project file has no block diagram",
                        file.display()
                    );
                    return ExitCode::from(2);
                }
            };
            print(&rendered, ExitCode::SUCCESS)
        }
        Err(err) => {
            eprintln!("error: {}: {err}", file.display());
            ExitCode::from(2)
        }
    }
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
fn inspect_archive(file: &Path, format: InspectFormat) -> ExitCode {
    let fail = |err: String| {
        eprintln!("error: {}: {err}", file.display());
        ExitCode::from(2)
    };
    let opened = match archive::list(file) {
        Ok(Some(listing)) => listing,
        Ok(None) => {
            eprintln!(
                "note: {}: the archive was built into an application and has no directory of \
                 members; it is shown as the resource file it is",
                file.display()
            );
            return inspect_binary(file, format);
        }
        Err(err) => return fail(err),
    };
    let rendered = match format {
        InspectFormat::Json => match Inspection::from_path(file) {
            Ok(inspection) => to_json(&ArchiveOutput {
                metadata: &inspection.metadata,
                archive: &opened,
            }),
            Err(err) => return fail(err),
        },
        InspectFormat::Markdown => {
            let title = file
                .file_name()
                .unwrap_or(file.as_os_str())
                .to_string_lossy();
            summary::archive_to_markdown(&title, &opened)
        }
        InspectFormat::Svg | InspectFormat::Dot | InspectFormat::Mermaid => {
            let example = opened.opened().first().map_or_else(
                || "<member>".to_owned(),
                |name| archive::member_path(file, name).display().to_string(),
            );
            eprintln!(
                "error: {}: an archive has no block diagram; name one of its members, as in \
                 `buzzard inspect \"{example}\"`",
                file.display()
            );
            return ExitCode::from(2);
        }
    };
    print(&rendered, ExitCode::SUCCESS)
}

/// `buzzard inspect` on a binary file: its metadata and block diagram, or a
/// picture of the diagram. `file` may name a member of an archive.
fn inspect_binary(file: &Path, format: InspectFormat) -> ExitCode {
    let inspection = match Inspection::from_path(file) {
        Ok(inspection) => inspection,
        Err(err) => {
            eprintln!("error: {}: {err}", file.display());
            return ExitCode::from(2);
        }
    };
    let picture = |draw: fn(&BlockDiagram) -> String| match &inspection.diagram {
        Some(diagram) => Ok(draw(diagram)),
        None => Err("file has no block diagram"),
    };
    let rendered = match format {
        InspectFormat::Svg => picture(render::to_svg),
        InspectFormat::Dot => picture(render::to_dot),
        InspectFormat::Mermaid => picture(render::to_mermaid),
        InspectFormat::Json => Ok(to_json(&InspectOutput {
            metadata: &inspection.metadata,
            diagram: inspection.diagram.as_ref(),
        })),
        InspectFormat::Markdown => {
            let title = file
                .file_name()
                .unwrap_or(file.as_os_str())
                .to_string_lossy();
            let mut text = summary::to_markdown(&title, &inspection.metadata, None);
            if let Some(diagram) = &inspection.diagram {
                text.push('\n');
                text.push_str(&summary::diagram_to_markdown(diagram));
            }
            Ok(text)
        }
    };
    match rendered {
        Ok(rendered) => print(&rendered, ExitCode::SUCCESS),
        Err(reason) => {
            eprintln!("error: {}: {reason}", file.display());
            ExitCode::from(2)
        }
    }
}

/// `buzzard lsp`: serve the language server over stdio.
fn lsp() -> ExitCode {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build();
    match runtime {
        Ok(runtime) => {
            let status = runtime.block_on(buzzard_ls::run_stdio());
            #[expect(
                clippy::exit,
                reason = "the runtime's blocking stdin reader would otherwise keep the process alive until the client closes the pipe"
            )]
            std::process::exit(status);
        }
        Err(err) => {
            eprintln!("error: cannot start async runtime: {err}");
            ExitCode::from(2)
        }
    }
}

/// Write the report to stdout and return the status to exit with.
///
/// A closed pipe (e.g. `buzzard check | head`) is not an error: the reader
/// has seen what it wanted. Any other failure (a full disk, a quota, a
/// broken mount) means the report is missing or truncated, so the command
/// must not look like it succeeded: the error goes to stderr and the
/// status is 2, whatever `status` was.
fn print(text: &str, status: ExitCode) -> ExitCode {
    let mut stdout = io::stdout().lock();
    match write_report(&mut stdout, text) {
        Ok(()) => status,
        Err(err) => {
            eprintln!("error: cannot write output: {err}");
            ExitCode::from(2)
        }
    }
}

fn write_report(out: &mut impl Write, text: &str) -> io::Result<()> {
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Err(err) if err.kind() != io::ErrorKind::BrokenPipe => Err(err),
        _ => Ok(()),
    }
}

fn to_json(value: &impl serde::Serialize) -> String {
    serde_json::to_string_pretty(value).expect("diagnostics always serialize") + "\n"
}

fn render_human(diagnostics: &[Diagnostic], file_count: usize) -> String {
    let mut out = String::new();
    for d in diagnostics {
        let _ = writeln!(
            out,
            "{}: {}[{}]: {}",
            d.path.display(),
            d.severity.as_str(),
            d.code,
            d.message
        );
    }
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();
    let files = if file_count == 1 { "file" } else { "files" };
    if diagnostics.is_empty() {
        let _ = writeln!(out, "No problems found in {file_count} {files}.");
    } else {
        let _ = writeln!(
            out,
            "Found {} problem{} ({errors} error{}, {warnings} warning{}) in {file_count} {files}.",
            diagnostics.len(),
            if diagnostics.len() == 1 { "" } else { "s" },
            if errors == 1 { "" } else { "s" },
            if warnings == 1 { "" } else { "s" },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Failing(io::ErrorKind);

    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(self.0.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Accepts everything and fails when flushed, like a buffered file on
    /// a full disk.
    struct FailingFlush;

    impl Write for FailingFlush {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::Other.into())
        }
    }

    #[test]
    fn only_a_closed_pipe_is_a_benign_write_error() {
        assert!(write_report(&mut Vec::new(), "report").is_ok());
        assert!(write_report(&mut Failing(io::ErrorKind::BrokenPipe), "report").is_ok());
        let err = write_report(&mut Failing(io::ErrorKind::Other), "report").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Other);
        assert!(write_report(&mut Failing(io::ErrorKind::PermissionDenied), "report").is_err());
        assert!(write_report(&mut FailingFlush, "report").is_err());
    }
}
