//! The `deps` subcommand: the external dependencies of a set of files.

use std::path::{Path, PathBuf};

use buzzard_lint::deps::{self, DepsReport, Roots};
use buzzard_lint::{DependencyGraph, Workspace};
use clap::ValueEnum;

#[derive(clap::Args)]
pub(crate) struct Args {
    /// Files, directories, or member paths such as lib/util.llb/Init.vi.
    #[arg(default_value = ".")]
    paths: Vec<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// The directory of a symbolic root, as in vilib=/opt/labview/vi.lib.
    /// Every file linked under that root is then looked up there and
    /// reported as found or missing. May be repeated, once per root.
    #[arg(long = "root", value_name = "NAME=DIR")]
    roots: Vec<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
    Markdown,
}

fn render(report: &DepsReport, format: Format) -> String {
    match format {
        Format::Text => deps::to_text(report),
        Format::Markdown => deps::to_markdown(report),
        Format::Json => {
            serde_json::to_string_pretty(report).expect("a dependency report always serializes")
                + "\n"
        }
    }
}

/// Returns the report. An `Err` is a message for stderr and status 2.
pub(crate) fn run(args: &Args) -> Result<String, String> {
    let mut roots = Roots::default();
    for spec in &args.roots {
        let (name, dir) = Roots::split(spec).map_err(|err| format!("--root: {err}"))?;
        roots
            .add(name, Path::new(dir))
            .map_err(|err| format!("--root: {err}"))?;
    }
    if let Some(missing) = args
        .paths
        .iter()
        .find(|p| !buzzard_lint::archive::exists(p))
    {
        return Err(format!("path does not exist: {}", missing.display()));
    }
    let workspace = Workspace::scan(&args.paths);
    let graph = DependencyGraph::build(&workspace);
    let report = deps::scan(&workspace, &graph, &roots);
    // The report lists the unused roots too. The warning on stderr reaches
    // the user even when a program reads stdout.
    let used: Vec<String> = report.roots.iter().map(|r| r.root.clone()).collect();
    for root in &report.unused_roots {
        eprintln!(
            "warning: --root {root}: no link is under <{root}>, so its directory was not used \
             (roots with links: {})",
            if used.is_empty() {
                "none".to_owned()
            } else {
                used.join(", ")
            }
        );
    }
    Ok(render(&report, args.format))
}
