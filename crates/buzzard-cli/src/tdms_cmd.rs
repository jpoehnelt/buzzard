//! The `tdms` subcommands: `info` describes a TDMS measurement file and
//! `export` writes its channels as a Parquet or CSV table.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use buzzard_lint::{patharg, printable, ErrorKind, Failure};
use buzzard_tdms::export::{self, ExportOptions, Exported};
use buzzard_tdms::{Channel, TdmsFile};
use clap::{Subcommand, ValueEnum};

use crate::{to_json, Outcome, Report};

#[derive(clap::Args)]
pub(crate) struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print what a TDMS file holds.
    ///
    /// The report lists the groups, channels, types, lengths, and
    /// properties of the file, its segment count, and its anomalies, such
    /// as a truncated last segment.
    #[command(
        after_help = "Exit status: 0, or 2 when buzzard cannot open the file or its \
        path holds a control character. Anomalies are part of the report and do not change \
        the status."
    )]
    Info {
        /// A TDMS file. The reader uses the index file beside it when the
        /// two match.
        file: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = InfoFormat::Text)]
        format: InfoFormat,
    },
    /// Write channels of a TDMS file as one table.
    ///
    /// The export does not replace a file: when --out names something that
    /// exists, pass --force. --dry-run reads the channels and reports the
    /// rows, the columns, and the target, and writes nothing.
    #[command(
        after_help = "Exit status: 0, or 2 on any error, such as an --out that exists \
        without --force. An error leaves no new output file behind, and leaves an \
        existing file as it was, with or without --force."
    )]
    Export {
        /// A TDMS file. The reader uses the index file beside it when the
        /// two match.
        file: PathBuf,
        /// The file to write.
        #[arg(long, value_name = "PATH")]
        out: PathBuf,
        /// The channels to export, in order. group/channel names one
        /// channel, and group names every channel of that group. The
        /// default is every channel.
        #[arg(long, num_args = 1.., value_name = "NAME")]
        channels: Vec<String>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = ExportFormat::Parquet)]
        format: ExportFormat,
        /// Compression of Parquet column data. CSV ignores it.
        #[arg(long, value_enum, default_value_t = Compression::Snappy)]
        compression: Compression,
        /// Export the stored values, without the linear scaling that a
        /// channel's properties describe.
        #[arg(long)]
        raw: bool,
        /// Number of worker threads for reading and encoding. The default
        /// is one per core.
        #[arg(long, value_name = "N")]
        threads: Option<usize>,
        /// Replace the file that --out names when it exists.
        #[arg(long)]
        force: bool,
        /// Read the channels and report what an export would write, the
        /// target, and whether the target exists. Nothing is written.
        #[arg(long)]
        dry_run: bool,
        /// Print the result as JSON: `input`, `out`, `format`, `dry_run`,
        /// `written`, `out_existed`, `rows`, `columns`, and `skipped`. An
        /// error is then one
        /// JSON object on stdout too.
        #[arg(long)]
        json: bool,
    },
}

impl Args {
    /// Whether the caller asked for a format that a program reads.
    pub(crate) fn machine(&self) -> bool {
        match &self.command {
            Command::Info { format, .. } => matches!(format, InfoFormat::Json),
            Command::Export { json, .. } => *json,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum InfoFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ExportFormat {
    Parquet,
    Csv,
}

impl ExportFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Parquet => "parquet",
            Self::Csv => "csv",
        }
    }
}

/// Compression of Parquet column data. CSV ignores it.
#[derive(Clone, Copy, ValueEnum)]
enum Compression {
    Snappy,
    None,
}

/// What `export` was asked to do, apart from the two paths.
#[derive(Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is one independent flag of the command line"
)]
struct ExportChoices<'a> {
    names: &'a [String],
    format: ExportFormat,
    compression: Compression,
    raw: bool,
    force: bool,
    dry_run: bool,
    json: bool,
}

/// Runs the subcommand and returns what to print.
pub(crate) fn run(args: &Args) -> Outcome {
    match &args.command {
        Command::Info { file, format } => info(file, *format),
        Command::Export {
            file,
            out,
            channels,
            format,
            compression,
            raw,
            threads,
            force,
            dry_run,
            json,
        } => {
            let choices = ExportChoices {
                names: channels,
                format: *format,
                compression: *compression,
                raw: *raw,
                force: *force,
                dry_run: *dry_run,
                json: *json,
            };
            let work = || export(file, out, choices);
            match threads {
                Some(n) => rayon::ThreadPoolBuilder::new()
                    .num_threads(*n)
                    .build()
                    .map_err(|e| {
                        Failure::new(
                            ErrorKind::Internal,
                            format!("cannot start {n} threads: {e}"),
                        )
                    })?
                    .install(work),
                None => work(),
            }
        }
    }
}

