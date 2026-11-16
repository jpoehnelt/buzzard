//! Hostile and damaged input: an error or a correct partial result, never
//! a panic, a hang, or more memory than the file accounts for.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use buzzard_tdms::build::{prefix, Built, FileBuilder, Object, ScalerSpec, Values};
use buzzard_tdms::{Anomaly, DataType, Error, OpenOptions, PropertyValue, TdmsFile};
use common::{assert_matches, daqmx_scenario, digital_scenario, open, scenario, Shape};

/// Opens the bytes and, when that works, uses everything: every property,
/// every value of every channel and scaler, scaled values, chunked reads.
/// Returns the error of opening. Panics only if the reader does, or if a
/// channel claims more values than the file has bytes for, which is what
/// keeps every allocation within the size of the file.
fn exercise(bytes: &[u8], index: Option<&[u8]>) -> Option<Error> {
    let file = match open(bytes, index) {
        Ok(file) => file,
        Err(e) => return Some(e),
    };
    let size = bytes.len() as u64;
    assert!(file.anomalies().len() <= 1024);
    let (blocks, _) = file.index_shape();
    assert!(blocks as u64 <= file.segment_count() * 2);
    let _ = (
        file.properties().len(),
        file.versions(),
        file.index_source(),
    );
    for group in file.groups() {
        let _ = (group.name(), group.properties().len());
        for channel in group.channels() {
            let len = channel.len();
            let _ = (channel.unit(), channel.waveform(), channel.scaling());
            let width = match channel.dtype() {
                DataType::String => 4,
                _ => 1,
            };
            assert!(
                len.saturating_mul(width) <= size,
                "{} claims {len} values in a file of {size} bytes",
                channel.path()
            );
            for scaler in channel.scalers() {
                let _ = channel.read_scaler_column(scaler.scale_id, 0..len);
            }
            if let Ok(column) = channel.read_column(0..len) {
                assert_eq!(column.len() as u64, len);
            }
            let _ = channel.read_scaled(0..len);
            let _ = channel.read_column(len / 2..len);
            let _ = channel.read_column(0..len.saturating_add(1));
            assert!(channel.chunks::<f64>(3).take(4).count() <= 4);
        }
    }
    None
}

fn small(shape: Shape) -> Built {
    scenario(shape)
}

fn shape(dtype: DataType, big_endian: bool, interleaved: bool) -> Shape {
    Shape {
        dtype,
        big_endian,
        interleaved,
        chunks: 2,
        version: 4713,
        padding: 0,
    }
}

/// The files that the sweeps damage: every layout and both byte orders.
fn victims() -> Vec<(&'static str, Built)> {
    vec![
        ("contiguous", small(shape(DataType::I32, false, false))),
        ("interleaved", small(shape(DataType::Timestamp, true, true))),
        ("strings", small(shape(DataType::String, false, false))),
        (
            "big-endian strings",
            small(shape(DataType::String, true, false)),
        ),
        ("driver buffers", daqmx_scenario(false, 2)),
        ("digital lines", digital_scenario()),
    ]
}

#[test]
fn a_file_cut_at_any_byte_gives_a_prefix_of_every_channel() {
    for (what, built) in victims() {
        for len in 0..=built.bytes.len() {
            let cut = &built.bytes[..len];
            if exercise(cut, None).is_some() {
                assert!(len < 4, "{what}: a file cut at {len} must open");
                continue;
            }
            let file = open(cut, None).unwrap();
            if len < built.bytes.len() && len > 0 {
                assert!(
                    !file.anomalies().is_empty() || built.segment_offsets.contains(&(len as u64)),
                    "{what}: a cut at {len} went unnoticed"
                );
            }
            for channel in file.channels() {
                let expected = built
                    .channels
                    .iter()
                    .find(|c| c.group == channel.group_name() && c.name == channel.name())
                    .unwrap_or_else(|| panic!("{what}: {} was never written", channel.path()));
                if channel.dtype() == DataType::Void || channel.scalers().len() > 1 {
                    continue;
                }
                let found = channel.read_column(0..channel.len()).unwrap();
                let all = expected.values.as_ref().unwrap();
                assert_eq!(
                    found,
                    prefix(all, found.len()),
                    "{what}: {} cut at {len}",
                    channel.path()
                );
            }
        }
    }
}

