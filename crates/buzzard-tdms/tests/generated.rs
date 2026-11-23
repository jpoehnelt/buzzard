//! Conformance tests on files written by the crate's own generator, whose
//! model of each file's contents is independent of the reader.

mod common;

use buzzard_tdms::build::{Built, FileBuilder, Object, ScalerSpec, Values};
use buzzard_tdms::{Anomaly, DataType, IndexSource, PropertyValue, ReadAt, Scaling, TdmsFile};
use common::{assert_matches, daqmx_scenario, digital_scenario, open, scenario, shapes, slice};

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
    // An index file of a data file that has had segments appended since.
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
    assert_eq!(file.channels().count(), 16);
    let mut patterns = std::collections::HashSet::new();
    for bit in 0..16u64 {
        let line = file.channel("port", &format!("line{bit}")).unwrap();
        assert!(line.scalers()[0].digital_line);
        let values = line.read_all::<u8>().unwrap();
        assert_eq!(values, common::bit_pattern(bit, 24), "line {bit}");
        assert!(values.iter().all(|&v| v <= 1), "line {bit}");
        patterns.insert(values);
    }
    // No two lines are alike, so a bit of the wrong line cannot pass.
    assert_eq!(patterns.len(), 16);
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
    // A string channel between them. Every segment has strings of the same
    // lengths with different contents.
    let strings = ["ab", "cd", "", "efg", "h"].map(str::to_owned).to_vec();
    segment.object(Object::channel("g", "text").data(Values::String(strings)));
    for i in 40..45 {
        segment.object(Object::channel("g", &format!("c{i}")).generated(DataType::I16, 3));
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
    let text = file.channel("g", "text").unwrap();
    let Some(Values::String(expected)) = &built.channels[40].values else {
        panic!("the model has the strings");
    };
    assert_eq!(text.len(), 2505);
    assert_ne!(expected[..5], expected[5..10]);
    assert_eq!(text.read_strings(2..2503).unwrap(), expected[2..2503]);
    assert_eq!(text.read_strings(1249..1256).unwrap(), expected[1249..1256]);
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

/// Reads a file's channels together and in steps, and compares every value
/// with the model.
fn check_reads(what: &str, built: &Built, steps: &[u64]) {
    let file = open(&built.bytes, None).unwrap_or_else(|e| panic!("{what}: {e}"));
    assert_eq!(file.anomalies(), [], "{what}");
    assert_matches(built, &file, what);
    let channels: Vec<_> = file.channels().collect();
    let model = |channel: &buzzard_tdms::Channel<'_>| {
        built
            .channels
            .iter()
            .find(|c| c.group == channel.group_name() && c.name == channel.name())
            .and_then(|c| c.values.as_ref())
            .unwrap_or_else(|| panic!("{what}: {} has no model", channel.path()))
    };
    let len = channels[0].len();
    assert!(channels.iter().all(|c| c.len() == len), "{what}");
    for range in [0..len, len / 3..len - len / 5, len - 1..len] {
        let together = file
            .read_columns(&channels, range.clone())
            .unwrap_or_else(|e| panic!("{what}: {range:?}: {e}"));
        assert_eq!(together.len(), channels.len(), "{what}");
        for (channel, column) in channels.iter().zip(&together) {
            assert!(
                *column == slice(model(channel), range.start, range.end),
                "{what}: {} read with the others, {range:?}",
                channel.path()
            );
        }
    }
    for &step in steps {
        for channel in &channels {
            let expected = model(channel);
            let mut from = 0;
            while from < len {
                let to = len.min(from + step);
                let part = channel
                    .read_column(from..to)
                    .unwrap_or_else(|e| panic!("{what}: {from}..{to}: {e}"));
                assert!(
                    part == slice(expected, from, to),
                    "{what}: {} in steps of {step}, {from}..{to}",
                    channel.path()
                );
                from = to;
            }
        }
    }
}

