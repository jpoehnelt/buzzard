//! Conformance against files written by the crate's own generator, whose
//! model of each file's contents is independent of the reader.

mod common;

use buzzard_tdms::build::{FileBuilder, Object, Values};
use buzzard_tdms::{Anomaly, DataType, IndexSource, PropertyValue, Scaling, TdmsFile};
use common::{assert_matches, daqmx_scenario, digital_scenario, open, scenario, shapes};

#[test]
fn every_shape_of_the_cross_product_reads_back() {
    let shapes = shapes();
    assert_eq!(shapes.len(), 14 * 32 + 16);
    for shape in shapes {
        let built = scenario(shape);
        let what = format!("{shape:?}");
        let file = open(&built.bytes, None).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(file.anomalies(), [], "{what}");
        assert_eq!(file.segment_count(), 9, "{what}");
        assert_eq!(file.versions(), [shape.version], "{what}");
        assert_eq!(file.index_source(), IndexSource::DataFile);
        assert_matches(&built, &file, &what);

        // The same through the index file.
        let indexed = open(&built.bytes, Some(&built.index_bytes)).unwrap();
        assert_eq!(indexed.index_source(), IndexSource::IndexFile, "{what}");
        assert_matches(&built, &indexed, &what);
    }
}

#[test]
fn a_stale_or_foreign_index_file_is_ignored() {
    let shape = shapes()[0];
    let built = scenario(shape);
    let other = scenario(common::Shape {
        dtype: DataType::F64,
        ..shape
    });
    let mut flipped = built.index_bytes.clone();
    let last = flipped.len() - 1;
    flipped[40] ^= 0x10;
    let mut longer = built.index_bytes.clone();
    longer.extend_from_slice(&built.index_bytes[..28]);
    let stale = [
        ("shorter", built.index_bytes[..last].to_vec()),
        ("one segment short", built.index_bytes[..last - 27].to_vec()),
        ("longer", longer),
        ("empty", Vec::new()),
        ("of another file", other.index_bytes.clone()),
        ("damaged", flipped),
        ("the data file itself", built.bytes.clone()),
    ];
    for (what, index) in stale {
        let file = open(&built.bytes, Some(&index)).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(
            file.index_source(),
            IndexSource::DataFileIndexRejected,
            "{what}"
        );
        assert_matches(&built, &file, what);
    }
    // An index file that describes a file since appended to.
    let mut grown = built.bytes.clone();
    grown.extend_from_slice(&built.bytes);
    let file = open(&grown, Some(&built.index_bytes)).unwrap();
    assert_eq!(file.index_source(), IndexSource::DataFileIndexRejected);
    assert_eq!(file.segment_count(), 18);
}