#[test]
fn any_single_damaged_byte_is_survived() {
    for (what, built) in victims() {
        assert!(exercise(&built.bytes, None).is_none(), "{what}");
        let mut bytes = built.bytes.clone();
        for at in 0..bytes.len() {
            let original = bytes[at];
            for damaged in [
                original ^ 0x01,
                original ^ 0x80,
                original ^ 0x10,
                0x00,
                0xFF,
            ] {
                bytes[at] = damaged;
                let _ = exercise(&bytes, None);
            }
            bytes[at] = original;
        }
        // The same damage to the index file beside an intact data file.
        let mut index = built.index_bytes.clone();
        for at in 0..index.len() {
            let original = index[at];
            for damaged in [original ^ 0x01, original ^ 0x80, 0x00, 0xFF] {
                index[at] = damaged;
                assert!(
                    exercise(&built.bytes, Some(&index)).is_none(),
                    "{what}: a damaged index file must fall back to the data file"
                );
            }
            index[at] = original;
        }
    }
}

#[test]
fn seeded_random_damage_is_survived() {
    // Words that break size arithmetic when they land in a count, a length
    // or an offset.
    const WORDS: [u64; 12] = [
        0,
        1,
        0x7F,
        0xFF,
        0x7FFF_FFFF,
        0x8000_0000,
        0xFFFF_FFFF,
        0x1_0000_0000,
        0x7FFF_FFFF_FFFF_FFFF,
        0x8000_0000_0000_0000,
        0xFFFF_FFFF_FFFF_FFE4,
        u64::MAX,
    ];
    let mut state = 0x2026_1116_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for (_, built) in victims() {
        let len = built.bytes.len() as u64;
        for round in 0..4_000 {
            let mut bytes = built.bytes.clone();
            for _ in 0..=next() % 3 {
                let at = usize::try_from(next() % len).unwrap();
                if round % 2 == 0 {
                    let word = WORDS[usize::try_from(next() % 12).unwrap()];
                    let word = if next() % 2 == 0 {
                        word.to_le_bytes()
                    } else {
                        word.to_be_bytes()
                    };
                    let width = [4, 8][usize::try_from(next() % 2).unwrap()];
                    let end = bytes.len().min(at + width);
                    bytes[at..end].copy_from_slice(&word[..end - at]);
                } else {
                    bytes[at] = next().to_le_bytes()[0];
                }
            }
            if next() % 8 == 0 {
                bytes.truncate(usize::try_from(next() % len).unwrap());
            }
            let _ = exercise(&bytes, None);
        }
    }
}

/// A file of two channels and two segments whose fields the tests below
/// overwrite.
fn plain() -> Built {
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::root().prop("title", PropertyValue::String("hostile".into())))
        .object(Object::channel("g", "a").generated(DataType::I32, 4))
        .object(Object::channel("g", "s").data(Values::String(vec!["one".into(), "two".into()])));
    builder.segment().no_metadata();
    builder.build()
}

/// The offset just after the first occurrence of `needle`.
fn after(bytes: &[u8], needle: &[u8]) -> usize {
    bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("the bytes are in the file")
        + needle.len()
}

fn with(bytes: &[u8], at: usize, value: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out[at..at + value.len()].copy_from_slice(value);
    out
}

fn corrupt(bytes: &[u8], reason: &str) {
    match exercise(bytes, None) {
        Some(Error::Corrupt { reason: found, .. }) => {
            assert!(
                found.contains(reason),
                "expected {reason:?}, found {found:?}"
            );
        }
        other => panic!("expected a corrupt file ({reason}), found {other:?}"),
    }
}