/// Files large enough to cross every size at which the reader changes how
/// it reads.
fn large_files() -> Vec<(String, Built, Vec<u64>)> {
    let steps = vec![65_535, 65_537, 524_288];
    let mut files = Vec::new();
    // More values than one task reads of a contiguous channel, in one
    // chunk and in a hundred.
    for big_endian in [false, true] {
        for chunks in [1, 100] {
            let mut builder = FileBuilder::new();
            let segment = builder.segment();
            segment.big_endian(big_endian).chunks(chunks);
            for name in ["x", "y", "z"] {
                let n = 700_000 / chunks;
                segment.object(Object::channel("g", name).generated(DataType::F64, n));
            }
            files.push((
                format!("contiguous, big-endian {big_endian}, {chunks} chunks"),
                builder.build(),
                steps.clone(),
            ));
        }
    }
    // Rows of mixed widths, many times the block of rows read at once.
    for big_endian in [false, true] {
        let mut builder = FileBuilder::new();
        let segment = builder.segment();
        segment.big_endian(big_endian).interleaved(true);
        for (name, dtype) in [
            ("p", DataType::F64),
            ("q", DataType::I16),
            ("r", DataType::F64),
            ("t", DataType::Timestamp),
        ] {
            segment.object(Object::channel("g", name).generated(dtype, 300_000));
        }
        files.push((
            format!("interleaved, big-endian {big_endian}"),
            builder.build(),
            steps.clone(),
        ));
    }
    // A row wider than the reader buffers: 8,300 channels of 8 bytes.
    let mut builder = FileBuilder::new();
    let segment = builder.segment();
    segment.interleaved(true).chunks(2);
    for i in 0..8_300 {
        segment.object(Object::channel("wide", &format!("c{i}")).generated(DataType::F64, 12));
    }
    files.push(("rows of 66,400 bytes".to_owned(), builder.build(), vec![5]));
    // More strings in a chunk than are read in one batch.
    let one = |chunk: usize| -> Vec<String> {
        (0..150_000usize)
            .map(|i| {
                let letter = char::from(b'a' + u8::try_from((i + chunk * 7) % 26).expect("small"));
                letter.to_string().repeat(i % 4)
            })
            .collect()
    };
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .chunks(2)
        .object(Object::channel("g", "s").data(Values::String([one(0), one(1)].concat())));
    files.push(("150,000 strings a chunk".to_owned(), builder.build(), steps));
    // Digital lines of a port one byte wide, in more rows than one block.
    let mut builder = FileBuilder::new();
    let segment = builder.segment().daqmx_widths(&[1]);
    for bit in [0u32, 1, 6, 7] {
        segment.object(Object::channel("port", &format!("line{bit}")).daqmx(
            false,
            true,
            vec![ScalerSpec {
                values: Values::U8(common::bit_pattern(u64::from(bit), 1_200_000)),
                buffer: 0,
                offset: bit,
                scale_id: 0,
            }],
        ));
    }
    files.push((
        "1,200,000 rows of digital lines".to_owned(),
        builder.build(),
        vec![1_048_577],
    ));
    files
}

#[test]
fn large_reads_cross_every_internal_boundary() {
    let files = large_files();
    for threads in [4, 1] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            for (what, built, steps) in &files {
                check_reads(&format!("{what}, {threads} threads"), built, steps);
            }
        });
    }
}

/// Counts the positional reads made of a file held in memory.
struct Counting {
    bytes: Vec<u8>,
    reads: std::sync::atomic::AtomicUsize,
}

impl ReadAt for Counting {
    fn size(&self) -> u64 {
        self.bytes.size()
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.bytes.read_exact_at(offset, buf)
    }
}

#[test]
fn a_channel_alone_in_its_chunks_is_one_read_per_segment() {
    use std::sync::atomic::Ordering;
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .chunks(5)
        .object(Object::channel("g", "a").generated(DataType::F64, 100));
    builder.segment().no_metadata().chunks(3);
    builder.segment().no_metadata().chunks(3);
    let built = builder.build();
    let source = std::sync::Arc::new(Counting {
        bytes: built.bytes.clone(),
        reads: std::sync::atomic::AtomicUsize::new(0),
    });
    let file = buzzard_tdms::OpenOptions::new()
        .open_source(std::sync::Arc::<Counting>::clone(&source), None)
        .unwrap();
    let channel = file.channel("g", "a").unwrap();
    source.reads.store(0, Ordering::Relaxed);
    let values = channel.read_all::<f64>().unwrap();
    assert_eq!(
        Values::F64(values),
        *built.channels[0].values.as_ref().unwrap()
    );
    // The chunks of a segment follow one another without a gap, so each of
    // the three segments takes one read.
    assert_eq!(source.reads.load(Ordering::Relaxed), 3);
    source.reads.store(0, Ordering::Relaxed);
    channel.read::<f64>(150..450).unwrap();
    assert_eq!(source.reads.load(Ordering::Relaxed), 1);
}

