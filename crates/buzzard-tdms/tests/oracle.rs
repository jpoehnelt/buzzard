//! Differential test against npTDMS, run as a separate program.
//!
//! Set `BUZZARD_TDMS_ORACLE` to a Python interpreter that has npTDMS
//! installed (and pytest, for the scenarios npTDMS ships):
//!
//! ```text
//! BUZZARD_TDMS_ORACLE=/path/to/venv/bin/python cargo test -p buzzard-tdms --test oracle
//! ```
//!
//! Without the variable the test prints why it did nothing and passes.
//! With `BUZZARD_TDMS_ORACLE_STRICT` also set, it fails when the scenarios
//! that npTDMS ships cannot be run.
//! `tests/oracle/README.md` describes the harness and its licence boundary.

mod common;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use buzzard_tdms::build::{Built, FileBuilder, Object, Values};
use buzzard_tdms::{Channel, Column, DataType, Property, PropertyValue, Scaling, TdmsFile};
use common::sha256;
use serde_json::{json, Value};

/// A place where this reader and npTDMS disagree, and why that is accepted.
struct Known {
    /// Parts of the file's name, in order, separated by `*`.
    file: &'static str,
    /// A part of the place in the comparison, such as `type` or `outcome`.
    at: &'static str,
    /// Why the two differ, and why this reader's answer is kept.
    why: &'static str,
}

/// Every accepted disagreement between this reader and npTDMS. A
/// difference that is not listed fails the test; so does an entry that no
/// longer matches anything, so the list cannot go stale.
const KNOWN_DIFFERENCES: &[Known] = &[
    Known {
        file: "complex_properties",
        at: "outcome",
        why: "npTDMS has no reader for a property of a complex type and refuses the whole \
              file (NotImplementedError). The format gives both complex types a code, and \
              a value is two floats, so this reader reads them.",
    },
    Known {
        file: "cross_*_complex_f*_interleaved_",
        at: "outcome",
        why: "npTDMS cannot read interleaved channels of the complex types \
              (AttributeError: no from_bytes). They are fixed-size values like any other, \
              so this reader reads them; the generator's model checks the values.",
    },
    Known {
        file: "cross_*_contiguous_",
        at: "group order",
        why: "npTDMS lists the groups that have an object of their own before the groups \
              that only appear in channel paths. This reader lists groups in the order \
              the file first mentions them. The format defines no order.",
    },
    Known {
        file: "ragged_middle_interleaved_",
        at: "outcome",
        why: "A finished interleaved segment whose raw data is not a whole number of rows, \
              with another segment after it: npTDMS raises ValueError (could not \
              broadcast). This reader keeps the whole rows, skips the partial one, reports \
              a PartialChunk anomaly and reads on.",
    },
    Known {
        file: "odd_first_segment_without_metadata",
        at: "outcome",
        why: "A first segment without metadata: npTDMS refuses the file (ValueError). \
              This reader has no channels to give the segment's bytes to, reports them as \
              an UnindexedData anomaly and reads the segments that follow.",
    },
    Known {
        file: "odd_index_length",
        at: "outcome",
        why: "npTDMS ignores the length field of a raw data index. This reader requires \
              it to be the length the format gives (20 bytes, 28 for strings), so an index \
              that is damaged or of an unknown kind is refused and not misread.",
    },
    Known {
        file: "odd_raw_data_without_its_flag",
        at: "channel g/a",
        why: "Bytes after the metadata of a segment whose table of contents says it has \
              no raw data: npTDMS reads them as values. This reader believes the table of \
              contents, reports the bytes as an UnindexedData anomaly and invents no \
              values.",
    },
    Known {
        file: "daqmx_",
        at: "wide: scaled, equal in single precision",
        why: "npTDMS applies a linear scale to 32-bit float raw values in single \
              precision. This reader folds a chain of linear scales into one slope and \
              one intercept and applies them in double precision.",
    },
];

fn bits(value: f64) -> String {
    format!("{:016x}", value.to_bits())
}