#[test]
fn the_index_file_beside_a_data_file_is_found() {
    let built = scenario(shapes()[3]);
    let dir = std::env::temp_dir().join(format!("buzzard-tdms-index-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("log.tdms");
    std::fs::write(&path, &built.bytes).unwrap();
    let file = TdmsFile::open(&path).unwrap();
    assert_eq!(file.index_source(), IndexSource::DataFile);
    std::fs::write(dir.join("log.tdms_index"), &built.index_bytes).unwrap();
    let file = TdmsFile::open(&path).unwrap();
    assert_eq!(file.index_source(), IndexSource::IndexFile);
    assert_matches(&built, &file, "from disk");
    let ignored = buzzard_tdms::OpenOptions::new()
        .index_file(false)
        .open(&path)
        .unwrap();
    assert_eq!(ignored.index_source(), IndexSource::DataFile);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn driver_buffers_read_through_their_scalers() {
    for big_endian in [false, true] {
        for chunks in [1, 4] {
            let built = daqmx_scenario(big_endian, chunks);
            let what = format!("big_endian {big_endian}, {chunks} chunks");
            let file = open(&built.bytes, None).unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(file.anomalies(), [], "{what}");
            assert_matches(&built, &file, &what);

            let ai0 = file.channel("Measured", "ai0").unwrap();
            assert_eq!(ai0.dtype(), DataType::DaqmxRaw);
            assert_eq!(ai0.scalers().len(), 1);
            assert_eq!(
                ai0.scaling(),
                Scaling::Linear {
                    slope: 0.25,
                    intercept: -3.0,
                    scaler: Some(0)
                }
            );
            let raw = ai0.read_all::<i16>().unwrap();
            let scaled = ai0.read_scaled(0..ai0.len()).unwrap();
            for (r, s) in raw.iter().zip(&scaled) {
                assert!((f64::from(*r) * 0.25 - 3.0 - s).abs() < 1e-12);
            }
            // A channel with two scalers has no single raw type.
            let ai1 = file.channel("Measured", "ai1").unwrap();
            assert!(ai1.read_all::<i32>().is_err());
            assert_eq!(ai1.read_scaler::<i32>(0, 0..2).unwrap().len(), 2);
            assert!(ai1.read_scaler::<f64>(1, 0..2).is_err());
            assert!(ai1.read_scaler::<f32>(7, 0..2).is_err());
            assert_eq!(
                file.channel("Measured", "typed").unwrap().dtype(),
                DataType::U16
            );
        }
    }
}

#[test]
fn digital_lines_read_as_bits() {
    let built = digital_scenario();
    let file = open(&built.bytes, None).unwrap();
    assert_matches(&built, &file, "digital lines");
    let line = file.channel("port", "line9").unwrap();
    assert!(line.scalers()[0].digital_line);
    assert_eq!(line.read_all::<u8>().unwrap(), [1, 1, 1, 0, 0, 1]);
}

#[test]
fn typed_reads_check_the_type_and_the_range() {
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(
            Object::channel("g", "f")
                .data(Values::F32(vec![1.0, 2.0, 3.0]))
                .with_unit(),
        )
        .object(Object::channel("g", "flag").data(Values::Bool(vec![1, 0, 1])))
        .object(Object::channel("g", "text").data(Values::String(vec!["a".into(), "bc".into()])));
    let file = TdmsFile::from_bytes(builder.build().bytes).unwrap();
    let f = file.channel("g", "f").unwrap();
    assert_eq!(f.dtype(), DataType::F32WithUnit);
    assert_eq!(f.read_all::<f32>().unwrap(), [1.0, 2.0, 3.0]);
    assert_eq!(f.read_f64(1..3).unwrap(), [2.0, 3.0]);
    assert!(matches!(
        f.read_all::<f64>(),
        Err(buzzard_tdms::Error::TypeMismatch { .. })
    ));
    assert!(matches!(
        f.read::<f32>(2..4),
        Err(buzzard_tdms::Error::OutOfRange { .. })
    ));
    let mut two = [0f32; 2];
    f.read_into(1, &mut two).unwrap();
    assert_eq!(two, [2.0, 3.0]);
    let chunks: Vec<Vec<f32>> = f.chunks(2).map(Result::unwrap).collect();
    assert_eq!(chunks, [vec![1.0, 2.0], vec![3.0]]);

    let flag = file.channel("g", "flag").unwrap();
    assert_eq!(flag.read_all::<u8>().unwrap(), [1, 0, 1]);
    assert_eq!(flag.read_f64(0..3).unwrap(), [1.0, 0.0, 1.0]);
    let text = file.channel("g", "text").unwrap();
    assert_eq!(text.read_strings(1..2).unwrap(), ["bc"]);
    assert!(text.read_all::<u8>().is_err());
    assert!(text.read_f64(0..1).is_err());
    assert!(f.read_strings(0..1).is_err());
    assert!(file.channel("g", "nope").is_none());
    assert!(file.group("nope").is_none());
}

#[test]
fn properties_of_every_type_read_back_in_both_byte_orders() {
    for big_endian in [false, true] {
        let mut root = Object::root();
        let mut channel = Object::channel("g", "c").generated(DataType::I8, 1);
        for (name, value) in common::every_property() {
            root = root.prop(name, value.clone());
            channel = channel.prop(name, value);
        }
        let mut builder = FileBuilder::new();
        builder
            .segment()
            .big_endian(big_endian)
            .object(root)
            .object(channel);
        let built = builder.build();
        let file = open(&built.bytes, None).unwrap();
        assert_eq!(file.properties().len(), 16);
        assert_matches(&built, &file, "every property");
    }
}

#[test]
fn waveform_properties_are_exposed() {
    let start = buzzard_tdms::Timestamp::new(3_850_000_000, 1 << 63);
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(
            Object::channel("g", "wave")
                .data(Values::F64(vec![0.0; 4]))
                .prop("wf_start_time", PropertyValue::Timestamp(start))
                .prop("wf_start_offset", PropertyValue::F64(0.5))
                .prop("wf_increment", PropertyValue::F64(0.001))
                .prop("wf_samples", PropertyValue::I32(4))
                .prop("unit_string", PropertyValue::String("V".into())),
        )
        .object(Object::channel("g", "plain").data(Values::F64(vec![0.0])));
    let file = TdmsFile::from_bytes(builder.build().bytes).unwrap();
    let wave = file.channel("g", "wave").unwrap();
    let waveform = wave.waveform().unwrap();
    assert_eq!(waveform.start_time, Some(start));
    assert_eq!(waveform.samples, Some(4));
    assert!((waveform.x(3) - 0.503).abs() < 1e-12);
    assert_eq!(wave.unit(), Some("V"));
    assert!(file.channel("g", "plain").unwrap().waveform().is_none());
}

#[test]
fn many_identical_segments_collapse_in_the_index() {
    let mut builder = FileBuilder::new();
    let segment = builder.segment();
    for i in 0..40 {
        segment.object(Object::channel("g", &format!("c{i}")).generated(DataType::F64, 10));
    }
    for _ in 0..500 {
        builder.segment().no_metadata();
    }
    let built = builder.build();
    let file = TdmsFile::from_bytes(built.bytes.clone()).unwrap();
    assert_eq!(file.segment_count(), 501);
    // One run of segments and one layout, whatever the segment count.
    assert_eq!(file.index_shape(), (1, 1));
    assert_matches(&built, &file, "collapsed");
}

#[test]
fn an_unfinished_last_segment_is_read_to_the_end_of_the_file() {
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::channel("g", "a").generated(DataType::I32, 4))
        .object(Object::channel("g", "b").generated(DataType::F64, 3));
    builder.segment().no_metadata().chunks(2).unfinished();
    let built = builder.build();
    let file = TdmsFile::from_bytes(built.bytes.clone()).unwrap();
    assert_eq!(
        file.anomalies(),
        [Anomaly::UnfinishedSegment {
            segment: 1,
            offset: built.segment_offsets[1]
        }]
    );
    assert_matches(&built, &file, "unfinished");
    assert!(buzzard_tdms::OpenOptions::new()
        .strict(true)
        .open_source(std::sync::Arc::new(built.bytes), None)
        .is_err());
}
