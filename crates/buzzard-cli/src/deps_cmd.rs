//! The `deps` subcommand: the external dependencies of a set of files.

use std::path::{Path, PathBuf};

use buzzard_lint::deps::{self, DepsReport, Roots};
use buzzard_lint::{patharg, printable, DependencyGraph, Failure, Workspace};
use clap::ValueEnum;

use crate::{to_json, Outcome, Report};

#[derive(clap::Args)]
pub(crate) struct Args {
    /// Files, directories, or member paths such as lib/util.llb/Init.vi.
    #[arg(default_value = ".")]
    paths: Vec<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// The directory of a symbolic root, as in vilib=/opt/labview/vi.lib.
    /// buzzard looks there for every file linked under that root and
    /// reports the file as found or missing. A file under a root without
    /// a directory is unchecked. May be repeated, once per root.
    #[arg(long = "root", value_name = "NAME=DIR")]
    roots: Vec<String>,
    /// How much of the report to print. A report of every file can be
    /// long, so ask for packages first.
    #[arg(long, value_enum, default_value_t = Detail::Files)]
    detail: Detail,
}

impl Args {
    /// Whether the caller asked for a format that a program reads.
    pub(crate) fn machine(&self) -> bool {
        matches!(self.format, Format::Json)
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
    Markdown,
}

#[derive(Clone, Copy, ValueEnum)]
enum Detail {
    /// The packages of each root with their counts, and the counts of the
    /// links: the `Overview` that the MCP tool returns by default.
    Packages,
    /// Every external file and every link, with the files that use it.
    Files,
}

fn render(report: &DepsReport, format: Format, detail: Detail) -> String {
    match (detail, format) {
        (Detail::Files, Format::Text) => deps::to_text(report),
        (Detail::Files, Format::Markdown) => deps::to_markdown(report),
        (Detail::Files, Format::Json) => to_json(report),
        (Detail::Packages, Format::Text) => deps::overview_to_text(report),
        (Detail::Packages, Format::Markdown) => deps::overview_to_markdown(report),
        (Detail::Packages, Format::Json) => to_json(&report.overview()),
    }
}

/// Returns the report. The status is 0 whatever the report holds.
pub(crate) fn run(args: &Args) -> Outcome {
    // The message repeats the argument, which may hold anything.
    let bad_root =
        |err: String| Failure::bad_argument(format!("--root: {}", printable::text(&err)));
    let mut roots = Roots::default();
    for spec in &args.roots {
        let (name, dir) = Roots::split(spec).map_err(bad_root)?;
        patharg::check(Path::new(dir))?;
        roots.add(name, Path::new(dir)).map_err(bad_root)?;
    }
    patharg::existing(&args.paths)?;
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
    Ok(Report::ok(render(&report, args.format, args.detail)))
}