fn property_view(p: &Property) -> Value {
    let (kind, value) = match &p.value {
        PropertyValue::I8(v) => ("int", json!(v.to_string())),
        PropertyValue::I16(v) => ("int", json!(v.to_string())),
        PropertyValue::I32(v) => ("int", json!(v.to_string())),
        PropertyValue::I64(v) => ("int", json!(v.to_string())),
        PropertyValue::U8(v) => ("int", json!(v.to_string())),
        PropertyValue::U16(v) => ("int", json!(v.to_string())),
        PropertyValue::U32(v) => ("int", json!(v.to_string())),
        PropertyValue::U64(v) => ("int", json!(v.to_string())),
        PropertyValue::F32(v) => ("float", json!(bits(f64::from(*v)))),
        PropertyValue::F64(v) => ("float", json!(bits(*v))),
        PropertyValue::String(v) => ("str", json!(v)),
        PropertyValue::Bool(v) => ("bool", json!(v)),
        PropertyValue::Timestamp(v) => (
            "timestamp",
            json!([v.seconds.to_string(), v.fractions.to_string()]),
        ),
        PropertyValue::ComplexF32(v) => (
            "complex",
            json!([bits(f64::from(v.re)), bits(f64::from(v.im))]),
        ),
        PropertyValue::ComplexF64(v) => ("complex", json!([bits(v.re), bits(v.im)])),
        _ => ("other", json!(p.value.to_string())),
    };
    json!({"name": p.name, "kind": kind, "value": value})
}

fn properties_view(properties: &[Property]) -> Value {
    Value::Array(properties.iter().map(property_view).collect())
}

/// The name of npTDMS's class for a type.
fn type_name(dtype: DataType) -> Value {
    json!(match dtype {
        DataType::Void => return Value::Null,
        DataType::I8 => "Int8",
        DataType::I16 => "Int16",
        DataType::I32 => "Int32",
        DataType::I64 => "Int64",
        DataType::U8 => "Uint8",
        DataType::U16 => "Uint16",
        DataType::U32 => "Uint32",
        DataType::U64 => "Uint64",
        DataType::F32 => "SingleFloat",
        DataType::F64 => "DoubleFloat",
        DataType::ExtendedFloat => "ExtendedFloat",
        DataType::F32WithUnit => "SingleFloatWithUnit",
        DataType::F64WithUnit => "DoubleFloatWithUnit",
        DataType::ExtendedFloatWithUnit => "ExtendedFloatWithUnit",
        DataType::String => "String",
        DataType::Bool => "Boolean",
        DataType::Timestamp => "TimeStamp",
        DataType::ComplexF32 => "ComplexSingleFloat",
        DataType::ComplexF64 => "ComplexDoubleFloat",
        DataType::DaqmxRaw => "DaqMxRawData",
        _ => "?",
    })
}

fn raw_view(column: &Column) -> Value {
    let dtype = match column.dtype() {
        DataType::I8 => "int8",
        DataType::I16 => "int16",
        DataType::I32 => "int32",
        DataType::I64 => "int64",
        DataType::U8 => "uint8",
        DataType::U16 => "uint16",
        DataType::U32 => "uint32",
        DataType::U64 => "uint64",
        DataType::F32 => "float32",
        DataType::F64 => "float64",
        DataType::Bool => "bool",
        DataType::Timestamp => "timestamp",
        DataType::ComplexF32 => "complex64",
        DataType::ComplexF64 => "complex128",
        DataType::String => "string",
        _ => "?",
    };
    json!({
        "dtype": dtype,
        "length": column.len(),
        "sha256": sha256::hex(&column.to_le_bytes()),
    })
}

fn channel_view(channel: &Channel<'_>) -> Result<Value, buzzard_tdms::Error> {
    let len = channel.len();
    let mut raw = serde_json::Map::new();
    let mut scaled = Value::Null;
    if len > 0 {
        let scalers = channel.scalers();
        if channel.dtype() == DataType::DaqmxRaw && !scalers.iter().any(|s| s.digital_line) {
            for scaler in scalers {
                let column = channel.read_scaler_column(scaler.scale_id, 0..len)?;
                raw.insert(scaler.scale_id.to_string(), raw_view(&column));
            }
        } else {
            raw.insert(String::new(), raw_view(&channel.read_column(0..len)?));
        }
        scaled = match channel.scaling() {
            Scaling::None => Value::Null,
            Scaling::Linear { .. } => {
                let mut text = String::new();
                for v in channel.read_scaled(0..len)? {
                    let _ = write!(text, "{}", bits(v));
                }
                json!(text)
            }
            other => json!(format!("{other:?}")),
        };
    }
    Ok(json!({
        "name": channel.name(),
        "type": type_name(channel.dtype()),
        "length": len,
        "properties": properties_view(channel.properties()),
        "raw": raw,
        "scaled": scaled,
    }))
}

