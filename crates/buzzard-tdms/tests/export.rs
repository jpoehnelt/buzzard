//! The CSV and Parquet exports.

use buzzard_tdms::build::{FileBuilder, Object, Values};
use buzzard_tdms::export::{write_csv, ExportOptions};
use buzzard_tdms::{Complex64, DataType, PropertyValue, TdmsFile, Timestamp};

fn sample() -> TdmsFile {
    let start = Timestamp::new(3_850_000_000, 0);
    let wave = |object: Object| {
        object
            .prop("wf_start_time", PropertyValue::Timestamp(start))
            .prop("wf_start_offset", PropertyValue::F64(0.0))
            .prop("wf_increment", PropertyValue::F64(0.5))
    };
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::root().prop("author", PropertyValue::String("stand, 3".into())))
        .object(Object::group("run").prop("test", PropertyValue::I32(7)))
        .object(
            wave(Object::channel("run", "pressure").data(Values::F64(vec![1.5, 2.5, -3.0])))
                .prop("unit_string", PropertyValue::String("bar".into())),
        )
        .object(wave(
            Object::channel("run", "valve").data(Values::Bool(vec![1, 0])),
        ))
        .object(wave(
            Object::channel("run", "count").data(Values::I16(vec![7, -8, 9])),
        ))
        .object(wave(Object::channel("run", "z").data(Values::ComplexF64(
            vec![Complex64 { re: 1.0, im: -1.0 }],
        ))))
        .object(wave(Object::channel("run", "note").data(Values::String(
            vec!["plain".into(), "a \"quoted\", one".into(), String::new()],
        ))))
        .object(wave(Object::channel("run", "when").data(
            Values::Timestamp(vec![start, Timestamp::new(i64::MAX, 0)]),
        )))
        .object(wave(Object::channel("run", "never")));
    TdmsFile::from_bytes(builder.build().bytes).expect("a valid file")
}

#[test]
fn csv_is_exact() {
    let file = sample();
    let channels: Vec<_> = file.channels().collect();
    let mut out = Vec::new();
    let exported = write_csv(&file, &channels, &mut out, &ExportOptions::default()).unwrap();
    assert_eq!(exported.rows, 3);
    assert_eq!(exported.skipped, ["/'run'/'never'"]);
    let text = String::from_utf8(out).unwrap();
    assert_eq!(
        text,
        "time_s,run/pressure,run/valve,run/count,run/z.re,run/z.im,run/note,run/when\n\
         0,1.5,true,7,1,-1,plain,2025-12-31T04:26:40.000000000Z\n\
         0.5,2.5,false,-8,,,\"a \"\"quoted\"\", one\",+292277026530-12-04T15:30:07.000000000Z\n\
         1,-3,,9,,,,\n"
    );
    // One row per batch gives the same text.
    let mut small = Vec::new();
    let options = ExportOptions {
        rows_per_batch: Some(1),
        ..ExportOptions::default()
    };
    write_csv(&file, &channels, &mut small, &options).unwrap();
    assert_eq!(String::from_utf8(small).unwrap(), text);
}

#[test]
fn channels_without_common_timing_get_no_time_column() {
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(
            Object::channel("g", "fast")
                .data(Values::F32(vec![1.0, 2.0]))
                .prop("wf_increment", PropertyValue::F64(0.001)),
        )
        .object(
            Object::channel("g", "slow")
                .data(Values::F32(vec![3.0]))
                .prop("wf_increment", PropertyValue::F64(1.0)),
        );
    let file = TdmsFile::from_bytes(builder.build().bytes).unwrap();
    let channels: Vec<_> = file.channels().collect();
    let mut out = Vec::new();
    write_csv(&file, &channels, &mut out, &ExportOptions::default()).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "g/fast,g/slow\n1,3\n2,\n");
    // One of them alone has its time.
    let mut out = Vec::new();
    write_csv(&file, &channels[..1], &mut out, &ExportOptions::default()).unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "time_s,g/fast\n0,1\n0.001,2\n"
    );
}

#[test]
fn scaled_channels_are_exported_in_engineering_units() {
    let mut builder = FileBuilder::new();
    builder.segment().object(
        Object::channel("g", "raw")
            .data(Values::I16(vec![0, 100, -100]))
            .prop(
                "NI_Scaling_Status",
                PropertyValue::String("unscaled".into()),
            )
            .prop("NI_Number_Of_Scales", PropertyValue::U32(1))
            .prop(
                "NI_Scale[0]_Scale_Type",
                PropertyValue::String("Linear".into()),
            )
            .prop("NI_Scale[0]_Linear_Slope", PropertyValue::F64(0.5))
            .prop("NI_Scale[0]_Linear_Y_Intercept", PropertyValue::F64(1.0)),
    );
    let file = TdmsFile::from_bytes(builder.build().bytes).unwrap();
    let channels: Vec<_> = file.channels().collect();
    let mut out = Vec::new();
    write_csv(&file, &channels, &mut out, &ExportOptions::default()).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "g/raw\n1\n51\n-49\n");
    let mut out = Vec::new();
    let raw = ExportOptions {
        scaled: false,
        ..ExportOptions::default()
    };
    write_csv(&file, &channels, &mut out, &raw).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "g/raw\n0\n100\n-100\n");
    assert_eq!(channels[0].dtype(), DataType::I16);
}