/// A file of `segments` segments that each hold four values of `/'g'/'a'`.
/// With `metadata`, every segment declares the channel again.
fn run_of_segments(segments: usize, metadata: bool) -> Built {
    let mut builder = FileBuilder::new();
    for number in 0..segments {
        let segment = builder.segment();
        if number == 0 || metadata {
            segment.object(Object::channel("g", "a").generated(DataType::I32, 4));
        } else {
            segment.no_metadata();
        }
    }
    builder.build()
}

/// Where each segment starts in an index file.
fn index_offsets(index: &[u8]) -> Vec<usize> {
    let mut offsets = Vec::new();
    let mut at = 0;
    while at < index.len() {
        offsets.push(at);
        let raw = u64::from_le_bytes(index[at + 20..at + 28].try_into().expect("eight bytes"));
        at += 28 + usize::try_from(raw).expect("small");
    }
    offsets
}

#[test]
fn an_index_file_with_a_different_lead_in_is_rejected() {
    let built = run_of_segments(5, true);
    let offsets = index_offsets(&built.index_bytes);
    assert_eq!(offsets.len(), 5);
    let file = open(&built.bytes, Some(&built.index_bytes)).unwrap();
    assert_eq!(file.index_source(), IndexSource::IndexFile);
    // The first, a middle, and the last segment. In each, one bit changes in
    // the table of contents (a bit that means nothing) and one in the
    // version number.
    for segment in [0, 2, 4] {
        for field in [4, 8] {
            let mut index = built.index_bytes.clone();
            index[offsets[segment] + field] ^= 1;
            let what = format!("segment {segment}, byte {field} of the lead-in");
            let file = open(&built.bytes, Some(&index)).unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(
                file.index_source(),
                IndexSource::DataFileIndexRejected,
                "{what}"
            );
            assert_eq!(file.anomalies(), [], "{what}");
            assert_matches(&built, &file, &what);
        }
    }
    // The index file is intact and a segment of the data file has lost its
    // tag. The reader must still refuse the data file.
    for segment in [0, 2, 4] {
        let mut data = built.bytes.clone();
        let at = usize::try_from(built.segment_offsets[segment]).unwrap();
        data[at..at + 4].copy_from_slice(b"TDSh");
        assert!(
            open(&data, Some(&built.index_bytes)).is_err(),
            "segment {segment} without its tag"
        );
    }
}

#[test]
fn an_index_file_of_a_long_file_is_checked_in_evenly_spaced_segments() {
    // The file has 150 segments, more than are compared one by one. Segment
    // 76 is one of those compared, segment 149 is the last, and segment 77
    // is neither.
    for metadata in [false, true] {
        let built = run_of_segments(150, metadata);
        let file = open(&built.bytes, Some(&built.index_bytes)).unwrap();
        assert_eq!(file.index_source(), IndexSource::IndexFile);
        assert_matches(&built, &file, "150 segments through the index file");
        for (segment, believed) in [(0, false), (76, false), (149, false), (77, true)] {
            // The data file has version 4712 where the index file has 4713.
            // Both numbers are valid.
            let mut data = built.bytes.clone();
            let at = usize::try_from(built.segment_offsets[segment]).unwrap() + 8;
            assert_eq!(data[at..at + 4], 4713u32.to_le_bytes());
            data[at..at + 4].copy_from_slice(&4712u32.to_le_bytes());
            let file = open(&data, Some(&built.index_bytes)).unwrap();
            let expected = if believed {
                IndexSource::IndexFile
            } else {
                IndexSource::DataFileIndexRejected
            };
            assert_eq!(file.index_source(), expected, "segment {segment}");
            assert_matches(&built, &file, "a version number that differs");
        }
    }
}

