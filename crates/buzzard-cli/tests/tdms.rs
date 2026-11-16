//! Black-box tests of `buzzard tdms info` and `buzzard tdms export`.

mod common;

use std::fs;
use std::process::{Command, Output};

use buzzard_tdms::build::{FileBuilder, Object, Values};
use buzzard_tdms::{DataType, PropertyValue, Timestamp};
use common::*;

fn run(args: &[&str]) -> (i32, String, String) {
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(BIN)
        .args(args)
        .output()
        .expect("run the buzzard binary");
    (
        status.code().expect("buzzard exits with a status code"),
        String::from_utf8(stdout).expect("stdout is UTF-8"),
        String::from_utf8(stderr).expect("stderr is UTF-8"),
    )
}

/// A small log: two segments, a waveform channel, a string channel and a
/// channel that never has data.
fn sample() -> Vec<u8> {
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::root().prop("author", PropertyValue::String("stand 3".into())))
        .object(Object::group("run").prop("number", PropertyValue::I32(7)))
        .object(
            Object::channel("run", "pressure")
                .data(Values::F64(vec![1.5, 2.5, -3.0]))
                .prop("unit_string", PropertyValue::String("bar".into()))
                .prop(
                    "wf_start_time",
                    PropertyValue::Timestamp(Timestamp::new(3_850_000_000, 0)),
                )
                .prop("wf_increment", PropertyValue::F64(0.5)),
        )
        .object(Object::channel("run", "note").data(Values::String(vec![
            "a".into(),
            "b,c".into(),
            String::new(),
        ])))
        .object(Object::channel("aux", "unused"));
    builder
        .segment()
        .object(Object::channel("run", "pressure").same())
        .object(Object::channel("run", "note").no_data());
    builder.build().bytes
}

fn write_sample(dir: &TempDir) -> String {
    let path = dir.0.join("log.tdms");
    fs::write(&path, sample()).expect("write the sample");
    path.to_string_lossy().into_owned()
}

#[test]
fn info_prints_the_structure_as_text() {
    let dir = TempDir::new("tdms-info-text");
    let path = write_sample(&dir);
    let (status, stdout, stderr) = run(&["tdms", "info", &path]);
    assert_eq!((status, stderr.as_str()), (0, ""));
    let expected = format!(
        "file: {path}\n\
         size: 468 bytes\n\
         segments: 2 (version 4713)\n\
         metadata read from: the data file\n\
         anomalies: none\n\
         properties:\n\
         \x20 author (string) = stand 3\n\
         group 'run' (2 channels)\n\
         \x20 number (i32) = 7\n\
         \x20 channel 'pressure': f64, 6 values, unit bar, increment 0.5\n\
         \x20   unit_string (string) = bar\n\
         \x20   wf_start_time (timestamp) = 2025-12-31T04:26:40.000000000Z\n\
         \x20   wf_increment (f64) = 0.5\n\
         \x20 channel 'note': string, 3 values\n\
         group 'aux' (1 channel)\n\
         \x20 channel 'unused': void, 0 values\n"
    );
    assert_eq!(stdout, expected);
    // The same bytes every time.
    assert_eq!(run(&["tdms", "info", &path]).1, expected);
}

