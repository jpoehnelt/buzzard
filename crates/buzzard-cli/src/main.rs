use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use buzzard_lint::{
    lint, sarif, summary, DependencyGraph, Diagnostic, LintConfig, Severity, Workspace,
};
use buzzard_rsrc::ViMetadata;
use clap::{Parser, Subcommand, ValueEnum};

/// Static analyzer and language server that reads binary .vi and .ctl files
/// directly, without the IDE.
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
    /// Run the language server over stdin/stdout.
    Lsp,
}

#[derive(Clone, Copy, ValueEnum)]
enum CheckFormat {
    Human,
    Json,
    Sarif,
}

#[derive(Clone, Copy, ValueEnum)]
enum InspectFormat {
    Markdown,
    Json,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Check {
            paths,
            format,
            ignore,
            max_path_length,
            exit_zero,
        } => {
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
            let workspace = Workspace::scan(&paths);
            let graph = DependencyGraph::build(&workspace);
            let diagnostics = lint(&workspace, &graph, &config);
            let rendered = match format {
                CheckFormat::Human => render_human(&diagnostics, workspace.files().len()),
                CheckFormat::Json => to_json(&diagnostics),
                CheckFormat::Sarif => {
                    to_json(&sarif::to_sarif(&diagnostics, env!("CARGO_PKG_VERSION")))
                }
            };
            print(&rendered);
            let failed = diagnostics.iter().any(|d| d.severity != Severity::Info);
            if failed && !exit_zero {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Command::Inspect { file, format } => match ViMetadata::from_path(&file) {
            Ok(meta) => {
                let rendered = match format {
                    InspectFormat::Json => to_json(&meta),
                    InspectFormat::Markdown => {
                        let title = file
                            .file_name()
                            .unwrap_or(file.as_os_str())
                            .to_string_lossy();
                        summary::to_markdown(&title, &meta, None)
                    }
                };
                print(&rendered);
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("error: {}: {err}", file.display());
                ExitCode::from(2)
            }
        },
        Command::Lsp => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => {
                    runtime.block_on(buzzard_ls::run_stdio());
                    // The runtime's blocking stdin reader would otherwise keep
                    // the process alive until the client closes the pipe.
                    std::process::exit(0)
                }
                Err(err) => {
                    eprintln!("error: cannot start async runtime: {err}");
                    ExitCode::from(2)
                }
            }
        }
    }
}

/// Write to stdout, ignoring a closed pipe (e.g. `buzzard check | head`).
fn print(text: &str) {
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

fn to_json(value: &impl serde::Serialize) -> String {
    serde_json::to_string_pretty(value).expect("diagnostics always serialize") + "\n"
}

fn render_human(diagnostics: &[Diagnostic], file_count: usize) -> String {
    let mut out = String::new();
    for d in diagnostics {
        out.push_str(&format!(
            "{}: {}[{}]: {}\n",
            d.path.display(),
            d.severity.as_str(),
            d.code,
            d.message
        ));
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
        out.push_str(&format!("No problems found in {file_count} {files}.\n"));
    } else {
        out.push_str(&format!(
            "Found {} problem{} ({errors} error{}, {warnings} warning{}) in {file_count} {files}.\n",
            diagnostics.len(),
            if diagnostics.len() == 1 { "" } else { "s" },
            if errors == 1 { "" } else { "s" },
            if warnings == 1 { "" } else { "s" },
        ));
    }
    out
}