#[test]
fn an_index_file_with_other_metadata_is_not_believed() {
    // The index file has the same lead-ins and the same length, and one
    // segment whose metadata names another channel.
    let renamed = |built: &Built, segment: usize| {
        let mut index = built.index_bytes.clone();
        let at = common::after_nth(&index, b"/'g'/'a'", segment);
        index[at - 2] = b'b';
        index
    };
    let built = run_of_segments(5, true);
    for segment in 0..5 {
        let index = renamed(&built, segment);
        let file = open(&built.bytes, Some(&index)).unwrap();
        assert_eq!(
            file.index_source(),
            IndexSource::DataFileIndexRejected,
            "segment {segment}"
        );
        assert_eq!(file.anomalies(), [], "segment {segment}");
        assert_matches(&built, &file, "an index file with other metadata");
    }
    // The limit of the comparison. Of a long file only some segments are
    // compared, and an index file that differs in another segment is
    // believed. This test pins that limit, which the README states.
    let built = run_of_segments(150, true);
    for segment in [0, 76, 149] {
        let file = open(&built.bytes, Some(&renamed(&built, segment))).unwrap();
        assert_eq!(
            file.index_source(),
            IndexSource::DataFileIndexRejected,
            "segment {segment}"
        );
        assert_matches(&built, &file, "an index file with other metadata");
    }
    let file = open(&built.bytes, Some(&renamed(&built, 77))).unwrap();
    assert_eq!(file.index_source(), IndexSource::IndexFile);
    assert!(file.channel("g", "b").is_some());
}

#[test]
fn every_scaler_type_reads_back() {
    for big_endian in [false, true] {
        for typed in [false, true] {
            let built = common::every_scaler_scenario(big_endian, typed);
            let what = format!("big-endian {big_endian}, typed {typed}");
            let file = open(&built.bytes, None).unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(file.anomalies(), [], "{what}");
            assert_matches(&built, &file, &what);
            for dtype in common::SCALER_TYPES {
                let channel = file.channel("types", dtype.name()).unwrap();
                assert_eq!(channel.len(), 21, "{what}");
                let scalers = channel.scalers();
                assert_eq!(scalers.len(), 1, "{what}");
                assert_eq!(scalers[0].dtype, dtype, "{what}");
                assert!(!scalers[0].digital_line, "{what}");
                let stored = if typed { dtype } else { DataType::DaqmxRaw };
                assert_eq!(channel.dtype(), stored, "{what}");
                assert_eq!(channel.read_column(0..21).unwrap().dtype(), dtype);
            }
        }
        // A scaler of timestamps, which no other reader is known to read. A
        // value is sixteen bytes, laid out as a timestamp channel's values are.
        let mut builder = FileBuilder::new();
        builder
            .segment()
            .big_endian(big_endian)
            .chunks(2)
            .daqmx_widths(&[20])
            .object(Object::channel("g", "when").daqmx(
                true,
                false,
                vec![ScalerSpec {
                    values: buzzard_tdms::build::generate(DataType::Timestamp, 3, 0, 6),
                    buffer: 0,
                    offset: 3,
                    scale_id: 0,
                }],
            ));
        let built = builder.build();
        let file = open(&built.bytes, None).unwrap();
        assert_matches(&built, &file, "a scaler of timestamps");
        let when = file.channel("g", "when").unwrap();
        assert_eq!(when.scalers()[0].dtype, DataType::Timestamp);
    }
}