/// This reader's view of a file, in the shape `dump.py` prints.
fn file_view(path: &Path) -> Value {
    let view = || -> Result<Value, buzzard_tdms::Error> {
        let file = TdmsFile::open(path)?;
        let mut groups = Vec::new();
        for group in file.groups() {
            let mut channels = Vec::new();
            for channel in group.channels() {
                channels.push(channel_view(&channel)?);
            }
            groups.push(json!({
                "name": group.name(),
                "properties": properties_view(group.properties()),
                "channels": channels,
            }));
        }
        Ok(json!({"properties": properties_view(file.properties()), "groups": groups}))
    };
    view().unwrap_or_else(|e| json!({"error": e.to_string()}))
}

/// The scaled values of a view as floats.
fn floats(text: &str) -> Vec<f64> {
    text.as_bytes()
        .chunks(16)
        .map(|hex| {
            let hex = std::str::from_utf8(hex).expect("hex digits");
            f64::from_bits(u64::from_str_radix(hex, 16).expect("hex digits"))
        })
        .collect()
}

/// How far apart two lists of scaled values are: `Some(0)` when equal to
/// double precision, `Some(1)` when equal to single precision only, `None`
/// otherwise.
fn distance(ours: &Value, theirs: &Value) -> Option<u8> {
    let (Some(a), Some(b)) = (ours.as_str(), theirs.as_str()) else {
        return (ours == theirs).then_some(0);
    };
    let (a, b) = (floats(a), floats(b));
    if a.len() != b.len() {
        return None;
    }
    let within = |tolerance: f64| {
        a.iter().zip(&b).all(|(x, y)| {
            x.to_bits() == y.to_bits() || (x - y).abs() <= tolerance * x.abs().max(y.abs()).max(1.0)
        })
    };
    if within(1e-12) {
        Some(0)
    } else if within(1e-6) {
        Some(1)
    } else {
        None
    }
}

/// Every place where the two views of one file differ: (place, ours,
/// theirs).
#[expect(
    clippy::too_many_lines,
    reason = "one list of cases, read top to bottom"
)]
fn differences(ours: &Value, theirs: &Value) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut differ = |at: String, a: &Value, b: &Value| {
        let short = |v: &Value| {
            let text = v.to_string();
            text.chars().take(160).collect::<String>()
        };
        out.push((at, short(a), short(b)));
    };
    if ours.get("error").is_some() || theirs.get("error").is_some() {
        if ours.get("error").is_some() != theirs.get("error").is_some() {
            differ("outcome".to_owned(), ours, theirs);
        }
        return out;
    }
    if ours["properties"] != theirs["properties"] {
        differ(
            "file properties".to_owned(),
            &ours["properties"],
            &theirs["properties"],
        );
    }
    // Neither the order of groups nor that of channels is part of the
    // format; a different order is reported once and the contents are
    // compared by name.
    let by_name = |list: &Value| -> BTreeMap<String, Value> {
        list.as_array()
            .into_iter()
            .flatten()
            .map(|item| (item["name"].as_str().unwrap_or("").to_owned(), item.clone()))
            .collect()
    };
    let order = |list: &Value| -> Vec<String> {
        list.as_array()
            .into_iter()
            .flatten()
            .map(|item| item["name"].as_str().unwrap_or("").to_owned())
            .collect()
    };
    let (our_groups, their_groups) = (by_name(&ours["groups"]), by_name(&theirs["groups"]));
    if our_groups.keys().ne(their_groups.keys()) {
        differ(
            "groups".to_owned(),
            &json!(order(&ours["groups"])),
            &json!(order(&theirs["groups"])),
        );
        return out;
    }
    if order(&ours["groups"]) != order(&theirs["groups"]) {
        differ(
            "group order".to_owned(),
            &json!(order(&ours["groups"])),
            &json!(order(&theirs["groups"])),
        );
    }
    for (group, a) in &our_groups {
        let b = &their_groups[group];
        if a["properties"] != b["properties"] {
            differ(
                format!("group {group}: properties"),
                &a["properties"],
                &b["properties"],
            );
        }
        let (our_channels, their_channels) = (by_name(&a["channels"]), by_name(&b["channels"]));
        if our_channels.keys().ne(their_channels.keys()) {
            differ(
                format!("group {group}: channels"),
                &json!(order(&a["channels"])),
                &json!(order(&b["channels"])),
            );
            continue;
        }
        if order(&a["channels"]) != order(&b["channels"]) {
            differ(
                format!("group {group}: channel order"),
                &json!(order(&a["channels"])),
                &json!(order(&b["channels"])),
            );
        }
        for (name, a) in &our_channels {
            let b = &their_channels[name];
            for field in ["type", "length", "properties", "raw"] {
                if a[field] != b[field] {
                    differ(
                        format!("channel {group}/{name}: {field}"),
                        &a[field],
                        &b[field],
                    );
                }
            }
            match distance(&a["scaled"], &b["scaled"]) {
                Some(0) => {}
                Some(_) => differ(
                    format!("channel {group}/{name}: scaled, equal in single precision"),
                    &a["scaled"],
                    &b["scaled"],
                ),
                None => differ(
                    format!("channel {group}/{name}: scaled"),
                    &a["scaled"],
                    &b["scaled"],
                ),
            }
        }
    }
    out
}