fn open(path: &Path) -> Result<TdmsFile, Failure> {
    patharg::check(path)?;
    TdmsFile::open(path).map_err(|e| {
        let mut failure = Failure::unreadable(path, e);
        if std::fs::symlink_metadata(path).is_err() {
            failure.kind = ErrorKind::PathNotFound;
        }
        failure
    })
}

/// `buzzard tdms info`: what the file holds, without reading any raw data.
/// The report is the one of `buzzard_tdms::info`, which the MCP server
/// prints too.
fn info(path: &Path, format: InfoFormat) -> Outcome {
    let file = open(path)?;
    let text = match format {
        InfoFormat::Text => buzzard_tdms::info::to_text(&printable::path(path), &file),
        // JSON escapes the path itself.
        InfoFormat::Json => buzzard_tdms::info::to_json(&path.display().to_string(), &file)
            .map_err(|e| {
                Failure::new(
                    ErrorKind::Internal,
                    format!("cannot serialize the report: {e}"),
                )
            })?,
    };
    Ok(Report::ok(text))
}

/// The channels a `--channels` value names: one channel as `group/channel`
/// or as its full path, or every channel of a group.
fn select<'a>(file: &'a TdmsFile, name: &str) -> Result<Vec<Channel<'a>>, Failure> {
    if let Some(channel) = file.channels().find(|c| c.path() == name) {
        return Ok(vec![channel]);
    }
    // Names may contain slashes themselves, so try every split.
    for (at, _) in name.match_indices('/') {
        if let Some(channel) = file.channel(&name[..at], &name[at + 1..]) {
            return Ok(vec![channel]);
        }
    }
    match file.group(name) {
        Some(group) => Ok(group.channels().collect()),
        None => Err(Failure::bad_argument(format!(
            "no channel or group named {}",
            printable::text(name)
        ))),
    }
}

/// The result of `tdms export --json`, of a real run or a dry run.
#[derive(serde::Serialize)]
struct ExportJson<'a> {
    /// The TDMS file.
    input: String,
    /// The file that `--out` names.
    out: String,
    /// `parquet` or `csv`.
    format: &'static str,
    dry_run: bool,
    /// Whether this run wrote `out`.
    written: bool,
    /// Whether `out` existed before this run.
    out_existed: bool,
    rows: u64,
    /// The names of the columns, in order.
    columns: &'a [String],
    /// The channels left out because they never held a value.
    skipped: &'a [String],
}

/// Whether `out` and `input` are one file. Writing the table there would
/// destroy the measurement before it is read.
fn same_file(out: &Path, input: &Path) -> bool {
    matches!(
        (out.canonicalize(), input.canonicalize()),
        (Ok(out), Ok(input)) if out == input
    )
}

/// `buzzard tdms export`: write the chosen channels as one table.
fn export(path: &Path, out: &Path, choices: ExportChoices<'_>) -> Outcome {
    patharg::check(out)?;
    let file = open(path)?;
    let mut channels: Vec<Channel<'_>> = Vec::new();
    if choices.names.is_empty() {
        channels.extend(file.channels());
    }
    for name in choices.names {
        for channel in select(&file, name)? {
            if !channels.iter().any(|c| c.path() == channel.path()) {
                channels.push(channel);
            }
        }
    }
    let options = ExportOptions {
        scaled: !choices.raw,
        ..ExportOptions::default()
    };
    // A link that leads nowhere exists too: a file created through it
    // would appear somewhere else.
    let out_existed = std::fs::symlink_metadata(out).is_ok();
    if same_file(out, path) {
        return Err(Failure::bad_argument(format!(
            "--out names the input file: {}",
            printable::path(out)
        ))
        .with_path(out));
    }
    let failed = |err: buzzard_tdms::Error| {
        // An I/O error of a real export is more often the disk of the
        // output than the input, which was opened and indexed already. A
        // dry run writes nowhere, so there it is the input.
        let (kind, about) = match err {
            buzzard_tdms::Error::Io(_) if !choices.dry_run => (ErrorKind::WriteFailed, out),
            _ => (ErrorKind::UnreadableFile, path),
        };
        Failure::new(kind, format!("{}: {err}", printable::path(path))).with_path(about)
    };
    if cfg!(not(feature = "parquet")) && choices.format == ExportFormat::Parquet {
        return Err(Failure::unsupported(
            path,
            "this build has no Parquet support; use --format csv",
        ));
    }
    let exported = if choices.dry_run {
        // The same work as a real export, into nothing: the rows and
        // columns are those a real run would write, and so are the errors.
        write(&file, &channels, io::sink(), choices, &options).map_err(failed)?
    } else {
        if out_existed && !choices.force {
            return Err(Failure::output_exists(out));
        }
        // A new file is written in place. A replacement is written beside
        // the file and moved onto it when it is whole, so that an export
        // that fails leaves the old file as it was.
        let written_to = if out_existed {
            beside(out)
        } else {
            out.to_path_buf()
        };
        let target = create(&written_to, out)?;
        let moved = write(&file, &channels, BufWriter::new(target), choices, &options)
            .map_err(failed)
            .and_then(|exported| {
                if out_existed {
                    std::fs::rename(&written_to, out)
                        .map_err(|err| Failure::cannot_create(out, &err))?;
                }
                Ok(exported)
            });
        match moved {
            Ok(exported) => exported,
            Err(failure) => {
                // Do not leave half a table behind.
                let _ = std::fs::remove_file(&written_to);
                return Err(failure);
            }
        }
    };
    Ok(Report::ok(if choices.json {
        to_json(&ExportJson {
            input: path.to_string_lossy().into_owned(),
            out: out.to_string_lossy().into_owned(),
            format: choices.format.as_str(),
            dry_run: choices.dry_run,
            written: !choices.dry_run,
            out_existed,
            rows: exported.rows,
            columns: &exported.columns,
            skipped: &exported.skipped,
        })
    } else {
        export_text(out, out_existed, choices, &exported)
    }))
}