#[test]
fn absurd_lead_in_offsets_are_errors_or_truncation() {
    let built = plain();
    let bytes = &built.bytes;
    // Next segment offset: zero, smaller than the metadata, one short.
    corrupt(&with(bytes, 12, &0u64.to_le_bytes()), "metadata is longer");
    corrupt(&with(bytes, 12, &10u64.to_le_bytes()), "metadata is longer");
    let next = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    corrupt(&with(bytes, 12, &(next - 1).to_le_bytes()), "segment tag");
    // Raw data offset beyond the segment.
    corrupt(
        &with(bytes, 20, &u64::MAX.to_le_bytes()),
        "metadata is longer",
    );
    corrupt(
        &with(bytes, 20, &(next + 1).to_le_bytes()),
        "metadata is longer",
    );
    // A next segment offset near the top of the range must not wrap: the
    // segment is taken to run to the end of the file.
    for huge in [u64::MAX - 1, u64::MAX - 27, u64::MAX - 28, 1 << 63] {
        let file = open(&with(bytes, 12, &huge.to_le_bytes()), None).unwrap();
        assert!(matches!(
            file.anomalies()[0],
            Anomaly::TruncatedSegment { segment: 0, .. }
        ));
        assert_eq!(file.segment_count(), 1);
    }
    // Not a TDMS file at all, and an index file passed off as one.
    assert!(matches!(exercise(b"", None), Some(Error::NotTdms)));
    assert!(matches!(exercise(b"TDS", None), Some(Error::NotTdms)));
    assert!(matches!(
        exercise(&with(bytes, 0, b"TDSh"), None),
        Some(Error::NotTdms)
    ));
    assert!(matches!(exercise(&[0u8; 64], None), Some(Error::NotTdms)));
    // An unknown version is reported and read.
    let file = open(&with(bytes, 8, &7u32.to_le_bytes()), None).unwrap();
    assert_eq!(
        file.anomalies(),
        [Anomaly::UnknownVersion {
            segment: 0,
            version: 7
        }]
    );
    assert_matches(&built, &file, "unknown version");
    assert!(OpenOptions::new()
        .strict(true)
        .open_source(Arc::new(with(bytes, 8, &7u32.to_le_bytes())), None)
        .is_err());
}

#[test]
fn absurd_counts_and_lengths_are_errors() {
    let built = plain();
    let bytes = &built.bytes;
    let max = u32::MAX.to_le_bytes();
    // The number of objects, a path length, a property count, the length
    // of a property's name and of its value.
    corrupt(&with(bytes, 28, &max), "ends inside a field");
    corrupt(
        &with(bytes, 28, &0x7FFF_FFFFu32.to_le_bytes()),
        "ends inside a field",
    );
    corrupt(&with(bytes, 32, &max), "ends inside a field");
    let root_props = 32 + 4 + 1 + 4;
    corrupt(&with(bytes, root_props, &max), "");
    corrupt(&with(bytes, root_props + 4, &max), "ends inside a field");
    let value = after(bytes, b"title") + 4;
    corrupt(&with(bytes, value, &max), "ends inside a field");
    // The raw data index of /'g'/'a': length, type, dimension, count.
    let index = after(bytes, b"/'g'/'a'");
    corrupt(&with(bytes, index, &21u32.to_le_bytes()), "index length");
    corrupt(&with(bytes, index, &28u32.to_le_bytes()), "index length");
    corrupt(
        &with(bytes, index + 4, &0x55u32.to_le_bytes()),
        "unknown data type",
    );
    corrupt(&with(bytes, index + 4, &max), "DAQmx data type");
    corrupt(&with(bytes, index + 8, &2u32.to_le_bytes()), "dimension");
    corrupt(&with(bytes, index + 8, &0u32.to_le_bytes()), "dimension");
    corrupt(
        &with(bytes, index + 12, &u64::MAX.to_le_bytes()),
        "overflows",
    );
    corrupt(
        &with(bytes, index + 12, &(1u64 << 62).to_le_bytes()),
        "overflows",
    );
    // "Same index as before" for an object that has none.
    corrupt(&with(bytes, index, &0u32.to_le_bytes()), "never had");
    // Types whose layout is not published, and no type at all.
    for code in [11u32, 0x1B, 0x4F, 0] {
        let damaged = with(bytes, index + 4, &code.to_le_bytes());
        assert!(
            matches!(exercise(&damaged, None), Some(Error::Unsupported { .. })),
            "type code {code:#x}"
        );
    }
    // A property of an unknown type, and of a type without a known size.
    let kind = after(bytes, b"title");
    corrupt(
        &with(bytes, kind, &0x55u32.to_le_bytes()),
        "unknown property type",
    );
    assert!(matches!(
        exercise(&with(bytes, kind, &11u32.to_le_bytes()), None),
        Some(Error::Unsupported { .. })
    ));
    // The string channel: fewer bytes than its offset table needs.
    let strings = after(bytes, b"/'g'/'s'");
    corrupt(
        &with(bytes, strings + 20, &7u64.to_le_bytes()),
        "offset table",
    );
    corrupt(
        &with(bytes, strings + 12, &u64::MAX.to_le_bytes()),
        "offset table",
    );
}

