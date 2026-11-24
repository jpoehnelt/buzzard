//! The `buzzard` command: `check`, `inspect`, `deps`, `diff`, `verify`, and
//! `rules` over the analysis in `buzzard-lint`, `guide` for the guide for
//! agents, `lsp` for the language server in
//! `buzzard-ls`, `mcp` for the Model Context Protocol server in `buzzard-mcp`,
//! and `tdms` for the measurement file reader in `buzzard-tdms`.

#![expect(
    clippy::print_stderr,
    reason = "a command-line tool reports its errors on stderr"
)]

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use buzzard_lint::verify::report::{self, Verified};
use buzzard_lint::{
    check as checked, lint, patharg, printable, rules, sarif, DependencyGraph, Failure, LintConfig,
    Workspace,
};
use clap::{Parser, Subcommand, ValueEnum};

mod deps_cmd;
mod diff_cmd;
mod errors;
mod inspect_cmd;
mod tdms_cmd;

/// What a command prints on stdout, and the status it exits with.
pub(crate) struct Report {
    pub(crate) text: String,
    pub(crate) status: u8,
}

impl Report {
    /// A report of a command that exits with status 0.
    pub(crate) fn ok(text: String) -> Self {
        Self { text, status: 0 }
    }
}

/// A report, or the error that ends the command with status 2.
pub(crate) type Outcome = Result<Report, Failure>;

