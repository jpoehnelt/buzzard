//! The report of `buzzard tdms info`: what a file holds, without any of
//! its raw data.
//!
//! [`to_text`] is the report for a person. With the `json` feature,
//! [`report`] is the same as a value that serializes to the JSON of
//! `buzzard tdms info --format json`, and [`to_json`] is that text.

use std::fmt::Write as _;

use crate::{Channel, IndexSource, Property, Scaling, TdmsFile};

#[cfg(feature = "json")]
pub use json::{report, to_json, Info};

fn scaling_text(channel: &Channel<'_>) -> Option<String> {
    match channel.scaling() {
        Scaling::None => None,
        Scaling::Linear {
            slope, intercept, ..
        } => Some(format!("linear {slope} {intercept}")),
        Scaling::Unsupported(kind) => Some(format!("unsupported {kind}")),
    }
}

fn property_lines(out: &mut String, indent: &str, properties: &[Property]) {
    for p in properties {
        let _ = writeln!(out, "{indent}{} ({}) = {}", p.name, p.dtype, p.value);
    }
}

/// The report as text: the file, its size, segments, and anomalies, then
/// each group with its channels and properties.
///
/// `shown` is the path as the report names it. A caller that prints the
/// report for a person passes the path with its control characters
/// escaped. The names and values from the file are printed as they are,
/// so a property that holds a line break spans two lines.
pub fn to_text(shown: &str, file: &TdmsFile) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "file: {shown}");
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

/// The JSON form of the report.
#[cfg(feature = "json")]
mod json {
    use serde::Serialize;

    use super::scaling_text;
    use crate::{Channel, IndexSource, Property, PropertyValue, TdmsFile, Timestamp, Waveform};

    #[derive(Debug, Serialize)]
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

    #[derive(Debug, Serialize)]
    struct AnomalyJson {
        kind: &'static str,
        message: String,
    }

    #[derive(Debug, Serialize)]
    struct WaveformJson {
        #[serde(skip_serializing_if = "Option::is_none")]
        start_time: Option<String>,
        start_offset: serde_json::Value,
        increment: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        samples: Option<u64>,
    }

    #[derive(Debug, Serialize)]
    struct ScalerJson {
        scale_id: u32,
        #[serde(rename = "type")]
        dtype: &'static str,
        digital_line: bool,
    }

    #[derive(Debug, Serialize)]
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

    #[derive(Debug, Serialize)]
    struct GroupJson<'a> {
        name: &'a str,
        properties: Vec<PropertyJson<'a>>,
        channels: Vec<ChannelJson<'a>>,
    }

    /// What a TDMS file holds, in the shape of `buzzard tdms info --format
    /// json`: `file`, `size`, `segments`, `versions`, `index`, `anomalies`,
    /// `properties`, and `groups` with their channels. Make one with
    /// [`report`] and serialize it.
    #[derive(Debug, Serialize)]
    pub struct Info<'a> {
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

    fn channel_json<'a>(channel: &Channel<'a>) -> ChannelJson<'a> {
        ChannelJson {
            name: channel.name(),
            path: channel.path(),
            dtype: channel.dtype().name(),
            length: channel.len(),
            unit: channel.unit(),
            waveform: channel.waveform().map(|w: Waveform| WaveformJson {
                start_time: w.start_time.map(Timestamp::to_rfc3339),
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

    /// The report on `file`. `shown` is the path as the report names it,
    /// in the field `file`.
    pub fn report<'a>(shown: &str, file: &'a TdmsFile) -> Info<'a> {
        Info {
            file: shown.to_owned(),
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
        }
    }

    /// The report as indented JSON that ends in a line break, as
    /// `buzzard tdms info --format json` prints it.
    ///
    /// # Errors
    ///
    /// Returns the error of the serializer. No report is known to cause
    /// one.
    pub fn to_json(shown: &str, file: &TdmsFile) -> serde_json::Result<String> {
        serde_json::to_string_pretty(&report(shown, file)).map(|text| text + "\n")
    }
}