#[test]
#[expect(
    clippy::cast_precision_loss,
    reason = "the conversion under test loses the same bits"
)]
#[expect(
    clippy::too_many_lines,
    reason = "one list of cases, read top to bottom"
)]
fn read_f64_converts_every_numeric_type() {
    let cases: Vec<(Values, Vec<f64>, bool)> = vec![
        (
            Values::I8(vec![i8::MIN, -1, 0, i8::MAX]),
            vec![-128.0, -1.0, 0.0, 127.0],
            false,
        ),
        (
            Values::I16(vec![i16::MIN, -1, 0, i16::MAX]),
            vec![-32768.0, -1.0, 0.0, 32767.0],
            false,
        ),
        (
            Values::I32(vec![i32::MIN, -1, 0, i32::MAX]),
            vec![-2_147_483_648.0, -1.0, 0.0, 2_147_483_647.0],
            false,
        ),
        (
            Values::I64(vec![i64::MIN, -1, 0, i64::MAX]),
            vec![i64::MIN as f64, -1.0, 0.0, i64::MAX as f64],
            false,
        ),
        (
            Values::U8(vec![0, 1, 0x80, u8::MAX]),
            vec![0.0, 1.0, 128.0, 255.0],
            false,
        ),
        (
            Values::U16(vec![0, 1, 0x8000, u16::MAX]),
            vec![0.0, 1.0, 32768.0, 65535.0],
            false,
        ),
        (
            Values::U32(vec![0, 1, 0x8000_0000, u32::MAX]),
            vec![0.0, 1.0, 2_147_483_648.0, 4_294_967_295.0],
            false,
        ),
        (
            Values::U64(vec![0, 1, 1 << 63, u64::MAX]),
            vec![0.0, 1.0, (1u64 << 63) as f64, u64::MAX as f64],
            false,
        ),
        (
            Values::F32(vec![f32::MIN, -1.5, 0.0, f32::MAX]),
            vec![f64::from(f32::MIN), -1.5, 0.0, f64::from(f32::MAX)],
            false,
        ),
        (
            Values::F64(vec![f64::MIN, -1.5, 0.0, f64::MAX]),
            vec![f64::MIN, -1.5, 0.0, f64::MAX],
            false,
        ),
        (
            Values::F32(vec![-2.5, 0.25, 3.0, 1e30]),
            vec![-2.5, 0.25, 3.0, f64::from(1e30_f32)],
            true,
        ),
        (
            Values::F64(vec![-2.5, 0.25, 3.0, 1e300]),
            vec![-2.5, 0.25, 3.0, 1e300],
            true,
        ),
        // A boolean is true when its byte is not zero.
        (
            Values::Bool(vec![0, 1, 2, 255]),
            vec![0.0, 1.0, 1.0, 1.0],
            false,
        ),
    ];
    for big_endian in [false, true] {
        for interleaved in [false, true] {
            let mut builder = FileBuilder::new();
            let segment = builder.segment();
            segment.big_endian(big_endian).interleaved(interleaved);
            for (number, (values, _, with_unit)) in cases.iter().enumerate() {
                let object = Object::channel("g", &format!("c{number}")).data(values.clone());
                segment.object(if *with_unit {
                    object.with_unit()
                } else {
                    object
                });
            }
            let file = open(&builder.build().bytes, None).unwrap();
            for (number, (values, expected, _)) in cases.iter().enumerate() {
                let what = format!(
                    "{} (big-endian {big_endian}, interleaved {interleaved})",
                    values.dtype()
                );
                let channel = file.channel("g", &format!("c{number}")).unwrap();
                assert_eq!(&channel.read_f64(0..4).unwrap(), expected, "{what}");
                assert_eq!(channel.read_f64(1..3).unwrap(), expected[1..3], "{what}");
                // Without scaling properties the scaled values are the same.
                assert_eq!(&channel.read_scaled(0..4).unwrap(), expected, "{what}");
            }
        }
    }
    // Types that are not numbers.
    let mut builder = FileBuilder::new();
    let segment = builder.segment();
    segment.object(Object::channel("g", "text").data(Values::String(vec!["a".into()])));
    for dtype in [
        DataType::Timestamp,
        DataType::ComplexF32,
        DataType::ComplexF64,
    ] {
        segment.object(Object::channel("g", dtype.name()).generated(dtype, 1));
    }
    let file = open(&builder.build().bytes, None).unwrap();
    for channel in file.channels() {
        assert!(
            matches!(
                channel.read_f64(0..1),
                Err(buzzard_tdms::Error::TypeMismatch { .. })
            ),
            "{}",
            channel.path()
        );
    }
}

#[test]
fn features_outside_the_cross_product_read_back() {
    let cases = common::feature_cases();
    assert_eq!(cases.len(), 17);
    for (name, built) in cases {
        let file = open(&built.bytes, None).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(file.anomalies(), [], "{name}");
        assert_matches(&built, &file, name);
        let indexed = open(&built.bytes, Some(&built.index_bytes)).unwrap();
        assert_eq!(indexed.index_source(), IndexSource::IndexFile, "{name}");
        assert_matches(&built, &indexed, name);
    }
}

#[test]
fn a_name_property_does_not_rename_the_object() {
    let (_, built) = common::feature_cases()
        .into_iter()
        .find(|(name, _)| *name == "name_property")
        .unwrap();
    let file = open(&built.bytes, None).unwrap();
    assert_eq!(file.channels().count(), 1);
    let channel = file.channel("g", "a").unwrap();
    assert_eq!(channel.path(), "/'g'/'a'");
    assert_eq!(channel.name(), "a");
    assert_eq!(
        channel.property("name"),
        Some(&PropertyValue::String("renamed".into()))
    );
    assert!(file.channel("g", "renamed").is_none());
}