// `about` is the `description` of `Cargo.toml`.
#[derive(Parser)]
#[command(name = "buzzard", version, about, after_long_help = errors::help())]
struct Cli {
    /// How to report an error that ends the command. With json, the error
    /// is also one JSON object on one line of stdout, whatever format the
    /// report has. --format json, --format sarif, and --json imply it.
    #[arg(long, global = true, value_enum, default_value_t = ErrorFormat::Text, value_name = "FORMAT")]
    error_format: ErrorFormat,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ErrorFormat {
    /// `error: <message>` on stderr.
    Text,
    /// The line on stderr, and `{"error":{"kind","message","path"}}` on
    /// stdout.
    Json,
}

#[derive(Subcommand)]
enum Command {
    /// Analyze files and directories and report problems.
    #[command(
        after_help = "Exit status: 0 when no problem is an error or a warning, or when \
        --exit-zero is set. 1 when a problem is an error or a warning. 2 for an unknown \
        --ignore code, a path that does not exist or that holds a control character, or a \
        report that buzzard cannot write. --limit does not change the status."
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
        /// Longest path that rule BZ003 accepts, in characters.
        #[arg(long, default_value_t = 260)]
        max_path_length: usize,
        /// Exit with status 0 whatever problems the report holds.
        #[arg(long)]
        exit_zero: bool,
        /// Print the first N problems only. The summary still counts every
        /// problem and says how many are not shown. With a limit, json
        /// prints an object with `diagnostics` and `summary` in place of
        /// the list, and sarif adds the counts to the properties of the
        /// run when problems are left out.
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },
    /// Print what one file holds.
    ///
    /// For an .llb archive, the report is the directory of the archive.
    ///
    /// The json format with --detail full holds every node, terminal, and
    /// wire of the block diagram, and is often over 1 MB for one VI. Ask
    /// for --detail summary first: it is the same metadata with an outline
    /// of the diagram, and it states the size of the full JSON in
    /// `full_json_bytes`.
    #[command(
        after_help = "Exit status: 0, or 2 when buzzard cannot read the file, when you \
        ask for svg, dot, or mermaid and the file has no block diagram or with --detail \
        summary, when the file has no such frame, or when the path is a directory or holds \
        a control character."
    )]
    Inspect {
        /// A file, or a member path such as lib/util.llb/Init.vi.
        file: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = inspect_cmd::Format::Markdown)]
        format: inspect_cmd::Format,
        /// How much of a block diagram the json and markdown formats hold.
        /// An archive and a project file have one level of detail.
        #[arg(long, value_enum, default_value_t = inspect_cmd::Detail::Full)]
        detail: inspect_cmd::Detail,
        /// Print one frame of the block diagram as JSON: the id of a
        /// structure in the outline of --detail summary. Needs --frame and
        /// --format json. With --detail summary, the outline of that frame.
        #[arg(long, value_name = "ID", requires = "frame", allow_negative_numbers = true,
              conflicts_with_all = ["first_node", "last_node"])]
        structure: Option<i32>,
        /// The index of a frame of --structure, from 0.
        #[arg(long, value_name = "INDEX", requires = "structure")]
        frame: Option<usize>,
        /// Print the nodes with an id from --first-node to --last-node as
        /// JSON. A structure comes with everything inside it. Needs
        /// --format json.
        #[arg(
            long,
            value_name = "ID",
            requires = "last_node",
            allow_negative_numbers = true
        )]
        first_node: Option<i32>,
        /// The largest node id of the range.
        #[arg(
            long,
            value_name = "ID",
            requires = "first_node",
            allow_negative_numbers = true
        )]
        last_node: Option<i32>,
    },
    /// List what the files need from outside the project.
    ///
    /// The report lists the external files by package, the
    /// machine-absolute links, the relative links whose file does not
    /// exist, and the files that buzzard cannot parse. An external file is
    /// a file under a symbolic root such as vilib, instrlib, or userlib.
    #[command(
        after_help = "Exit status: 0 whatever the report holds, 2 for a path that does \
        not exist or that holds a control character, or a --root that buzzard cannot use."
    )]
    Deps(deps_cmd::Args),
    /// Compare the settings, dependencies, nodes, and wires of two versions
    /// of a file.
    #[command(
        after_help = "Exit status: 0 when the two files are the same, 1 when they \
        differ, 2 on an error, such as a path that holds a control character. With --git \
        the status is always 0."
    )]
    Diff(diff_cmd::Args),
    /// Find the state machines on block diagrams and check them.
    ///
    /// The findings are states that can never be reached, loops whose
    /// condition terminal can never stop them, and constants that no case
    /// handles. Each finding is labeled proven, structural, or heuristic.
    #[command(
        after_help = "Exit status: 0 when there are no findings. 1 when there are \
        findings, of any label. 2 on an error, such as a path that does not exist or that \
        holds a control character. The \
        report lists each file that buzzard cannot read or parse, and such a file does \
        not change the status."
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
    /// List the rules of `check`: code, severity, name, and summary.
    ///
    /// The codes are the values of `check --ignore` and the `ruleId` of
    /// SARIF.
    #[command(after_help = "Exit status: 0, or 2 when buzzard cannot write the list.")]
    Rules {
        /// Output format.
        #[arg(long, value_enum, default_value_t = RulesFormat::Text)]
        format: RulesFormat,
    },
    /// Print the guide to buzzard for an AI agent.
    ///
    /// The guide says which command, format, and size limit to ask for,
    /// what the exit statuses, labels, and error kinds mean, and how to
    /// treat names from files. `buzzard mcp` serves the same text as the
    /// prompt `buzzard`.
    #[command(after_help = "Exit status: 0, or 2 when buzzard cannot write the guide.")]
    Guide,
    /// Run the language server over stdio.
    #[command(
        after_help = "Exit status: 0 after shutdown and then exit, or when the client \
        closes the connection. 1 after exit without shutdown. 2 when the server cannot \
        start."
    )]
    Lsp,
    /// Run the Model Context Protocol server over stdio.
    ///
    /// AI agents call its tools to run the analysis. The tools read files
    /// and write none.
    #[command(
        after_help = "Exit status: 0 when the client closes the connection, 2 when \
        reading a request or writing a response fails, or when a --root is not a \
        directory."
    )]
    Mcp {
        /// A directory that the tools may read. With one or more roots,
        /// every path argument of every tool must lead into a root once
        /// links and `..` are resolved, and the server refuses any other
        /// path. Without a root, a tool reads any file that the process
        /// can read. May be repeated.
        #[arg(long = "root", value_name = "DIR")]
        roots: Vec<PathBuf>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum CheckFormat {
    Human,
    Json,
    Sarif,
}

#[derive(Clone, Copy, ValueEnum)]
enum VerifyFormat {
    /// The state machines and their findings.
    Text,
    /// The report as JSON.
    Json,
    /// The transition graphs as a Graphviz graph.
    Dot,
}

#[derive(Clone, Copy, ValueEnum)]
enum RulesFormat {
    /// One line per rule.
    Text,
    /// An object with the list `rules`.
    Json,
}

impl Command {
    /// Whether the caller asked for a format that a program reads. An
    /// error is then printed as JSON on stdout too.
    fn machine(&self) -> bool {
        match self {
            Self::Check { format, .. } => matches!(format, CheckFormat::Json | CheckFormat::Sarif),
            Self::Inspect { format, .. } => matches!(format, inspect_cmd::Format::Json),
            Self::Verify { format, .. } => matches!(format, VerifyFormat::Json),
            Self::Rules { format } => matches!(format, RulesFormat::Json),
            Self::Deps(args) => args.machine(),
            Self::Diff(args) => args.machine(),
            Self::Tdms(args) => args.machine(),
            Self::Guide | Self::Lsp | Self::Mcp { .. } => false,
        }
    }
}

fn main() -> ExitCode {
    let (command, error_format) = match Cli::try_parse() {
        Ok(cli) => (cli.command, cli.error_format),
        Err(err) => return errors::usage(&err),
    };
    let machine = command.machine() || error_format == ErrorFormat::Json;
    let outcome = match command {
        Command::Check {
            paths,
            format,
            ignore,
            max_path_length,
            exit_zero,
            limit,
        } => check(&paths, format, ignore, max_path_length, exit_zero, limit),
        Command::Inspect {
            file,
            format,
            detail,
            structure,
            frame,
            first_node,
            last_node,
        } => {
            // clap makes each pair come together and never both pairs.
            let part = match (structure.zip(frame), first_node.zip(last_node)) {
                (Some((structure, frame)), _) => {
                    Some(inspect_cmd::Part::Frame { structure, frame })
                }
                (None, Some((first, last))) => Some(inspect_cmd::Part::Nodes { first, last }),
                (None, None) => None,
            };
            inspect_cmd::run(&file, format, detail, part)
        }
        Command::Verify { paths, format } => verify(&paths, format),
        Command::Rules { format } => Ok(Report::ok(match format {
            RulesFormat::Text => rules::listing_to_text(),
            RulesFormat::Json => rules::listing_to_json(),
        })),
        Command::Guide => Ok(Report::ok(buzzard_mcp::guide().to_owned())),
        Command::Deps(args) => deps_cmd::run(&args),
        Command::Diff(args) => diff_cmd::run(&args),
        Command::Tdms(args) => tdms_cmd::run(&args),
        Command::Lsp => return lsp(),
        Command::Mcp { roots } => {
            let served = roots
                .iter()
                .try_for_each(|root| {
                    patharg::check(root)?;
                    // The error names the root as the caller wrote it.
                    if root.is_dir() {
                        Ok(())
                    } else {
                        Err(Failure::bad_argument(format!(
                            "the root is not an existing directory: {}",
                            printable::path(root)
                        ))
                        .with_path(root))
                    }
                })
                .and_then(|()| buzzard_mcp::run_stdio(&roots));
            return match served {
                Ok(()) => ExitCode::SUCCESS,
                Err(failure) => errors::fail(&failure, machine),
            };
        }
    };
    match outcome {
        Ok(report) => print(&report.text, ExitCode::from(report.status)),
        Err(failure) => errors::fail(&failure, machine),
    }
}

/// `buzzard check`: lint everything under `paths` and print the findings.
fn check(
    paths: &[PathBuf],
    format: CheckFormat,
    ignore: Vec<String>,
    max_path_length: usize,
    exit_zero: bool,
    limit: Option<usize>,
) -> Outcome {
    let unknown = rules::unknown_codes(&ignore);
    if !unknown.is_empty() {
        return Err(Failure::unknown_rules(&unknown));
    }
    patharg::existing(paths)?;
    let config = LintConfig {
        max_path_length,
        ignore: ignore.into_iter().collect(),
    };
    let workspace = Workspace::scan(paths);
    let graph = DependencyGraph::build(&workspace);
    let diagnostics = lint(&workspace, &graph, &config);
    let files = workspace.files().len();
    let version = env!("CARGO_PKG_VERSION");
    let shown = checked::limit(&diagnostics, files, limit);
    // Without a limit, each format is the one that the command always
    // printed: json is the list of findings and nothing else.
    let text = match (limit, format) {
        (None, CheckFormat::Human) => checked::to_human(&diagnostics, files),
        (None, CheckFormat::Json) => to_json(&diagnostics),
        (None, CheckFormat::Sarif) => to_json(&sarif::to_sarif(&workspace, &diagnostics, version)),
        (Some(_), CheckFormat::Human) => shown.to_human(),
        (Some(_), CheckFormat::Json) => shown.to_json(),
        (Some(_), CheckFormat::Sarif) => to_json(&shown.to_sarif(&workspace, version)),
    };
    // The status counts every problem, shown or not.
    Ok(Report {
        text,
        status: u8::from(shown.failed() && !exit_zero),
    })
}

/// `buzzard verify`: check the state machines under `paths`.
fn verify(paths: &[PathBuf], format: VerifyFormat) -> Outcome {
    patharg::existing(paths)?;
    let verified = Verified::scan(paths);
    let text = match format {
        VerifyFormat::Text => report::to_text(&verified),
        VerifyFormat::Json => to_json(&verified),
        VerifyFormat::Dot => report::to_dot(&verified),
    };
    Ok(Report {
        text,
        status: u8::from(verified.has_findings()),
    })
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
/// A closed pipe (for example `buzzard check | head`) is not an error,
/// because the reader chose to stop reading. Any other failure (a full
/// disk, a quota, a broken mount) means the report is missing or
/// truncated. The error then goes to stderr and the status is 2, whatever
/// `status` was. It is not printed as JSON, because stdout is what failed.
fn print(text: &str, status: ExitCode) -> ExitCode {
    let mut stdout = io::stdout().lock();
    match write_report(&mut stdout, text) {
        Ok(()) => status,
        Err(err) => errors::fail(&Failure::write_failed(&err), false),
    }
}

pub(crate) fn write_report(out: &mut impl Write, text: &str) -> io::Result<()> {
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Err(err) if err.kind() != io::ErrorKind::BrokenPipe => Err(err),
        _ => Ok(()),
    }
}

pub(crate) fn to_json(value: &impl serde::Serialize) -> String {
    serde_json::to_string_pretty(value).expect("a report always serializes") + "\n"
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

    /// The arguments are checked when the tests run, as clap checks them
    /// in a debug build on the first parse.
    #[test]
    fn the_command_line_is_well_formed() {
        use clap::CommandFactory as _;
        Cli::command().debug_assert();
    }
}
