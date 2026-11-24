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
    /// The export does not replace a file: when --out names a file that
    /// exists, pass --force. --dry-run reads the channels and reports the
    /// rows, the columns, and the target, and writes nothing.
    #[command(
        after_help = "Exit status: 0, or 2 on any error, such as an --out that exists \
        without --force. An error leaves no new output file behind, and leaves an \
        existing file as it was, with or without --force. The one exception is a \
        report that cannot be written after the export: the status is 2 and the table \
        stays. --force replaces a regular file, behind a link too, and keeps its \
        permissions. An --out that is not a regular file, such as /dev/stdout or a \
        pipe, is written into and needs no --force."
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
        /// target, and whether the target exists. Nothing is written. A
        /// dry run fails where it can see that the export would: a bad
        /// channel name, an --out that is a directory or whose directory
        /// does not exist.
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
    if path.is_dir() {
        return Err(Failure::not_a_file(path));
    }
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
    let target = Target::of(out);
    let out_existed = !matches!(target, Target::New);
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
    // A directory is never a place for a table, with or without --force,
    // in a real run and in a dry run.
    if let Target::Directory = target {
        return Err(Failure::cannot_create(
            out,
            &io::Error::from(io::ErrorKind::IsADirectory),
        ));
    }
    let exported = if choices.dry_run {
        // A new file needs a directory to be created in.
        if let (Target::New, Some(parent)) = (&target, out.parent()) {
            if !parent.as_os_str().is_empty() && !parent.is_dir() {
                return Err(Failure::cannot_create(
                    out,
                    &io::Error::from(io::ErrorKind::NotFound),
                ));
            }
        }
        // The same work as a real export, into nothing: the rows and
        // columns are those a real run would write, and so are the errors.
        write(&file, &channels, io::sink(), choices, &options).map_err(failed)?
    } else {
        if target.needs_force() && !choices.force {
            return Err(Failure::output_exists(out));
        }
        let table = |to: File| {
            write(&file, &channels, BufWriter::new(to), choices, &options).map_err(failed)
        };
        match &target {
            Target::New => {
                let created = create(out, out)?;
                table(created).inspect_err(|_| {
                    // Do not leave half a table behind.
                    let _ = std::fs::remove_file(out);
                })?
            }
            Target::Regular { file, permissions } => replace(file, permissions.clone(), table)?,
            // Nothing here can be put back or taken away: the table goes
            // into what `--out` names.
            Target::InPlace { .. } | Target::Directory => {
                let opened = File::options()
                    .write(true)
                    .truncate(true)
                    .create(target.needs_force())
                    .open(out)
                    .map_err(|err| Failure::cannot_create(out, &err))?;
                table(opened)?
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
        export_text(out, &target, choices, &exported)
    }))
}

/// What `--out` names before the export.
enum Target {
    /// Nothing has the name.
    New,
    /// A regular file, which only `--force` replaces. `file` is the file
    /// itself, behind every link that `--out` goes through.
    Regular {
        file: PathBuf,
        permissions: std::fs::Permissions,
    },
    /// Something else that a table can be written into: a device such as
    /// the null device, a pipe, standard output by its name, or a link
    /// whose target does not exist. It is never replaced, because the
    /// directory entry is what the caller means to keep.
    InPlace {
        /// A link that leads nowhere: a file created through it would
        /// appear somewhere else, so it needs `--force` as a file does.
        dangling: bool,
    },
    /// A directory.
    Directory,
}

impl Target {
    fn of(out: &Path) -> Self {
        if std::fs::symlink_metadata(out).is_err() {
            return Self::New;
        }
        match std::fs::metadata(out) {
            Err(_) => Self::InPlace { dangling: true },
            Ok(meta) if meta.is_dir() => Self::Directory,
            Ok(meta) if meta.is_file() => Self::Regular {
                file: out.canonicalize().unwrap_or_else(|_| out.to_path_buf()),
                permissions: meta.permissions(),
            },
            Ok(_) => Self::InPlace { dangling: false },
        }
    }

    /// Whether an export would destroy or create a file that the caller
    /// did not name as new.
    fn needs_force(&self) -> bool {
        matches!(
            self,
            Self::Regular { .. } | Self::InPlace { dangling: true }
        )
    }
}

/// Replace the regular file `file` with what `write` writes. The table is
/// written beside the file and moved onto it when it is whole, so that an
/// export that fails leaves the old file as it was. The new file has the
/// permissions of the old one. Another name of the old file (a hard link)
/// keeps the old content.
fn replace<T>(
    file: &Path,
    permissions: std::fs::Permissions,
    write: impl FnOnce(File) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let beside = beside(file);
    let created = create(&beside, file)?;
    let moved = created
        .set_permissions(permissions)
        .map_err(|err| Failure::cannot_create(&beside, &err))
        .and_then(|()| write(created))
        .and_then(|written| {
            std::fs::rename(&beside, file).map_err(|err| Failure::cannot_create(file, &err))?;
            Ok(written)
        });
    if moved.is_err() {
        // Do not leave half a table behind.
        let _ = std::fs::remove_file(&beside);
    }
    moved
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
    target: &Target,
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
            match (target, choices.force) {
                (Target::New, _) => format!("{shown} does not exist; nothing was written"),
                (Target::InPlace { dangling: false }, _) => format!(
                    "{shown} exists and is not a regular file; an export writes into it; \
                     nothing was written"
                ),
                (_, true) => format!("{shown} exists and --force replaces it; nothing was written"),
                (_, false) =>
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("buzzard-tdms-cmd-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A forced export that fails after it started to write leaves the
    /// old file as it was, and nothing beside it.
    #[test]
    fn a_failed_replacement_leaves_the_old_file_and_nothing_else() {
        let dir = sandbox("replace");
        let file = dir.join("table.csv");
        std::fs::write(&file, "keep me").unwrap();
        let permissions = std::fs::metadata(&file).unwrap().permissions();

        let failed: Result<(), Failure> = replace(&file, permissions.clone(), |mut to| {
            to.write_all(b"half a table").unwrap();
            // The old file is untouched while the new one is written.
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep me");
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
            Err(Failure::new(ErrorKind::WriteFailed, "the disk is full"))
        });
        assert_eq!(failed.unwrap_err().message, "the disk is full");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep me");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        let done = replace(&file, permissions, |mut to| {
            to.write_all(b"a whole table").unwrap();
            Ok(7)
        });
        assert_eq!(done.unwrap(), 7);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "a whole table");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_target_is_told_by_what_the_name_leads_to() {
        let dir = sandbox("target");
        assert!(matches!(Target::of(&dir.join("new.csv")), Target::New));
        assert!(matches!(Target::of(&dir), Target::Directory));
        std::fs::write(dir.join("old.csv"), "x").unwrap();
        let old = Target::of(&dir.join("old.csv"));
        assert!(matches!(old, Target::Regular { .. }) && old.needs_force());
        assert!(!Target::New.needs_force());
        assert!(!Target::InPlace { dangling: false }.needs_force());
        assert!(Target::InPlace { dangling: true }.needs_force());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