#[test]
fn a_property_written_again_with_another_type() {
    let (_, built) = common::feature_cases()
        .into_iter()
        .find(|(name, _)| *name == "property_changes_its_type")
        .unwrap();
    for index in [None, Some(built.index_bytes.as_slice())] {
        let file = open(&built.bytes, index).unwrap();
        let properties = file.channel("g", "a").unwrap().properties();
        // The property keeps its place and takes the new type and value.
        assert_eq!(properties.len(), 2);
        assert_eq!(properties[0].name, "p");
        assert_eq!(properties[0].dtype, DataType::F32);
        assert_eq!(properties[0].value, PropertyValue::F32(5.5));
        assert_eq!(properties[1].dtype, DataType::U8);
        assert_eq!(file.properties()[0].dtype, DataType::U8);
        assert_eq!(file.properties()[0].value, PropertyValue::U8(2));
    }
}

#[test]
fn versions_lists_each_number_once_in_order() {
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .version(4712)
        .object(Object::channel("g", "a").generated(DataType::I32, 2));
    for version in [4713, 4712, 9999, 4713, 9999] {
        builder.segment().version(version).no_metadata();
    }
    let built = builder.build();
    for index in [None, Some(built.index_bytes.as_slice())] {
        let file = open(&built.bytes, index).unwrap();
        assert_eq!(file.versions(), [4712, 4713, 9999]);
        // An unknown number is reported where it first appears.
        assert_eq!(
            file.anomalies(),
            [Anomaly::UnknownVersion {
                segment: 3,
                version: 9999
            }]
        );
        assert_matches(&built, &file, "mixed versions");
    }
}

#[test]
fn a_void_channel_with_no_values_is_accepted() {
    // Beside a channel with values, so that the segment has raw data, and
    // alone in a segment without raw data.
    for alone in [false, true] {
        let mut builder = FileBuilder::new();
        let segment = builder.segment();
        segment.object(Object::channel("g", "a").generated(DataType::I32, 0));
        if !alone {
            segment.object(Object::channel("g", "b").generated(DataType::I16, 3));
        }
        builder.segment().no_metadata();
        let mut built = builder.build();
        let raw_flag = built.bytes[4] & (1 << 3) != 0;
        assert_eq!(raw_flag, !alone);
        for bytes in [&mut built.bytes, &mut built.index_bytes] {
            // The type code follows the path and the index length.
            let code = common::after_nth(bytes, b"/'g'/'a'", 0) + 4;
            assert_eq!(bytes[code..code + 4], DataType::I32.code().to_le_bytes());
            bytes[code..code + 4].copy_from_slice(&DataType::Void.code().to_le_bytes());
        }
        for index in [None, Some(built.index_bytes.as_slice())] {
            let file = open(&built.bytes, index).unwrap_or_else(|e| panic!("alone {alone}: {e}"));
            assert_eq!(file.anomalies(), []);
            let a = file.channel("g", "a").unwrap();
            assert_eq!((a.dtype(), a.len()), (DataType::Void, 0));
            if !alone {
                let b = file.channel("g", "b").unwrap();
                assert_eq!(
                    b.read_column(0..6).unwrap(),
                    *built.channels[1].values.as_ref().unwrap()
                );
            }
        }
    }
}

#[test]
fn metadata_is_read_only_when_the_table_of_contents_says_so() {
    // The second segment has the bytes of metadata and a raw data offset
    // that skips them, but its table of contents lacks the metadata flag.
    // The reader does not read those bytes as metadata.
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::channel("g", "a").generated(DataType::I32, 4));
    builder.segment().toc_xor(1 << 1).object(
        Object::channel("g", "a")
            .same()
            .prop("ignored", PropertyValue::I32(1)),
    );
    let built = builder.build();
    let toc = usize::try_from(built.segment_offsets[1]).unwrap() + 4;
    assert_eq!(built.bytes[toc] & 2, 0);
    for index in [None, Some(built.index_bytes.as_slice())] {
        let file = open(&built.bytes, index).unwrap();
        assert_eq!(file.anomalies(), []);
        let a = file.channel("g", "a").unwrap();
        assert_eq!(a.properties(), []);
        assert_eq!(
            a.read_column(0..a.len()).unwrap(),
            *built.channels[0].values.as_ref().unwrap()
        );
    }
}