/// Whether `name` holds the parts of `pattern`, which `*` separates, in
/// order.
fn matches(pattern: &str, name: &str) -> bool {
    let mut rest = name;
    for part in pattern.split('*') {
        let Some(found) = rest.find(part) else {
            return false;
        };
        rest = &rest[found + part.len()..];
    }
    true
}

fn python(interpreter: &str, args: &[&str]) -> (bool, String) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/oracle/dump.py");
    let output = Command::new(interpreter)
        .arg("-I")
        .arg(script)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {interpreter}: {e}"));
    assert!(
        output.status.success() || output.status.code() == Some(3),
        "dump.py {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (
        output.status.success(),
        String::from_utf8(output.stdout).expect("the script prints UTF-8"),
    )
}

fn save(dir: &Path, name: &str, built: &Built, with_index: bool) {
    fs::write(dir.join(format!("{name}.tdms")), &built.bytes).expect("write a generated file");
    if with_index {
        fs::write(dir.join(format!("{name}.tdms_index")), &built.index_bytes)
            .expect("write an index file");
    }
}

/// Files for cases that the cross product does not reach: truncation,
/// ragged tails, text that is not UTF-8.
#[expect(
    clippy::too_many_lines,
    reason = "one list of cases, read top to bottom"
)]
fn special_cases(dir: &Path) {
    let two = |interleaved: bool, chunks: u64| {
        let mut builder = FileBuilder::new();
        builder
            .segment()
            .interleaved(interleaved)
            .object(Object::channel("g", "a").generated(DataType::I32, 5))
            .object(Object::channel("g", "b").generated(DataType::F64, 5))
            .object(Object::channel("g", "c").generated(DataType::I16, 5));
        builder
            .segment()
            .interleaved(interleaved)
            .chunks(chunks)
            .no_metadata();
        builder
    };
    for interleaved in [false, true] {
        let layout = if interleaved {
            "interleaved"
        } else {
            "contiguous"
        };
        // A file cut short at many places in its last segment.
        let built = two(interleaved, 2).build();
        let last = usize::try_from(built.segment_offsets[1]).expect("small");
        for cut in (last..built.bytes.len()).step_by(7) {
            let mut cut_file = built.clone();
            cut_file.bytes.truncate(cut);
            save(dir, &format!("cut_{layout}_{cut:04}"), &cut_file, false);
        }
        // The same cuts with the length field never written.
        let mut builder = two(interleaved, 2);
        builder.segment().no_metadata().unfinished();
        let built = builder.build();
        let last = usize::try_from(built.data_offsets[2]).expect("small");
        for cut in (last..=built.bytes.len()).step_by(5) {
            let mut cut_file = built.clone();
            cut_file.bytes.truncate(cut);
            save(
                dir,
                &format!("unfinished_{layout}_{cut:04}"),
                &cut_file,
                false,
            );
        }
        // A finished segment whose raw data is not a whole number of
        // chunks, in the middle of the file and at its end.
        for extra in [1usize, 9, 30, 50] {
            let mut builder = two(interleaved, 1);
            builder
                .segment()
                .interleaved(interleaved)
                .no_metadata()
                .extra_raw(&vec![0x5A; extra]);
            save(
                dir,
                &format!("ragged_end_{layout}_{extra:02}"),
                &builder.build(),
                false,
            );
            builder.segment().interleaved(interleaved).no_metadata();
            save(
                dir,
                &format!("ragged_middle_{layout}_{extra:02}"),
                &builder.build(),
                false,
            );
        }
    }
    // A file with a string channel, cut short in its last segment.
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::channel("g", "a").generated(DataType::I32, 3))
        .object(Object::channel("g", "s").data(Values::String(vec!["one".into(), "three".into()])))
        .object(Object::channel("g", "b").generated(DataType::F64, 2));
    builder.segment().no_metadata().chunks(2);
    let built = builder.build();
    let last = usize::try_from(built.segment_offsets[1]).expect("small");
    for cut in (last..built.bytes.len()).step_by(5) {
        let mut cut_file = built.clone();
        cut_file.bytes.truncate(cut);
        save(dir, &format!("cut_strings_{cut:04}"), &cut_file, false);
    }
    // Text that is not UTF-8, and text that ends in a zero byte.
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::root().prop("note", PropertyValue::String("caf\u{7f}!".into())))
        .object(
            Object::channel("g", "t\u{7f}me")
                .data(Values::String(vec!["o\u{7f}k".into(), "fine".into()]))
                .prop("ends", PropertyValue::String("zero\0".into())),
        );
    let mut built = builder.build();
    // The three marker bytes become a byte that is not UTF-8.
    let markers = built.bytes.iter_mut().filter(|b| **b == 0x7F);
    assert_eq!(markers.map(|b| *b = 0xE9).count(), 3);
    save(dir, "not_utf8", &built, false);
    // Files that bend the format, to learn where the two readers draw the
    // line.
    let one = |object: Object| {
        let mut builder = FileBuilder::new();
        builder.segment().object(object);
        builder
    };
    let plain = || Object::channel("g", "a").generated(DataType::I32, 4);
    save(
        dir,
        "odd_index_length",
        &one(plain().index_length(99)).build(),
        false,
    );
    let mut builder = one(plain());
    builder.segment().version(9999).no_metadata();
    save(dir, "odd_version", &builder.build(), false);
    let mut builder = FileBuilder::new();
    builder.segment().no_metadata().extra_raw(&[1, 2, 3, 4]);
    builder.segment().object(plain());
    save(
        dir,
        "odd_first_segment_without_metadata",
        &builder.build(),
        false,
    );
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .interleaved(true)
        .object(Object::channel("g", "text").data(Values::String(vec!["ab".into(), "c".into()])));
    save(
        dir,
        "odd_single_string_interleaved",
        &builder.build(),
        false,
    );
    let mut builder = one(plain());
    builder.segment().no_metadata().toc_xor(1 << 3);
    save(
        dir,
        "odd_raw_data_without_its_flag",
        &builder.build(),
        false,
    );
    // Properties of every kind, the complex ones included.
    let mut root = Object::root();
    for (name, value) in common::every_property() {
        root = root.prop(name, value);
    }
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(root)
        .object(Object::channel("g", "a").generated(DataType::F64, 3));
    save(dir, "complex_properties", &builder.build(), false);
    // Channels of several lengths and a channel declared without values.
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .big_endian(true)
        .chunks(2)
        .object(Object::channel("g", "long").generated(DataType::Timestamp, 6))
        .object(Object::channel("g", "short").generated(DataType::ComplexF32, 1))
        .object(Object::channel("g", "none").generated(DataType::U8, 0))
        .object(Object::channel("g", "text").data(Values::String(vec![
            "αβ".into(),
            String::new(),
            "γ".into(),
            "δ".into(),
        ])));
    save(dir, "mixed_lengths", &builder.build(), true);
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("buzzard-tdms-oracle-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create a temporary directory");
        Self(dir)
    }

    fn text(&self) -> &str {
        self.0.to_str().expect("a UTF-8 temporary path")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the steps of one comparison, in order"
)]
fn the_reader_agrees_with_nptdms() {
    let Ok(interpreter) = std::env::var("BUZZARD_TDMS_ORACLE") else {
        eprintln!(
            "skipped: set BUZZARD_TDMS_ORACLE to a Python interpreter with npTDMS installed \
             to compare this reader with npTDMS"
        );
        return;
    };

    // Our generator's files: the cross product, half of them with an index
    // file, driver buffers, and the special cases.
    let generated = TempDir::new("generated");
    for (number, shape) in common::shapes().into_iter().enumerate() {
        let name = format!(
            "cross_{number:03}_{}_{}_{}_{}chunks_{}_pad{}",
            shape.dtype,
            if shape.big_endian { "be" } else { "le" },
            if shape.interleaved {
                "interleaved"
            } else {
                "contiguous"
            },
            shape.chunks,
            shape.version,
            shape.padding
        );
        save(
            &generated.0,
            &name,
            &common::scenario(shape),
            number % 2 == 0,
        );
    }
    for big_endian in [false, true] {
        for chunks in [1, 3] {
            let name = format!(
                "daqmx_{}_{chunks}chunks",
                if big_endian { "be" } else { "le" }
            );
            save(
                &generated.0,
                &name,
                &common::daqmx_scenario(big_endian, chunks),
                false,
            );
        }
    }
    save(
        &generated.0,
        "daqmx_digital",
        &common::digital_scenario(),
        false,
    );
    special_cases(&generated.0);

    // Files written by npTDMS's own writer.
    let written = TempDir::new("written");
    assert!(
        python(&interpreter, &["write", written.text()]).0,
        "dump.py write failed"
    );

    // The scenarios and data files that the installed npTDMS ships, when
    // they can be run. They are used where they are; nothing is copied
    // into this repository.
    let scenarios = TempDir::new("scenarios");
    let have_scenarios = python(&interpreter, &["scenarios", scenarios.text()]).0;
    if !have_scenarios {
        eprintln!("the scenarios npTDMS ships were not run (is pytest installed?)");
    }
    // CI sets this so that a missing pytest cannot quietly shrink the test.
    let strict = std::env::var_os("BUZZARD_TDMS_ORACLE_STRICT").is_some();
    assert!(
        have_scenarios || !strict,
        "the scenarios npTDMS ships could not be run"
    );
    let (ok, shipped) = python(&interpreter, &["data-dir"]);
    assert!(ok, "dump.py data-dir failed");
    let shipped = shipped.trim().to_owned();
    let have_shipped = Path::new(&shipped).is_dir();

    // The real file in the repository.
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zenodo-7758683");
    let fixtures = fixtures.to_str().expect("a UTF-8 path");
    let mut sources = vec![generated.text(), written.text(), fixtures];
    if have_scenarios {
        sources.push(scenarios.text());
    }
    if have_shipped {
        sources.push(&shipped);
    }
    let mut args = vec!["dump"];
    args.extend(&sources);
    let (ok, dump) = python(&interpreter, &args);
    assert!(ok, "dump.py dump failed");
    let dump: Value = serde_json::from_str(&dump).expect("the dump is JSON");
    let files: BTreeMap<String, Value> =
        serde_json::from_value(dump["files"].clone()).expect("a map of files");
    assert!(
        files.len() > 500,
        "only {} files were compared",
        files.len()
    );

    let mut used = vec![0usize; KNOWN_DIFFERENCES.len()];
    let mut unexplained = String::new();
    let (mut compared, mut refused) = (0, 0);
    for (path, theirs) in &files {
        let ours = file_view(Path::new(path));
        compared += 1;
        if theirs.get("error").is_some() && ours.get("error").is_some() {
            refused += 1;
            eprintln!(
                "both refuse {path}:\n  buzzard: {}\n  npTDMS:  {}",
                ours["error"], theirs["error"]
            );
        }
        let file = Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path);
        for (at, a, b) in differences(&ours, theirs) {
            match KNOWN_DIFFERENCES
                .iter()
                .position(|k| matches(k.file, file) && at.contains(k.at))
            {
                Some(known) => used[known] += 1,
                None => {
                    let _ = writeln!(unexplained, "{file}: {at}\n  buzzard: {a}\n  npTDMS:  {b}");
                }
            }
        }
    }
    eprintln!(
        "compared {compared} files with npTDMS {}; both refused {refused}",
        dump["nptdms"]
    );
    for (known, count) in KNOWN_DIFFERENCES.iter().zip(&used) {
        eprintln!(
            "known difference in {count} places ({} / {}): {}",
            known.file, known.at, known.why
        );
    }
    assert!(
        unexplained.is_empty(),
        "differences from npTDMS:\n{unexplained}"
    );
    if have_scenarios && have_shipped {
        for (known, count) in KNOWN_DIFFERENCES.iter().zip(&used) {
            assert!(
                *count > 0,
                "the known difference {} / {} no longer occurs",
                known.file,
                known.at
            );
        }
    }
}

#[test]
fn sha256_matches_the_standard() {
    assert_eq!(
        sha256::hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256::hex(&[b'a'; 200]),
        "c2a908d98f5df987ade41b5fce213067efbcc21ef2240212a41e54b5e7c28ae5"
    );
}
