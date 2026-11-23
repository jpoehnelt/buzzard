//! The `tdms` subcommands: `info` describes a TDMS measurement file and
//! `export` writes its channels as a Parquet or CSV table.

use std::fmt::Write as _;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use buzzard_tdms::export::{self, ExportOptions, Exported};
use buzzard_tdms::{Channel, IndexSource, Property, PropertyValue, Scaling, TdmsFile, Waveform};
use clap::{Subcommand, ValueEnum};
use serde::Serialize;

#[derive(clap::Args)]
pub(crate) struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the groups, channels, types, lengths and properties of a TDMS
    /// file, its segment count, and anything wrong with it such as a
    /// truncated last segment.
    #[command(
        after_help = "Exit status: 0, or 2 when the file cannot be opened. Anomalies \
        are part of the report and do not change the status."
    )]
    Info {
        /// A TDMS file. The index file beside it is used when it matches.
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
        /// A TDMS file. The index file beside it is used when it matches.
        file: PathBuf,
        /// The file to write.
        #[arg(long, value_name = "PATH")]
        out: PathBuf,
        /// What to export, in order: group/channel for one channel or
        /// group for all channels of a group. Default: every channel.
        #[arg(long, num_args = 1.., value_name = "NAME")]
        channels: Vec<String>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = ExportFormat::Parquet)]
        format: ExportFormat,
        /// Compression of Parquet column data. CSV ignores it.
        #[arg(long, value_enum, default_value_t = Compression::Snappy)]
        compression: Compression,
        /// Export stored values as they are, without applying the linear
        /// scaling a channel's properties describe.
        #[arg(long)]
        raw: bool,
        /// Worker threads for reading and encoding. Default: one per core.
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

/// Compression of Parquet column data; ignored for CSV.
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

#[derive(Serialize)]
struct PropertyJson<'a> {
    name: &'a str,
    #[serde(rename = "type")]
    dtype: &'static str,
    value: serde_json::Value,
    /// For a time: the stored whole seconds since 1904-01-01T00:00:00Z.
    #[serde(skip_serializing_if = "Option::is_none")]
    seconds: Option<i64>,
    /// For a time: the stored fraction in units of 2^-64 s.
    #[serde(skip_serializing_if = "Option::is_none")]
    fractions: Option<u64>,
}

#[derive(Serialize)]
struct AnomalyJson {
    kind: &'static str,
    message: String,
}

#[derive(Serialize)]
struct WaveformJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    start_time: Option<String>,
    start_offset: serde_json::Value,
    increment: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    samples: Option<u64>,
}

#[derive(Serialize)]
struct ScalerJson {
    scale_id: u32,
    #[serde(rename = "type")]
    dtype: &'static str,
    digital_line: bool,
}

#[derive(Serialize)]
struct ChannelJson<'a> {
    name: &'a str,
    path: &'a str,
    #[serde(rename = "type")]
    dtype: &'static str,
    length: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    waveform: Option<WaveformJson>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    scalers: Vec<ScalerJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scaling: Option<String>,
    properties: Vec<PropertyJson<'a>>,
}

#[derive(Serialize)]
struct GroupJson<'a> {
    name: &'a str,
    properties: Vec<PropertyJson<'a>>,
    channels: Vec<ChannelJson<'a>>,
}

#[derive(Serialize)]
struct InfoJson<'a> {
    file: String,
    size: u64,
    segments: u64,
    versions: &'a [u32],
    index: &'static str,
    anomalies: Vec<AnomalyJson>,
    properties: Vec<PropertyJson<'a>>,
    groups: Vec<GroupJson<'a>>,
}

/// A float as JSON: a number, or its name when JSON has no number for it.
fn float(value: f64) -> serde_json::Value {
    serde_json::Number::from_f64(value).map_or_else(
        || serde_json::Value::String(value.to_string()),
        serde_json::Value::Number,
    )
}

fn property_json(property: &Property) -> PropertyJson<'_> {
    use serde_json::Value;
    let mut time = None;
    let value = match &property.value {
        PropertyValue::I8(v) => Value::from(*v),
        PropertyValue::I16(v) => Value::from(*v),
        PropertyValue::I32(v) => Value::from(*v),
        PropertyValue::I64(v) => Value::from(*v),
        PropertyValue::U8(v) => Value::from(*v),
        PropertyValue::U16(v) => Value::from(*v),
        PropertyValue::U32(v) => Value::from(*v),
        PropertyValue::U64(v) => Value::from(*v),
        PropertyValue::F32(v) => float(f64::from(*v)),
        PropertyValue::F64(v) => float(*v),
        PropertyValue::String(v) => Value::from(v.as_str()),
        PropertyValue::Bool(v) => Value::from(*v),
        PropertyValue::Timestamp(v) => {
            time = Some(*v);
            Value::from(v.to_rfc3339())
        }
        PropertyValue::ComplexF32(v) => {
            Value::from(vec![float(f64::from(v.re)), float(f64::from(v.im))])
        }
        PropertyValue::ComplexF64(v) => Value::from(vec![float(v.re), float(v.im)]),
        other => Value::from(other.to_string()),
    };
    PropertyJson {
        name: &property.name,
        dtype: property.dtype.name(),
        value,
        seconds: time.map(|t| t.seconds),
        fractions: time.map(|t| t.fractions),
    }
}