#[cfg(feature = "parquet")]
mod parquet {
    use std::fs::File;

    use arrow_array::cast::AsArray;
    use arrow_array::types::{Float64Type, Int16Type, TimestampNanosecondType};
    use arrow_array::Array;
    use buzzard_tdms::export::parquet::{write_parquet, Compression};
    use buzzard_tdms::export::ExportOptions;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    use super::sample;

    #[test]
    fn parquet_round_trips_values_and_metadata() {
        let file = sample();
        let channels: Vec<_> = file.channels().collect();
        for (compression, batch) in [(Compression::Snappy, None), (Compression::None, Some(2))] {
            let options = ExportOptions {
                rows_per_batch: batch,
                ..ExportOptions::default()
            };
            let mut first = Vec::new();
            let exported =
                write_parquet(&file, &channels, &mut first, &options, compression).unwrap();
            assert_eq!(exported.rows, 3);
            assert_eq!(exported.columns.len(), 8);
            // Byte-identical when written again.
            let mut second = Vec::new();
            write_parquet(&file, &channels, &mut second, &options, compression).unwrap();
            assert_eq!(first, second);

            let path = std::env::temp_dir().join(format!(
                "buzzard-tdms-export-{}-{}.parquet",
                std::process::id(),
                batch.unwrap_or(0)
            ));
            std::fs::write(&path, &first).unwrap();
            let builder =
                ParquetRecordBatchReaderBuilder::try_new(File::open(&path).unwrap()).unwrap();
            let metadata = std::sync::Arc::clone(builder.metadata());
            let batches: Vec<_> = builder.build().unwrap().map(Result::unwrap).collect();
            std::fs::remove_file(&path).unwrap();

            let keys: Vec<(String, String)> = metadata
                .file_metadata()
                .key_value_metadata()
                .unwrap()
                .iter()
                .map(|kv| (kv.key.clone(), kv.value.clone().unwrap_or_default()))
                .collect();
            let get = |key: &str| {
                keys.iter()
                    .find(|(k, _)| k == key)
                    .map_or_else(|| panic!("no key {key}"), |(_, v)| v.as_str())
            };
            assert_eq!(get("tdms.file.author"), "stand, 3");
            assert_eq!(get("tdms.group.run.test"), "7");
            assert_eq!(get("tdms.channel.run/pressure.unit_string"), "bar");
            assert_eq!(get("buzzard.channel.run/count.dtype"), "i16");
            assert_eq!(get("buzzard.channel.run/valve.length"), "2");
            assert_eq!(get("buzzard.time.column"), "time_s");
            assert_eq!(get("buzzard.time.increment_s"), "0.5");
            assert_eq!(get("buzzard.time.start"), "2025-12-31T04:26:40.000000000Z");
            assert_eq!(
                metadata.num_row_groups(),
                if batch.is_some() { 2 } else { 1 }
            );

            let rows: usize = batches.iter().map(arrow_array::RecordBatch::num_rows).sum();
            assert_eq!(rows, 3);
            let batch = &batches[0];
            let names: Vec<String> = batch
                .schema()
                .fields()
                .iter()
                .map(|f| f.name().clone())
                .collect();
            assert_eq!(names, exported.columns);
            let time = batch.column(0).as_primitive::<Float64Type>();
            assert_eq!(time.values()[..2], [0.0, 0.5]);
            let pressure = batch.column(1).as_primitive::<Float64Type>();
            assert_eq!(pressure.values()[..2], [1.5, 2.5]);
            let valve = batch.column(2).as_boolean();
            assert!(valve.value(0) && !valve.value(1));
            let count = batch.column(3).as_primitive::<Int16Type>();
            assert_eq!(count.values()[..2], [7, -8]);
            let re = batch.column(4).as_primitive::<Float64Type>();
            assert!(re.is_valid(0) && re.is_null(1));
            let note = batch.column(6).as_string::<i32>();
            assert_eq!(note.value(1), "a \"quoted\", one");
            let when = batch.column(7).as_primitive::<TimestampNanosecondType>();
            assert_eq!(when.value(0), 1_767_155_200_000_000_000);
            // A time beyond what nanoseconds can hold is missing.
            assert!(when.is_null(1));
            // The valve channel has ended by the last row.
            let last = batches.last().unwrap();
            assert!(last.column(2).is_null(last.num_rows() - 1));
        }
    }

    #[test]
    fn an_empty_selection_is_a_valid_file() {
        let file = sample();
        let mut out = Vec::new();
        let exported = write_parquet(
            &file,
            &[],
            &mut out,
            &ExportOptions::default(),
            Compression::Snappy,
        )
        .unwrap();
        assert_eq!(exported.rows, 0);
        assert_eq!(&out[..4], b"PAR1");
    }
}