#[test]
fn a_new_object_list_without_data_objects_ends_the_old_layout() {
    // The sixteen extra bytes would be one whole chunk of the first segment's
    // two channels if its layout still applied.
    for objects in [true, false] {
        let mut builder = FileBuilder::new();
        builder
            .segment()
            .object(Object::channel("g", "a").generated(DataType::I32, 2))
            .object(Object::channel("g", "b").generated(DataType::F64, 1));
        let segment = builder.segment();
        segment.new_object_list(true).extra_raw(&[7; 16]);
        if objects {
            segment.object(Object::group("g"));
        }
        builder.segment().no_metadata().extra_raw(&[9; 16]);
        let built = builder.build();
        for index in [None, Some(built.index_bytes.as_slice())] {
            let file = open(&built.bytes, index).unwrap();
            let unindexed = |segment: usize| Anomaly::UnindexedData {
                segment: segment as u64,
                offset: built.data_offsets[segment],
                bytes: 16,
            };
            assert_eq!(file.anomalies(), [unindexed(1), unindexed(2)]);
            assert_eq!(file.channel("g", "a").unwrap().len(), 2);
            assert_eq!(file.channel("g", "b").unwrap().len(), 1);
            assert_matches(&built, &file, "a new list without data objects");
        }
    }
}

/// Removes the file when the test ends, however it ends.
struct Removed(std::path::PathBuf);

impl Drop for Removed {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
#[ignore = "writes a sparse file of 5 GiB under target/; run with -- --ignored"]
fn offsets_beyond_four_gibibytes_are_read() {
    use std::io::{Seek, SeekFrom, Write};
    const VALUES: u64 = (5 << 30) / 8;
    // The lead-in and metadata of a segment of one channel, with the count
    // and the segment's length changed to span 5 GiB of raw data.
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::channel("g", "big").generated(DataType::F64, 4));
    let small = builder.build();
    let data_start = small.data_offsets[0];
    let mut head = small.bytes[..usize::try_from(data_start).unwrap()].to_vec();
    let count = common::after_nth(&head, b"/'g'/'big'", 0) + 12;
    head[count..count + 8].copy_from_slice(&VALUES.to_le_bytes());
    let next = data_start - 28 + VALUES * 8;
    head[12..20].copy_from_slice(&next.to_le_bytes());
    // A second segment after it, with a new object list of one channel.
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::channel("g", "after").data(Values::I32(vec![11, 22, 33])));
    let second = builder.build().bytes;

    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("five-gibibytes.tdms");
    let _removed = Removed(path.clone());
    let mut out = std::fs::File::create(&path).unwrap();
    out.write_all(&head).unwrap();
    // Three values just below 4 GiB, three across it, and the last three.
    let marks: [(u64, [f64; 3]); 3] = [
        ((1 << 29) - 10, [1.5, 2.5, 3.5]),
        ((1 << 29) - 1, [-1.0, -2.0, -3.0]),
        (VALUES - 3, [7.0, 8.0, 9.0]),
    ];
    for (first, values) in marks {
        out.seek(SeekFrom::Start(data_start + first * 8)).unwrap();
        for value in values {
            out.write_all(&value.to_le_bytes()).unwrap();
        }
    }
    assert_eq!(out.stream_position().unwrap(), data_start + VALUES * 8);
    out.write_all(&second).unwrap();
    drop(out);

    let file = TdmsFile::open(&path).unwrap();
    assert_eq!(file.anomalies(), []);
    assert_eq!(file.segment_count(), 2);
    assert!(file.size() > 5 << 30);
    let big = file.channel("g", "big").unwrap();
    assert_eq!(big.len(), VALUES);
    for (first, values) in marks {
        assert_eq!(big.read::<f64>(first..first + 3).unwrap(), values);
    }
    assert_eq!(big.read::<f64>(0..2).unwrap(), [0.0, 0.0]);
    assert!(big.read::<f64>(VALUES - 1..VALUES + 1).is_err());
    let after = file.channel("g", "after").unwrap();
    assert_eq!(after.read_all::<i32>().unwrap(), [11, 22, 33]);
}