#[test]
fn a_count_larger_than_the_data_gives_what_is_there() {
    let built = plain();
    let index = after(&built.bytes, b"/'g'/'a'");
    // 2^40 values declared; each segment holds 16 bytes of them and 14 of
    // the string channel.
    let damaged = with(&built.bytes, index + 12, &(1u64 << 40).to_le_bytes());
    assert!(exercise(&damaged, None).is_none());
    let file = open(&damaged, None).unwrap();
    assert!(file
        .anomalies()
        .iter()
        .all(|a| matches!(a, Anomaly::PartialChunk { .. })));
    assert_eq!(file.anomalies().len(), 2);
    assert!(file.channel("g", "a").unwrap().len() <= 15);
}

#[test]
fn inconsistent_layouts_are_errors() {
    // Interleaved channels of different lengths.
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .interleaved(true)
        .object(Object::channel("g", "a").generated(DataType::I32, 4))
        .object(Object::channel("g", "b").generated(DataType::I16, 4));
    let built = builder.build();
    let index = after(&built.bytes, b"/'g'/'b'");
    corrupt(
        &with(&built.bytes, index + 12, &5u64.to_le_bytes()),
        "interleaved channels differ",
    );
    // A string channel in an interleaved segment.
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .toc_xor(1 << 5)
        .object(Object::channel("g", "a").generated(DataType::I32, 2))
        .object(Object::channel("g", "s").data(Values::String(vec!["x".into(), "y".into()])));
    assert!(matches!(
        exercise(&builder.build().bytes, None),
        Some(Error::Unsupported { .. })
    ));
    // A channel that changes its type.
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::channel("g", "a").generated(DataType::I32, 2));
    builder
        .segment()
        .object(Object::channel("g", "a").generated(DataType::I32, 3));
    let built = builder.build();
    let second = usize::try_from(built.segment_offsets[1]).unwrap();
    let index = second + after(&built.bytes[second..], b"/'g'/'a'");
    corrupt(
        &with(&built.bytes, index + 4, &DataType::F32.code().to_le_bytes()),
        "changes its data type",
    );
    // A malformed object path.
    for path in ["g", "/g", "/'g", "/'g'/'a'/'b'", "/'g'x", ""] {
        let mut builder = FileBuilder::new();
        builder.segment().object(Object::raw_path(path));
        corrupt(&builder.build().bytes, "path is malformed");
    }
}

#[test]
fn the_same_object_twice_in_a_segment_is_read_in_order() {
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::channel("g", "a").generated(DataType::I32, 4))
        .object(Object::channel("g", "b").generated(DataType::F64, 2))
        .object(Object::channel("g", "a").no_data());
    let built = builder.build();
    let file = open(&built.bytes, None).unwrap();
    assert_eq!(file.channel("g", "a").unwrap().len(), 0);
    assert_matches(&built, &file, "an object listed twice");
}

#[test]
fn bad_string_offsets_are_a_read_error() {
    let built = plain();
    let data = usize::try_from(built.data_offsets[0]).unwrap();
    // The raw data: 16 bytes of /'g'/'a', then the offsets 3 and 6.
    let table = data + 16;
    assert_eq!(built.bytes[table..table + 8], [3, 0, 0, 0, 6, 0, 0, 0]);
    for (first, second) in [(5u32, 3u32), (3, 7), (u32::MAX, u32::MAX), (7, 6)] {
        let mut damaged = with(&built.bytes, table, &first.to_le_bytes());
        damaged = with(&damaged, table + 4, &second.to_le_bytes());
        assert!(exercise(&damaged, None).is_none());
        let file = open(&damaged, None).unwrap();
        let strings = file.channel("g", "s").unwrap();
        assert!(matches!(
            strings.read_strings(0..4),
            Err(Error::CorruptData { .. })
        ));
        // The second segment's strings, and the other channel, still read.
        assert_eq!(strings.read_strings(2..4).unwrap().len(), 2);
        assert_eq!(
            file.channel("g", "a")
                .unwrap()
                .read_all::<i32>()
                .unwrap()
                .len(),
            8
        );
    }
}