fn index_name(source: IndexSource) -> &'static str {
    match source {
        IndexSource::DataFile => "data_file",
        IndexSource::IndexFile => "index_file",
        IndexSource::DataFileIndexRejected => "index_file_rejected",
    }
}

fn scaling_text(channel: &Channel<'_>) -> Option<String> {
    match channel.scaling() {
        Scaling::None => None,
        Scaling::Linear {
            slope, intercept, ..
        } => Some(format!("linear {slope} {intercept}")),
        Scaling::Unsupported(kind) => Some(format!("unsupported {kind}")),
        _ => Some("unsupported".to_owned()),
    }
}

fn channel_json<'a>(channel: &Channel<'a>) -> ChannelJson<'a> {
    ChannelJson {
        name: channel.name(),
        path: channel.path(),
        dtype: channel.dtype().name(),
        length: channel.len(),
        unit: channel.unit(),
        waveform: channel.waveform().map(|w: Waveform| WaveformJson {
            start_time: w.start_time.map(buzzard_tdms::Timestamp::to_rfc3339),
            start_offset: float(w.start_offset),
            increment: float(w.increment),
            samples: w.samples,
        }),
        scalers: channel
            .scalers()
            .iter()
            .map(|s| ScalerJson {
                scale_id: s.scale_id,
                dtype: s.dtype.name(),
                digital_line: s.digital_line,
            })
            .collect(),
        scaling: scaling_text(channel),
        properties: channel.properties().iter().map(property_json).collect(),
    }
}

fn property_lines(out: &mut String, indent: &str, properties: &[Property]) {
    for p in properties {
        let _ = writeln!(out, "{indent}{} ({}) = {}", p.name, p.dtype, p.value);
    }
}

/// `buzzard tdms info`: what the file holds, without reading any raw data.
fn info(path: &Path, format: InfoFormat) -> Result<String, String> {
    let file = open(path)?;
    match format {
        InfoFormat::Text => Ok(info_text(path, &file)),
        InfoFormat::Json => info_json(path, &file),
    }
}

fn info_json(path: &Path, file: &TdmsFile) -> Result<String, String> {
    let report = InfoJson {
        file: path.display().to_string(),
        size: file.size(),
        segments: file.segment_count(),
        versions: file.versions(),
        index: index_name(file.index_source()),
        anomalies: file
            .anomalies()
            .iter()
            .map(|a| AnomalyJson {
                kind: a.kind(),
                message: a.to_string(),
            })
            .collect(),
        properties: file.properties().iter().map(property_json).collect(),
        groups: file
            .groups()
            .map(|group| GroupJson {
                name: group.name(),
                properties: group.properties().iter().map(property_json).collect(),
                channels: group.channels().map(|c| channel_json(&c)).collect(),
            })
            .collect(),
    };
    serde_json::to_string_pretty(&report)
        .map(|text| text + "\n")
        .map_err(|e| format!("cannot serialize the report: {e}"))
}

fn info_text(path: &Path, file: &TdmsFile) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "file: {}", path.display());
    let _ = writeln!(out, "size: {} bytes", file.size());
    let versions: Vec<String> = file.versions().iter().map(u32::to_string).collect();
    let _ = writeln!(
        out,
        "segments: {} (version {})",
        file.segment_count(),
        versions.join(", ")
    );
    let _ = writeln!(
        out,
        "metadata read from: {}",
        match file.index_source() {
            IndexSource::DataFile => "the data file",
            IndexSource::IndexFile => "the index file",
            IndexSource::DataFileIndexRejected =>
                "the data file (the index file does not match it)",
        }
    );
    if file.anomalies().is_empty() {
        out.push_str("anomalies: none\n");
    } else {
        out.push_str("anomalies:\n");
        for anomaly in file.anomalies() {
            let _ = writeln!(out, "  {anomaly}");
        }
    }
    if !file.properties().is_empty() {
        out.push_str("properties:\n");
        property_lines(&mut out, "  ", file.properties());
    }
    for group in file.groups() {
        let count = group.channels().len();
        let _ = writeln!(
            out,
            "group '{}' ({count} channel{})",
            group.name(),
            if count == 1 { "" } else { "s" }
        );
        property_lines(&mut out, "  ", group.properties());
        for channel in group.channels() {
            let _ = write!(
                out,
                "  channel '{}': {}, {} values",
                channel.name(),
                channel.dtype(),
                channel.len()
            );
            if let Some(unit) = channel.unit() {
                let _ = write!(out, ", unit {unit}");
            }
            if let Some(waveform) = channel.waveform() {
                let _ = write!(out, ", increment {}", waveform.increment);
            }
            if let Some(scaling) = scaling_text(&channel) {
                let _ = write!(out, ", scaling {scaling}");
            }
            out.push('\n');
            for scaler in channel.scalers() {
                let _ = writeln!(
                    out,
                    "    scaler {}: {}{}",
                    scaler.scale_id,
                    scaler.dtype,
                    if scaler.digital_line {
                        ", digital line"
                    } else {
                        ""
                    }
                );
            }
            property_lines(&mut out, "    ", channel.properties());
        }
    }
    out
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