#[test]
fn info_prints_json() {
    let dir = TempDir::new("tdms-info-json");
    let path = write_sample(&dir);
    let (status, stdout, _) = run(&["tdms", "info", &path, "--format", "json"]);
    assert_eq!(status, 0);
    assert_eq!(run(&["tdms", "info", &path, "--format", "json"]).1, stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(json["segments"], 2);
    assert_eq!(json["versions"][0], 4713);
    assert_eq!(json["index"], "data_file");
    assert_eq!(json["anomalies"].as_array().map(Vec::len), Some(0));
    assert_eq!(json["properties"][0]["name"], "author");
    assert_eq!(json["properties"][0]["type"], "string");
    let pressure = &json["groups"][0]["channels"][0];
    assert_eq!(pressure["name"], "pressure");
    assert_eq!(pressure["path"], "/'run'/'pressure'");
    assert_eq!(pressure["type"], "f64");
    assert_eq!(pressure["length"], 6);
    assert_eq!(pressure["unit"], "bar");
    assert_eq!(pressure["waveform"]["increment"], 0.5);
    assert_eq!(
        pressure["waveform"]["start_time"],
        "2025-12-31T04:26:40.000000000Z"
    );
    let start = &pressure["properties"][1];
    assert_eq!(start["seconds"], 3_850_000_000_u64);
    assert_eq!(start["fractions"], 0);
    assert_eq!(json["groups"][1]["channels"][0]["type"], "void");
}

#[test]
fn info_reports_a_truncated_last_segment() {
    let dir = TempDir::new("tdms-info-truncated");
    let bytes = sample();
    let path = dir.0.join("cut.tdms");
    fs::write(&path, &bytes[..bytes.len() - 10]).expect("write the file");
    let path = path.to_string_lossy().into_owned();
    let (status, stdout, _) = run(&["tdms", "info", &path]);
    assert_eq!(status, 0);
    assert!(
        stdout.contains("anomalies:\n  segment 1 at byte")
            && stdout.contains("is truncated: 10 bytes are missing"),
        "{stdout}"
    );
    assert!(
        stdout.contains("channel 'pressure': f64, 4 values"),
        "{stdout}"
    );
    let (_, json, _) = run(&["tdms", "info", &path, "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(json["anomalies"][0]["kind"], "truncated_segment");
    assert_eq!(json["anomalies"][1]["kind"], "partial_chunk");
}

#[test]
fn errors_exit_with_status_2() {
    let dir = TempDir::new("tdms-errors");
    let path = write_sample(&dir);
    let missing = dir.0.join("missing.tdms").to_string_lossy().into_owned();
    let (status, stdout, stderr) = run(&["tdms", "info", &missing]);
    assert_eq!((status, stdout.as_str()), (2, ""));
    assert!(stderr.starts_with("error: "), "{stderr}");

    let other = dir.0.join("other.tdms");
    fs::write(&other, b"not a measurement file").expect("write the file");
    let (status, _, stderr) = run(&["tdms", "info", &other.to_string_lossy()]);
    assert_eq!(status, 2);
    assert!(stderr.contains("not a TDMS file"), "{stderr}");

    let out = dir.0.join("out.csv").to_string_lossy().into_owned();
    let (status, _, stderr) = run(&[
        "tdms",
        "export",
        &path,
        "--out",
        &out,
        "--format",
        "csv",
        "--channels",
        "run/nope",
    ]);
    assert_eq!(status, 2);
    assert!(
        stderr.contains("no channel or group named run/nope"),
        "{stderr}"
    );
    assert!(!dir.0.join("out.csv").exists());
}

#[test]
fn export_writes_csv() {
    let dir = TempDir::new("tdms-export-csv");
    let path = write_sample(&dir);
    let out = dir.0.join("all.csv");
    let out_text = out.to_string_lossy().into_owned();
    let (status, stdout, stderr) = run(&[
        "tdms", "export", &path, "--out", &out_text, "--format", "csv",
    ]);
    assert_eq!((status, stderr.as_str()), (0, ""));
    assert_eq!(
        stdout,
        format!(
            "exported 6 rows of 2 columns to {out_text}\n\
             skipped /'aux'/'unused': it never held a value\n"
        )
    );
    let text = fs::read_to_string(&out).expect("read the export");
    let rows: Vec<&str> = text.lines().collect();
    assert_eq!(rows[0], "run/pressure,run/note");
    assert_eq!(rows[1], "1.5,a");
    assert_eq!(rows[2], "2.5,\"b,c\"");
    assert_eq!(rows.len(), 7);

    // A selection, in the order given; one channel alone has its time.
    let one = dir.0.join("one.csv").to_string_lossy().into_owned();
    let (status, stdout, _) = run(&[
        "tdms",
        "export",
        &path,
        "--out",
        &one,
        "--format",
        "csv",
        "--channels",
        "run/pressure",
        "--threads",
        "2",
    ]);
    assert_eq!(status, 0);
    assert_eq!(stdout, format!("exported 6 rows of 2 columns to {one}\n"));
    let text = fs::read_to_string(&one).expect("read the export");
    assert!(
        text.starts_with("time_s,run/pressure\n0,1.5\n0.5,2.5\n1,-3\n"),
        "{text}"
    );
}

#[cfg(feature = "parquet")]
#[test]
fn export_writes_parquet_by_default_and_is_deterministic() {
    let dir = TempDir::new("tdms-export-parquet");
    // Enough data to make the threads matter.
    let mut builder = FileBuilder::new();
    let segment = builder.segment().interleaved(true);
    for i in 0..20 {
        segment.object(Object::channel("g", &format!("c{i}")).generated(DataType::F32, 5000));
    }
    builder.segment().no_metadata().chunks(3);
    let path = dir.0.join("wide.tdms");
    fs::write(&path, builder.build().bytes).expect("write the file");
    let path = path.to_string_lossy().into_owned();

    let mut outputs = Vec::new();
    for (name, threads) in [("a.parquet", "1"), ("b.parquet", "4"), ("c.parquet", "4")] {
        let out = dir.0.join(name);
        let out_text = out.to_string_lossy().into_owned();
        let (status, stdout, stderr) = run(&[
            "tdms",
            "export",
            &path,
            "--out",
            &out_text,
            "--threads",
            threads,
        ]);
        assert_eq!((status, stderr.as_str()), (0, ""));
        assert_eq!(
            stdout,
            format!("exported 20000 rows of 20 columns to {out_text}\n")
        );
        outputs.push(fs::read(&out).expect("read the export"));
    }
    assert_eq!(&outputs[0][..4], b"PAR1");
    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[1], outputs[2]);

    // The group name selects all of its channels.
    let out = dir.0.join("group.parquet").to_string_lossy().into_owned();
    let (status, stdout, _) = run(&[
        "tdms",
        "export",
        &path,
        "--out",
        &out,
        "--channels",
        "g",
        "--compression",
        "none",
    ]);
    assert_eq!(status, 0);
    assert_eq!(
        stdout,
        format!("exported 20000 rows of 20 columns to {out}\n")
    );
}
