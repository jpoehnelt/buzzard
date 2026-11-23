//! Tests of a file written by real acquisition software, which is kept in
//! `tests/fixtures` with its source and license. An ignored test reads a
//! directory of files that are not in the repository.

mod common;

use std::path::{Path, PathBuf};

use buzzard_tdms::{Column, DataType, IndexSource, PropertyValue, TdmsFile, Timestamp};
use common::sha256;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/zenodo-7758683/waveforms-point21.tdms")
}

#[test]
fn the_fixture_is_the_published_file() {
    let bytes = std::fs::read(fixture()).unwrap();
    assert_eq!(bytes.len(), 526_053);
    assert_eq!(
        sha256::hex(&bytes),
        "e65dd8e2cc58e406c99a16d494a9f6648c8ac6b31c65495110dc5a4c63b47442"
    );
}

#[test]
fn a_file_from_real_acquisition_software_reads() {
    let file = TdmsFile::open(fixture()).unwrap();
    assert_eq!(file.segment_count(), 2);
    assert_eq!(file.versions(), [4713]);
    assert_eq!(file.anomalies(), []);
    assert_eq!(file.index_source(), IndexSource::DataFile);
    assert_eq!(file.properties().len(), 1);
    assert_eq!(
        file.properties()[0].value.as_str(),
        Some("Waveforms Point21")
    );
    let groups: Vec<_> = file.groups().map(|g| g.name().to_owned()).collect();
    assert_eq!(groups, ["Configuration0_Impact00"]);

    // The hashes are those npTDMS computes for the file.
    let expected = [
        (
            "cDAQ1Mod1_ai0",
            "Newtons",
            "dd847ef1e2cf74d28e20257b1c13cda3b9065de715433469ca2ea9928d2d7405",
        ),
        (
            "cDAQ1Mod1_ai1",
            "g",
            "9bb86d0ef504bdcd08e61f86856b3162d453adc655fccea96bce61cd03e76cce",
        ),
    ];
    let channels: Vec<_> = file.channels().collect();
    assert_eq!(channels.len(), 2);
    for (channel, (name, unit, hash)) in channels.iter().zip(expected) {
        assert_eq!(channel.name(), name);
        assert_eq!(channel.dtype(), DataType::F64);
        assert_eq!(channel.len(), 32_768);
        assert_eq!(channel.unit(), Some(unit));
        let waveform = channel.waveform().unwrap();
        assert_eq!(waveform.increment.to_bits(), 0.000_117_187_5_f64.to_bits());
        assert_eq!(waveform.start_offset.to_bits(), 0.0_f64.to_bits());
        assert_eq!(waveform.samples, Some(32_768));
        assert_eq!(
            waveform.start_time,
            Some(Timestamp::new(3_666_383_833, 8_625_028_834_391_726_050))
        );
        assert_eq!(
            waveform.start_time.unwrap().to_rfc3339(),
            "2020-03-06T23:57:13.467563749Z"
        );
        let column = channel.read_column(0..channel.len()).unwrap();
        assert_eq!(sha256::hex(&column.to_le_bytes()), hash, "{name}");
        // A range in the middle agrees with the whole.
        let Column::F64(all) = column else {
            panic!("a float channel")
        };
        assert_eq!(channel.read::<f64>(1000..1010).unwrap(), all[1000..1010]);
        assert!(all.iter().all(|v| v.is_finite()));
    }
    assert_eq!(
        channels[0].property("NI_SV_AnalogSWTrigger"),
        Some(&PropertyValue::Bool(true))
    );
    assert_eq!(
        channels[0].property("NI_SV_PretriggerSamples"),
        Some(&PropertyValue::I32(8))
    );
    assert_eq!(channels[0].properties().len(), 22);
}

/// Reads every channel of every `.tdms` file in the directory that
/// `BUZZARD_TDMS_SAMPLES` names. The directory is for real files that
/// cannot be redistributed or that are too large for the repository.
///
/// ```text
/// BUZZARD_TDMS_SAMPLES=/data/hot-fire cargo test -p buzzard-tdms --release --test real -- --ignored --nocapture
/// ```
#[test]
#[ignore = "needs BUZZARD_TDMS_SAMPLES to name a directory of TDMS files"]
fn real_files_outside_the_repository() {
    let dir = std::env::var("BUZZARD_TDMS_SAMPLES")
        .expect("set BUZZARD_TDMS_SAMPLES to a directory of .tdms files");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "tdms"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no .tdms files in {dir}");
    for path in paths {
        let file = TdmsFile::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut values = 0u64;
        for channel in file.channels() {
            if channel.dtype() == DataType::Void {
                continue;
            }
            // Read in pieces, so that a channel of many gigabytes fits in memory.
            let mut at = 0;
            while at < channel.len() {
                let end = channel.len().min(at + (1 << 22));
                let scalers = channel.scalers();
                if scalers.len() > 1 {
                    for scaler in scalers {
                        channel
                            .read_scaler_column(scaler.scale_id, at..end)
                            .unwrap_or_else(|e| {
                                panic!("{} {}: {e}", path.display(), channel.path())
                            });
                    }
                } else {
                    channel
                        .read_column(at..end)
                        .unwrap_or_else(|e| panic!("{} {}: {e}", path.display(), channel.path()));
                }
                at = end;
            }
            values += channel.len();
        }
        println!(
            "{}: {} segments, {} channels, {values} values, {} anomalies, index {:?}",
            path.display(),
            file.segment_count(),
            file.channels().count(),
            file.anomalies().len(),
            file.index_source(),
        );
        for anomaly in file.anomalies() {
            println!("  {anomaly}");
        }
    }
}