/// The file that a replacement of `out` is written to first: `out` with
/// one more extension, in the same directory, so that the move onto `out`
/// stays on one file system.
fn beside(out: &Path) -> PathBuf {
    let mut name = out.as_os_str().to_owned();
    name.push(format!(".{}.tmp", std::process::id()));
    PathBuf::from(name)
}

/// Create the file that the table is written to, for the output `out`. The
/// file is created only if nothing has its name, in one step: a file that
/// appears after `export` looked is not replaced either.
fn create(written_to: &Path, out: &Path) -> Result<File, Failure> {
    File::options()
        .write(true)
        .create_new(true)
        .open(written_to)
        .map_err(|err| {
            if written_to == out && err.kind() == io::ErrorKind::AlreadyExists {
                Failure::output_exists(out)
            } else {
                Failure::cannot_create(written_to, &err)
            }
        })
}

/// The text report of an export or of a dry run. The names of columns and
/// channels come from the TDMS file, so each is printed on one line.
fn export_text(
    out: &Path,
    out_existed: bool,
    choices: ExportChoices<'_>,
    exported: &Exported,
) -> String {
    let shown = printable::path(out);
    let mut text = format!(
        "{} {} rows of {} columns to {shown}\n",
        if choices.dry_run {
            "would export"
        } else {
            "exported"
        },
        exported.rows,
        exported.columns.len(),
    );
    if choices.dry_run {
        for column in &exported.columns {
            let _ = writeln!(text, "column {}", printable::text(column));
        }
    }
    for skipped in &exported.skipped {
        let _ = writeln!(
            text,
            "skipped {}: it never held a value",
            printable::text(skipped)
        );
    }
    if choices.dry_run {
        let _ = writeln!(
            text,
            "{}",
            match (out_existed, choices.force) {
                (false, _) => format!("{shown} does not exist; nothing was written"),
                (true, true) =>
                    format!("{shown} exists and --force replaces it; nothing was written"),
                (true, false) =>
                    format!("{shown} exists; an export needs --force; nothing was written"),
            }
        );
    }
    text
}

fn write<W: Write + Send>(
    file: &TdmsFile,
    channels: &[Channel<'_>],
    out: W,
    choices: ExportChoices<'_>,
    options: &ExportOptions,
) -> Result<Exported, buzzard_tdms::Error> {
    match choices.format {
        ExportFormat::Csv => export::write_csv(file, channels, out, options),
        #[cfg(feature = "parquet")]
        ExportFormat::Parquet => {
            let compression = match choices.compression {
                Compression::Snappy => export::parquet::Compression::Snappy,
                Compression::None => export::parquet::Compression::None,
            };
            export::parquet::write_parquet(file, channels, out, options, compression)
        }
        #[cfg(not(feature = "parquet"))]
        ExportFormat::Parquet => {
            // `export` refuses the format before it opens anything.
            let _ = (choices.compression, out);
            Err(buzzard_tdms::Error::Io(io::ErrorKind::Unsupported.into()))
        }
    }
}