#[test]
fn text_that_is_not_utf8_is_replaced_and_reported() {
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::channel("g", "t\u{7f}").generated(DataType::U8, 1));
    let mut built = builder.build();
    let at = after(&built.bytes, b"/'g'/'t");
    built.bytes[at] = 0xFF;
    let file = open(&built.bytes, None).unwrap();
    assert!(matches!(
        file.anomalies(),
        [Anomaly::InvalidUtf8 { segment: 0, .. }]
    ));
    let channel = file.channel("g", "t\u{fffd}").unwrap();
    assert_eq!(channel.len(), 1);
}

#[test]
fn bad_scalers_are_errors() {
    let scaler = |offset, buffer| ScalerSpec {
        values: Values::I16(vec![1, 2, 3]),
        buffer,
        offset,
        scale_id: 0,
    };
    let file = |widths: &[u32], offset, buffer, digital| {
        let mut builder = FileBuilder::new();
        builder
            .segment()
            .daqmx_widths(widths)
            .object(Object::channel("g", "ai").daqmx(true, digital, vec![scaler(offset, buffer)]));
        builder.build().bytes
    };
    assert!(exercise(&file(&[4], 2, 0, false), None).is_none());
    let good = file(&[4], 2, 0, false);
    let index = after(&good, b"/'g'/'ai'");
    // After the marker: type, dimension, count (8), scalers (4), then the
    // scaler: type, buffer, offset, format, id; then the widths.
    let (buffer, offset, width) = (index + 28, index + 32, index + 48);
    corrupt(
        &with(&good, offset, &3u32.to_le_bytes()),
        "beyond the width",
    );
    corrupt(
        &with(&good, offset, &u32::MAX.to_le_bytes()),
        "beyond the width",
    );
    corrupt(
        &with(&good, buffer, &1u32.to_le_bytes()),
        "raw buffer that does not exist",
    );
    corrupt(
        &with(&good, buffer, &u32::MAX.to_le_bytes()),
        "raw buffer that does not exist",
    );
    corrupt(&with(&good, width, &0u32.to_le_bytes()), "beyond the width");
    corrupt(&with(&good, width, &1u32.to_le_bytes()), "beyond the width");
    corrupt(
        &with(&good, index + 24, &99u32.to_le_bytes()),
        "unknown scaler data type",
    );
    corrupt(&with(&good, index + 20, &u32::MAX.to_le_bytes()), "");
    corrupt(
        &with(&good, width - 4, &u32::MAX.to_le_bytes()),
        "ends inside a field",
    );
    // A wider row than the data has: nothing can be read of the chunk.
    let wide = with(&good, width, &u32::MAX.to_le_bytes());
    assert!(exercise(&wide, None).is_none());
    assert_eq!(
        open(&wide, None).unwrap().channel("g", "ai").unwrap().len(),
        0
    );
}

