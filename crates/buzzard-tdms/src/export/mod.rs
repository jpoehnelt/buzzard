//! Export of channels as one table: Parquet (behind the `parquet` feature)
//! and CSV.
//!
//! The table has one column per channel (two for a complex channel) and as
//! many rows as the longest channel; shorter channels are padded with
//! missing values. When every exported channel has the same waveform
//! timing, a leading `time_s` column holds each row's time in seconds
//! (`wf_start_offset + row * wf_increment`), so the timing is in the data
//! and not only in the metadata.
//!
//! Channels are read a batch of rows at a time, in parallel, so a file of
//! many gigabytes is exported in bounded memory.

use std::io::Write;

use crate::error::{Error, Result};
use crate::file::{Channel, TdmsFile, Waveform};
use crate::scaling::Scaling;
use crate::types::{Column, DataType};

#[cfg(feature = "parquet")]
pub mod parquet;

/// Name of the generated time column.
pub const TIME_COLUMN: &str = "time_s";

/// Choices common to every export format.
#[derive(Clone, Debug)]
pub struct ExportOptions {
    /// Rows read and written at once. `None` picks a count that keeps a
    /// batch near 128 MiB.
    pub rows_per_batch: Option<u64>,
    /// Apply linear scaling to channels that describe one, and export them
    /// as 64-bit floats in engineering units. On by default.
    pub scaled: bool,
    /// Add the `time_s` column when the channels' waveform timing agrees.
    /// On by default.
    pub time_column: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            rows_per_batch: None,
            scaled: true,
            time_column: true,
        }
    }
}

/// What an export wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Exported {
    /// Number of rows.
    pub rows: u64,
    /// The names of the columns, in order.
    pub columns: Vec<String>,
    /// The paths of the channels that were left out because they have no
    /// data type: channels that never held a value.
    pub skipped: Vec<String>,
}

/// One exported channel.
pub(crate) struct Planned<'a> {
    pub(crate) channel: Channel<'a>,
    pub(crate) name: String,
    /// The scaler raw values come from, for a driver channel.
    scaler: Option<u32>,
    /// Slope and intercept, when the channel is exported scaled.
    linear: Option<(f64, f64)>,
    /// The type of the exported values.
    pub(crate) dtype: DataType,
}

pub(crate) struct Plan<'a> {
    pub(crate) file: &'a TdmsFile,
    pub(crate) columns: Vec<Planned<'a>>,
    pub(crate) rows: u64,
    pub(crate) time: Option<Waveform>,
    pub(crate) skipped: Vec<String>,
    pub(crate) batch: u64,
}

