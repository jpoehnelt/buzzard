//! The `buzzard` command: `check`, `inspect` and `diff` over the analysis in
//! `buzzard-lint`, `lsp` for the language server in `buzzard-ls`, `mcp`
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

use buzzard_lint::project::is_project_file;
use buzzard_lint::verify::report::{self, Verified};
use buzzard_lint::Inspection;
use buzzard_lint::{
    lint, render, sarif, summary, DependencyGraph, Diagnostic, LintConfig, ProjectFile, Severity,
    Workspace,
};
use buzzard_rsrc::{BlockDiagram, ViMetadata};
use clap::{Parser, Subcommand, ValueEnum};

mod diff_cmd;
mod tdms_cmd;

/// Static analyzer and language server that reads binary .vi and .ctl files
/// and .lvproj, .lvlib and .lvclass files directly, without the IDE.
#[derive(Parser)]
#[command(name = "buzzard", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Analyze files and directories and report problems.
    Check {
        /// Files or directories to analyze.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
        #[arg(long, value_enum, default_value_t = CheckFormat::Human)]
        format: CheckFormat,
        /// Rule code to skip, e.g. BZ004. May be repeated.
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
    Inspect {
        file: PathBuf,
        #[arg(long, value_enum, default_value_t = InspectFormat::Markdown)]
        format: InspectFormat,
    },
    /// Compare two versions of a file: settings, dependencies, nodes, wires.
    Diff(diff_cmd::Args),
    /// Find the state machines on block diagrams and check them: states
    /// that can never be reached, loops that their condition terminal can
    /// never stop, and constants that no case handles.
    Verify {
        /// Files or directories to analyze.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
        #[arg(long, value_enum, default_value_t = VerifyFormat::Text)]
        format: VerifyFormat,
    },
    /// Read TDMS measurement files: describe one, or export its channels
    /// as a Parquet or CSV table.
    Tdms(tdms_cmd::Args),
    /// Run the language server over stdin/stdout.
    Lsp,
    /// Run the Model Context Protocol server over stdin/stdout, so that AI
    /// agents can call the analysis as tools.
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
        Command::Inspect { file, format } if is_project_file(&file) => {
            inspect_project(&file, format)
        }
        Command::Inspect { file, format } => inspect_binary(&file, format),
        Command::Verify { paths, format } => verify(&paths, format),
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
    if let Some(missing) = paths.iter().find(|p| !p.exists()) {
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
        CheckFormat::Sarif => to_json(&sarif::to_sarif(&diagnostics, env!("CARGO_PKG_VERSION"))),
    };
    let failed = diagnostics.iter().any(|d| d.severity != Severity::Info);
    let status = if failed && !exit_zero {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    };
    print(&rendered, status)
}

/// `buzzard inspect` on a `.lvproj`, `.lvlib` or `.lvclass` file.
fn verify(paths: &[PathBuf], format: VerifyFormat) -> ExitCode {
    if let Some(missing) = paths.iter().find(|p| !p.exists()) {
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

/// `buzzard inspect` on a binary file: its metadata and block diagram, or a
/// picture of the diagram.
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

/// `buzzard lsp`: serve the language server over stdin/stdout.
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