#[cfg(unix)]
#[test]
fn a_file_that_shrinks_while_open_is_an_error_not_a_fault() {
    let built = scenario(shape(DataType::F64, false, false));
    let dir = std::env::temp_dir().join(format!("buzzard-tdms-shrink-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("log.tdms");
    std::fs::write(&path, &built.bytes).unwrap();
    let file = TdmsFile::open(&path).unwrap();
    let channel = file.channel("Group one", "a").unwrap();
    assert!(channel.read_all::<f64>().is_ok());
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(200)
        .unwrap();
    assert!(matches!(channel.read_all::<f64>(), Err(Error::Io(_))));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// What a pathological file may cost. The index is built in a single pass
/// whose work is in proportion to the bytes of metadata, so these hold
/// with a wide margin on any machine that runs the test suite at all.
const TIME_LIMIT: Duration = Duration::from_secs(30);

#[test]
fn a_million_empty_segments_cost_no_memory() {
    let mut lead = Vec::new();
    lead.extend(b"TDSm");
    lead.extend(0u32.to_le_bytes());
    lead.extend(4713u32.to_le_bytes());
    lead.extend([0u8; 16]);
    let bytes = lead.repeat(1_000_000);
    let started = Instant::now();
    let file = TdmsFile::from_bytes(bytes).unwrap();
    assert!(started.elapsed() < TIME_LIMIT, "{:?}", started.elapsed());
    assert_eq!(file.segment_count(), 1_000_000);
    // Nothing is kept per segment.
    assert_eq!(file.index_shape(), (0, 0));
    assert_eq!(file.anomalies(), []);
}

#[test]
fn many_tiny_segments_and_many_objects_stay_within_bounds() {
    // 100,000 segments of one byte each for 3 channels: the index keeps one
    // run of segments, not one entry per segment.
    let mut builder = FileBuilder::new();
    let segment = builder.segment();
    for i in 0..3 {
        segment.object(Object::channel("g", &format!("c{i}")).generated(DataType::U8, 1));
    }
    let first = builder.build().bytes;
    let mut bytes = first.clone();
    let mut tail = Vec::new();
    tail.extend(b"TDSm");
    tail.extend((1u32 << 3).to_le_bytes());
    tail.extend(4713u32.to_le_bytes());
    tail.extend(3u64.to_le_bytes());
    tail.extend(0u64.to_le_bytes());
    tail.extend([1, 2, 3]);
    bytes.extend(tail.repeat(100_000));
    let started = Instant::now();
    let file = TdmsFile::from_bytes(bytes).unwrap();
    assert!(started.elapsed() < TIME_LIMIT, "{:?}", started.elapsed());
    assert_eq!(file.segment_count(), 100_001);
    assert_eq!(file.index_shape(), (1, 1));
    let channel = file.channel("g", "c1").unwrap();
    assert_eq!(channel.len(), 100_001);
    let values = channel.read_all::<u8>().unwrap();
    assert!(values[1..].iter().all(|&v| v == 2));

    // 50,000 objects in one segment.
    let mut builder = FileBuilder::new();
    let segment = builder.segment();
    for i in 0..50_000 {
        segment.object(
            Object::channel(&format!("g{}", i % 100), &format!("c{i}"))
                .prop("number", PropertyValue::I32(i)),
        );
    }
    let bytes = builder.build().bytes;
    let started = Instant::now();
    let file = TdmsFile::from_bytes(bytes).unwrap();
    assert!(started.elapsed() < TIME_LIMIT, "{:?}", started.elapsed());
    assert_eq!(file.channels().count(), 50_000);
    assert_eq!(file.groups().len(), 100);
}

#[test]
fn a_file_cannot_make_the_index_grow_without_paying_for_it() {
    // 2,000 channels, then segments of a few bytes that each change the
    // length of one channel: every one would need a new layout of 2,000
    // entries. With the limit lowered, the reader gives up early.
    let mut builder = FileBuilder::new();
    let segment = builder.segment();
    for i in 0..2_000 {
        segment.object(Object::channel("g", &format!("c{i}")).generated(DataType::U8, 1));
    }
    for i in 0..400 {
        builder
            .segment()
            .object(Object::channel("g", "c7").generated(DataType::U8, 2 + i));
    }
    let bytes = builder.build().bytes;
    let options = OpenOptions::new().max_layout_entries(50_000);
    let started = Instant::now();
    let outcome = options.open_source(Arc::new(bytes.clone()), None);
    assert!(started.elapsed() < TIME_LIMIT);
    assert!(matches!(
        outcome,
        Err(Error::IndexTooLarge { limit: 50_000 })
    ));
    // The default limit takes the file: 800,000 entries.
    let file = TdmsFile::from_bytes(bytes).unwrap();
    assert_eq!(file.index_shape().1, 401);

    // Switching a channel off and on needs no new layouts, but walking
    // 2,000 objects for every tiny segment is work the file did not pay
    // for either.
    let mut builder = FileBuilder::new();
    let segment = builder.segment();
    for i in 0..2_000 {
        segment.object(Object::channel("g", &format!("c{i}")).generated(DataType::U8, 1));
    }
    for i in 0..600 {
        let object = Object::channel("g", "c7");
        builder.segment().object(if i % 2 == 0 {
            object.no_data()
        } else {
            object.same()
        });
    }
    let bytes = builder.build().bytes;
    let outcome = OpenOptions::new()
        .max_layout_entries(1_000)
        .open_source(Arc::new(bytes.clone()), None);
    assert!(matches!(outcome, Err(Error::IndexTooLarge { .. })));
    assert_eq!(TdmsFile::from_bytes(bytes).unwrap().index_shape().1, 2);
}