impl<'a> Plan<'a> {
    pub(crate) fn new(
        file: &'a TdmsFile,
        channels: &[Channel<'a>],
        options: &ExportOptions,
    ) -> Result<Self> {
        let mut columns: Vec<Planned<'a>> = Vec::new();
        let mut skipped = Vec::new();
        let mut row_bytes = 8u64;
        for &channel in channels {
            if channel.dtype() == DataType::Void {
                skipped.push(channel.path().to_owned());
                continue;
            }
            let scaling = if options.scaled {
                channel.scaling()
            } else {
                Scaling::None
            };
            let (scaler, linear, dtype) = if let Scaling::Linear {
                slope,
                intercept,
                scaler,
            } = scaling
            {
                (scaler, Some((slope, intercept)), DataType::F64)
            } else {
                let dtype = channel.storage().map_err(|_| {
                    Error::Export(format!(
                        "{} has several scalers and no linear scaling to choose one",
                        channel.path()
                    ))
                })?;
                (None, None, dtype.0.without_unit())
            };
            let mut name = format!("{}/{}", channel.group_name(), channel.name());
            let mut copy = 2;
            while columns.iter().any(|c| c.name == name) {
                name = format!("{}/{}#{copy}", channel.group_name(), channel.name());
                copy += 1;
            }
            row_bytes += dtype.size().unwrap_or(32);
            columns.push(Planned {
                channel,
                name,
                scaler,
                linear,
                dtype,
            });
        }
        let rows = columns.iter().map(|c| c.channel.len()).max().unwrap_or(0);
        let mut time = None;
        if options.time_column && !columns.is_empty() {
            let first = columns[0].channel.waveform();
            let same = |w: Option<Waveform>| match (w, first) {
                (Some(a), Some(b)) => {
                    a.increment.to_bits() == b.increment.to_bits()
                        && a.start_offset.to_bits() == b.start_offset.to_bits()
                        && a.start_time == b.start_time
                }
                _ => false,
            };
            if columns.iter().all(|c| same(c.channel.waveform())) {
                time = first;
            }
        }
        let batch = options
            .rows_per_batch
            .unwrap_or((128 << 20) / row_bytes)
            .max(1);
        Ok(Self {
            file,
            columns,
            rows,
            time,
            skipped,
            batch,
        })
    }

    /// The names of the table's columns.
    pub(crate) fn names(&self) -> Vec<String> {
        let mut names = Vec::new();
        if self.time.is_some() {
            names.push(TIME_COLUMN.to_owned());
        }
        for column in &self.columns {
            if matches!(column.dtype, DataType::ComplexF32 | DataType::ComplexF64) {
                names.push(format!("{}.re", column.name));
                names.push(format!("{}.im", column.name));
            } else {
                names.push(column.name.clone());
            }
        }
        names
    }

    /// Reads the rows `start..end` of every column. A column holds fewer
    /// values when its channel ends inside the range.
    pub(crate) fn read(&self, start: u64, end: u64) -> Result<Vec<Column>> {
        // Channels that cover the same part of the range are read together.
        let mut out: Vec<Option<Column>> = self.columns.iter().map(|_| None).collect();
        let mut todo: Vec<usize> = (0..self.columns.len()).collect();
        while let Some(&first) = todo.first() {
            let range_of = |at: usize| {
                let len = self.columns[at].channel.len();
                start.min(len)..end.min(len)
            };
            let range = range_of(first);
            let (now, later): (Vec<usize>, Vec<usize>) =
                todo.iter().partition(|&&at| range_of(at) == range);
            todo = later;
            let channels: Vec<_> = now
                .iter()
                .map(|&at| (self.columns[at].channel, self.columns[at].scaler))
                .collect();
            let read = self.file.read_columns_of(&channels, range)?;
            for (at, column) in now.into_iter().zip(read) {
                out[at] = Some(match self.columns[at].linear {
                    Some((slope, intercept)) => {
                        let mut values = column.into_f64().ok_or(Error::TypeMismatch {
                            stored: self.columns[at].channel.dtype(),
                            requested: "f64",
                        })?;
                        for v in &mut values {
                            *v = *v * slope + intercept;
                        }
                        Column::F64(values)
                    }
                    None => column,
                });
            }
        }
        Ok(out.into_iter().flatten().collect())
    }

    /// The table's metadata as ordered key-value pairs: every property of
    /// the file, of the groups of the exported channels and of the
    /// channels, and the timing.
    #[cfg(feature = "parquet")]
    pub(crate) fn metadata(&self) -> Vec<(String, String)> {
        let mut out = vec![(
            "buzzard.tdms.version".to_owned(),
            env!("CARGO_PKG_VERSION").to_owned(),
        )];
        let mut props = |prefix: &str, props: &[crate::types::Property]| {
            for p in props {
                out.push((format!("{prefix}{}", p.name), p.value.to_string()));
            }
        };
        props("tdms.file.", self.file.properties());
        let mut groups: Vec<&str> = Vec::new();
        for column in &self.columns {
            let group = column.channel.group_name();
            if !groups.contains(&group) {
                groups.push(group);
                if let Some(found) = self.file.group(group) {
                    props(&format!("tdms.group.{group}."), found.properties());
                }
            }
        }
        for column in &self.columns {
            props(
                &format!("tdms.channel.{}.", column.name),
                column.channel.properties(),
            );
        }
        for column in &self.columns {
            let key = |what: &str| format!("buzzard.channel.{}.{what}", column.name);
            out.push((key("path"), column.channel.path().to_owned()));
            out.push((key("dtype"), column.channel.dtype().name().to_owned()));
            out.push((key("length"), column.channel.len().to_string()));
            if let Some((slope, intercept)) = column.linear {
                out.push((key("scaling"), format!("linear {slope} {intercept}")));
            }
            if let Some(w) = column.channel.waveform() {
                if let Some(start) = w.start_time {
                    out.push((key("wf_start_time"), start.to_rfc3339()));
                }
                out.push((key("wf_start_offset"), w.start_offset.to_string()));
                out.push((key("wf_increment"), w.increment.to_string()));
            }
        }
        if let Some(w) = self.time {
            out.push(("buzzard.time.column".to_owned(), TIME_COLUMN.to_owned()));
            out.push((
                "buzzard.time.increment_s".to_owned(),
                w.increment.to_string(),
            ));
            out.push((
                "buzzard.time.start_offset_s".to_owned(),
                w.start_offset.to_string(),
            ));
            if let Some(start) = w.start_time {
                out.push(("buzzard.time.start".to_owned(), start.to_rfc3339()));
            }
        }
        out
    }

    pub(crate) fn done(self) -> Exported {
        Exported {
            rows: self.rows,
            columns: self.names(),
            skipped: self.skipped,
        }
    }
}

fn csv_text(out: &mut Vec<u8>, text: &str) {
    if text.contains(['"', ',', '\n', '\r']) || text.starts_with(' ') || text.ends_with(' ') {
        out.push(b'"');
        out.extend(text.replace('"', "\"\"").bytes());
        out.push(b'"');
    } else {
        out.extend(text.bytes());
    }
}

/// Writes the value at `at` of a column as one or two CSV fields.
fn csv_value(out: &mut Vec<u8>, column: &Column, at: usize) {
    use std::io::Write as _;
    // Writing to a `Vec` cannot fail.
    let _ = match column {
        Column::I8(v) => write!(out, "{}", v[at]),
        Column::I16(v) => write!(out, "{}", v[at]),
        Column::I32(v) => write!(out, "{}", v[at]),
        Column::I64(v) => write!(out, "{}", v[at]),
        Column::U8(v) => write!(out, "{}", v[at]),
        Column::U16(v) => write!(out, "{}", v[at]),
        Column::U32(v) => write!(out, "{}", v[at]),
        Column::U64(v) => write!(out, "{}", v[at]),
        Column::F32(v) => write!(out, "{}", v[at]),
        Column::F64(v) => write!(out, "{}", v[at]),
        Column::Bool(v) => write!(out, "{}", v[at] != 0),
        Column::Timestamp(v) => write!(out, "{}", v[at].to_rfc3339()),
        Column::ComplexF32(v) => write!(out, "{},{}", v[at].re, v[at].im),
        Column::ComplexF64(v) => write!(out, "{},{}", v[at].re, v[at].im),
        Column::String(v) => {
            csv_text(out, &v[at]);
            Ok(())
        }
    };
}

/// Writes the channels as CSV: a header row of column names, then one row
/// per sample. Fields follow RFC 4180 quoting, lines end in a line feed,
/// numbers are printed in their shortest form that reads back exactly,
/// times as RFC 3339 in UTC, and a channel that has ended leaves its field
/// empty. CSV carries no metadata; `buzzard tdms info --format json` and
/// the Parquet export do.
///
/// # Errors
///
/// When a channel cannot be read or exported, or writing fails.
pub fn write_csv<W: Write>(
    file: &TdmsFile,
    channels: &[Channel<'_>],
    mut out: W,
    options: &ExportOptions,
) -> Result<Exported> {
    let plan = Plan::new(file, channels, options)?;
    let mut line = Vec::new();
    for (i, name) in plan.names().iter().enumerate() {
        if i > 0 {
            line.push(b',');
        }
        csv_text(&mut line, name);
    }
    line.push(b'\n');
    out.write_all(&line)?;
    // Text is much larger than the values; keep batches modest.
    let batch = plan.batch.min(1 << 16);
    let mut start = 0;
    while start < plan.rows {
        let end = plan.rows.min(start + batch);
        let columns = plan.read(start, end)?;
        line.clear();
        for row in 0..usize::try_from(end - start).unwrap_or(0) {
            let mut first = true;
            if let Some(w) = plan.time {
                let _ = write!(line, "{}", w.x(start + row as u64));
                first = false;
            }
            for column in &columns {
                if !first {
                    line.push(b',');
                }
                first = false;
                if row < column.len() {
                    csv_value(&mut line, column, row);
                } else if matches!(column, Column::ComplexF32(_) | Column::ComplexF64(_)) {
                    line.push(b',');
                }
            }
            line.push(b'\n');
        }
        out.write_all(&line)?;
        start = end;
    }
    out.flush()?;
    Ok(plan.done())
}
