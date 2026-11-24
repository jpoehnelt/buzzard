//! The `tdms` subcommands: `info` describes a TDMS measurement file and
//! `export` writes its channels as a Parquet or CSV table.

use std::fmt::Write as _;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use buzzard_tdms::export::{self, ExportOptions, Exported};
use buzzard_tdms::{Channel, TdmsFile};
use clap::{Subcommand, ValueEnum};

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
        after_help = "Exit status: 0, or 2 when buzzard cannot open the file. Anomalies \
        are part of the report and do not change the status."
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
    #[command(
        after_help = "Exit status: 0, or 2 on any error. An error leaves no output \
        file behind."
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
    },
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

/// Compression of Parquet column data. CSV ignores it.
#[derive(Clone, Copy, ValueEnum)]
enum Compression {
    Snappy,
    None,
}

/// Runs the subcommand and returns what to print.
pub(crate) fn run(args: &Args) -> Result<String, String> {
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
        } => {
            let work = || export(file, out, channels, *format, *compression, *raw);
            match threads {
                Some(n) => rayon::ThreadPoolBuilder::new()
                    .num_threads(*n)
                    .build()
                    .map_err(|e| format!("cannot start {n} threads: {e}"))?
                    .install(work),
                None => work(),
            }
        }
    }
}

fn open(path: &Path) -> Result<TdmsFile, String> {
    TdmsFile::open(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// `buzzard tdms info`: what the file holds, without reading any raw data.
/// The report is the one of `buzzard_tdms::info`, which the MCP server
/// prints too.
fn info(path: &Path, format: InfoFormat) -> Result<String, String> {
    let file = open(path)?;
    let shown = path.display().to_string();
    match format {
        InfoFormat::Text => Ok(buzzard_tdms::info::to_text(&shown, &file)),
        InfoFormat::Json => buzzard_tdms::info::to_json(&shown, &file)
            .map_err(|e| format!("cannot serialize the report: {e}")),
    }
}

/// The channels a `--channels` value names: one channel as `group/channel`
/// or as its full path, or every channel of a group.
fn select<'a>(file: &'a TdmsFile, name: &str) -> Result<Vec<Channel<'a>>, String> {
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
        None => Err(format!("no channel or group named {name}")),
    }
}

/// `buzzard tdms export`: write the chosen channels as one table.
fn export(
    path: &Path,
    out: &Path,
    names: &[String],
    format: ExportFormat,
    compression: Compression,
    raw: bool,
) -> Result<String, String> {
    let file = open(path)?;
    let mut channels: Vec<Channel<'_>> = Vec::new();
    if names.is_empty() {
        channels.extend(file.channels());
    }
    for name in names {
        for channel in select(&file, name)? {
            if !channels.iter().any(|c| c.path() == channel.path()) {
                channels.push(channel);
            }
        }
    }
    let options = ExportOptions {
        scaled: !raw,
        ..ExportOptions::default()
    };
    let target = File::create(out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    let written = write(
        &file,
        &channels,
        BufWriter::new(target),
        format,
        compression,
        &options,
    );
    let exported = match written {
        Ok(exported) => exported,
        Err(message) => {
            // Do not leave half a table behind.
            let _ = std::fs::remove_file(out);
            return Err(format!("{}: {message}", path.display()));
        }
    };
    let mut text = format!(
        "exported {} rows of {} columns to {}\n",
        exported.rows,
        exported.columns.len(),
        out.display()
    );
    for skipped in &exported.skipped {
        let _ = writeln!(text, "skipped {skipped}: it never held a value");
    }
    Ok(text)
}

fn write(
    file: &TdmsFile,
    channels: &[Channel<'_>],
    out: BufWriter<File>,
    format: ExportFormat,
    compression: Compression,
    options: &ExportOptions,
) -> Result<Exported, String> {
    match format {
        ExportFormat::Csv => {
            export::write_csv(file, channels, out, options).map_err(|e| e.to_string())
        }
        #[cfg(feature = "parquet")]
        ExportFormat::Parquet => {
            let compression = match compression {
                Compression::Snappy => export::parquet::Compression::Snappy,
                Compression::None => export::parquet::Compression::None,
            };
            export::parquet::write_parquet(file, channels, out, options, compression)
                .map_err(|e| e.to_string())
        }
        #[cfg(not(feature = "parquet"))]
        ExportFormat::Parquet => {
            let _ = (compression, out);
            Err("this build has no Parquet support; use --format csv".to_owned())
        }
    }
}
