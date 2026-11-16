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

#[test]
fn a_typed_driver_channel_needs_one_scaler_of_its_type() {
    let scaler = |offset, scale_id| ScalerSpec {
        values: Values::I16(vec![1, 2, 3]),
        buffer: 0,
        offset,
        scale_id,
    };
    let typed = |scalers| {
        let mut builder = FileBuilder::new();
        builder
            .segment()
            .daqmx_widths(&[4])
            .object(Object::channel("g", "ai").daqmx(false, false, scalers));
        builder.build().bytes
    };
    let good = typed(vec![scaler(0, 0)]);
    assert!(exercise(&good, None).is_none());
    // Two scalers, and one scaler of another type than the channel's.
    corrupt(
        &typed(vec![scaler(0, 0), scaler(2, 1)]),
        "exactly one scaler of its type",
    );
    let index = after(&good, b"/'g'/'ai'");
    assert_eq!(
        good[index + 4..index + 8],
        DataType::I16.code().to_le_bytes()
    );
    for other in [DataType::I32, DataType::U16, DataType::F64] {
        corrupt(
            &with(&good, index + 4, &other.code().to_le_bytes()),
            "exactly one scaler of its type",
        );
    }
    // The scaler's own type code: 3 is a 16-bit integer, 2 is unsigned.
    assert_eq!(good[index + 24..index + 28], 3u32.to_le_bytes());
    corrupt(
        &with(&good, index + 24, &2u32.to_le_bytes()),
        "exactly one scaler of its type",
    );
}

#[test]
fn a_segment_of_driver_buffers_and_plain_data_is_an_error() {
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .daqmx_widths(&[4])
        .no_raw()
        .extra_raw(&[0; 12])
        .object(Object::channel("g", "ai").daqmx(
            true,
            false,
            vec![ScalerSpec {
                values: Values::I16(vec![1, 2, 3]),
                buffer: 0,
                offset: 0,
                scale_id: 0,
            }],
        ))
        .object(Object::channel("g", "plain").generated(DataType::I32, 1));
    corrupt(&builder.build().bytes, "mixes DAQmx and other raw data");
}

#[test]
fn a_driver_channel_that_changes_its_scalers_is_an_error() {
    // One scaler more, a scaler of another type, a scaler with another
    // number: no single read could cover such a channel.
    for how in ["more", "type", "id"] {
        let built = common::scalers_change(how);
        corrupt(&built.bytes, "changes its scalers");
        // The index file says the same, so it is not believed either, and
        // the data file is refused.
        assert!(matches!(
            exercise(&built.bytes, Some(&built.index_bytes)),
            Some(Error::Corrupt { .. })
        ));
    }
    // Where the scalers sit may change, and the order they are listed in.
    let scaler = |seed: i16, offset, scale_id| ScalerSpec {
        values: Values::I16((0..4).map(|i| seed * 10 + i).collect()),
        buffer: 0,
        offset,
        scale_id,
    };
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .daqmx_widths(&[8])
        .object(Object::channel("g", "a").daqmx(
            true,
            false,
            vec![scaler(1, 0, 0), scaler(2, 2, 1)],
        ));
    builder
        .segment()
        .daqmx_widths(&[11])
        .object(Object::channel("g", "a").daqmx(
            true,
            false,
            vec![scaler(3, 5, 1), scaler(4, 9, 0)],
        ));
    builder.segment().daqmx_widths(&[11]).no_metadata();
    let built = builder.build();
    let file = open(&built.bytes, None).unwrap();
    assert_eq!(file.anomalies(), []);
    let a = file.channel("g", "a").unwrap();
    assert_eq!(a.len(), 12);
    assert_eq!(a.scalers().len(), 2);
    assert_eq!(
        a.read_scaler::<i16>(0, 0..8).unwrap(),
        [10, 11, 12, 13, 40, 41, 42, 43]
    );
    assert_eq!(
        a.read_scaler::<i16>(1, 0..8).unwrap(),
        [20, 21, 22, 23, 30, 31, 32, 33]
    );
    assert_matches(&built, &file, "scalers that move");
}

#[test]
fn a_cut_chunk_of_several_driver_buffers_gives_the_whole_rows_of_each() {
    // Two buffers, of 4 and of 6 bytes a row; a chunk is four rows of the
    // first and then four rows of the second: 40 bytes.
    let built = common::two_driver_channels(&[4, 6], &|b| {
        b.segment()
            .daqmx_widths(&[4, 6])
            .no_metadata()
            .chunks(2)
            .unfinished();
    });
    let data = usize::try_from(built.data_offsets[1]).unwrap();
    assert_eq!(built.bytes.len() - data, 80);
    for cut in data..=built.bytes.len() {
        let present = (cut - data) as u64;
        let (chunks, rest) = (present / 40, present % 40);
        let file = open(&built.bytes[..cut], None).unwrap();
        let a = file.channel("g", "a").unwrap();
        let b = file.channel("g", "b").unwrap();
        let rows_a = (rest / 4).min(4);
        let rows_b = rest.saturating_sub(16) / 6;
        assert_eq!(a.len(), 4 + chunks * 4 + rows_a, "cut at {cut}");
        assert_eq!(b.len(), 4 + chunks * 4 + rows_b, "cut at {cut}");
        for (channel, expected) in [(a, &built.channels[0]), (b, &built.channels[1])] {
            let found = channel.read_column(0..channel.len()).unwrap();
            let all = expected.values.as_ref().unwrap();
            assert_eq!(found, prefix(all, found.len()), "cut at {cut}");
        }
        let unfinished = Anomaly::UnfinishedSegment {
            segment: 1,
            offset: built.segment_offsets[1],
        };
        if rest == 0 {
            assert_eq!(file.anomalies(), [unfinished], "cut at {cut}");
        } else {
            let partial = Anomaly::PartialChunk {
                segment: 1,
                offset: built.data_offsets[1] + chunks * 40,
                bytes: rest,
                recovered: rows_a > 0,
            };
            assert_eq!(file.anomalies(), [unfinished, partial], "cut at {cut}");
        }
    }
    // A channel with a scaler in each buffer has the rows that both hold.
    let built = daqmx_scenario(false, 2);
    let last = usize::try_from(built.data_offsets[1]).unwrap();
    let mut lengths = std::collections::BTreeSet::new();
    for cut in last..=built.bytes.len() {
        let file = open(&built.bytes[..cut], None).unwrap();
        let ai1 = file.channel("Measured", "ai1").unwrap();
        let wide = file.channel("Measured", "wide").unwrap();
        let ai0 = file.channel("Measured", "ai0").unwrap();
        // `wide` is in the second buffer alone, `ai0` in the first.
        assert_eq!(ai1.len(), wide.len(), "cut at {cut}");
        assert!(ai0.len() >= ai1.len(), "cut at {cut}");
        lengths.insert((ai0.len(), ai1.len()));
        let expected = built.channels.iter().find(|c| c.name == "ai1").unwrap();
        for (id, all) in &expected.scalers {
            let found = ai1.read_scaler_column(*id, 0..ai1.len()).unwrap();
            assert_eq!(found, prefix(all, found.len()), "cut at {cut}");
        }
    }
    // Every count of rows occurred, in the first buffer and in both.
    assert!(lengths.contains(&(13, 10)));
    assert!(lengths.contains(&(15, 13)));
    assert!(lengths.contains(&(20, 20)));
}

#[test]
fn odd_driver_and_object_lists_have_a_decided_reading() {
    // A driver channel declared without values beside one with values.
    let mut built = common::two_driver_channels(&[6], &|b| {
        b.segment().daqmx_widths(&[6]).no_metadata();
    });
    let count = common::after_nth(&built.bytes, b"/'g'/'b'", 0) + 12;
    built.bytes[count..count + 8].copy_from_slice(&0u64.to_le_bytes());
    assert!(exercise(&built.bytes, None).is_none());
    let file = open(&built.bytes, None).unwrap();
    assert_eq!(file.anomalies(), []);
    let a = file.channel("g", "a").unwrap();
    assert_eq!(
        a.read_column(0..a.len()).unwrap(),
        *built.channels[0].values.as_ref().unwrap()
    );
    assert_eq!(file.channel("g", "b").unwrap().len(), 0);

    // One object listed twice with two different indexes: the later one
    // replaces the earlier, and accounts for the segment's eight bytes.
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::channel("g", "a").generated(DataType::I32, 4))
        .object(Object::channel("g", "a").generated(DataType::I32, 2));
    let built = builder.build();
    assert_eq!(built.bytes.len() as u64 - built.data_offsets[0], 8);
    let file = open(&built.bytes, None).unwrap();
    assert_eq!(file.anomalies(), []);
    assert_eq!(file.channel("g", "a").unwrap().len(), 2);
    assert_matches(&built, &file, "an object listed twice with two indexes");
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one list of cases, read top to bottom"
)]
fn anomalies_say_exactly_what_is_wrong() {
    // A file cut five bytes short, in a chunk that holds strings: nothing
    // of the chunk can be kept.
    let built = plain();
    let cut = &built.bytes[..built.bytes.len() - 5];
    let file = open(cut, None).unwrap();
    assert_eq!(
        file.anomalies(),
        [
            Anomaly::TruncatedSegment {
                segment: 1,
                offset: built.segment_offsets[1],
                missing: 5,
            },
            Anomaly::PartialChunk {
                segment: 1,
                offset: built.data_offsets[1],
                bytes: 25,
                recovered: false,
            },
        ]
    );
    assert_eq!(file.segment_count(), 2);
    assert_eq!(file.channel("g", "a").unwrap().len(), 4);

    // The same cut in a chunk of fixed-size values: what is there is kept.
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::channel("g", "a").generated(DataType::I32, 4))
        .object(Object::channel("g", "b").generated(DataType::F64, 3));
    builder.segment().no_metadata().chunks(2);
    let fixed = builder.build();
    let file = open(&fixed.bytes[..fixed.bytes.len() - 10], None).unwrap();
    assert_eq!(
        file.anomalies(),
        [
            Anomaly::TruncatedSegment {
                segment: 1,
                offset: fixed.segment_offsets[1],
                missing: 10,
            },
            Anomaly::PartialChunk {
                segment: 1,
                offset: fixed.data_offsets[1] + 40,
                bytes: 30,
                recovered: true,
            },
        ]
    );
    assert_eq!(file.channel("g", "a").unwrap().len(), 12);
    assert_eq!(file.channel("g", "b").unwrap().len(), 7);

    // A finished segment with a tail that is no chunk: no truncation, and
    // the same report of the incomplete chunk.
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::channel("g", "s").data(Values::String(vec!["one".into(), "two".into()])))
        .extra_raw(&[1, 2, 3]);
    let ragged = builder.build();
    assert_eq!(
        open(&ragged.bytes, None).unwrap().anomalies(),
        [Anomaly::PartialChunk {
            segment: 0,
            offset: ragged.data_offsets[0] + 14,
            bytes: 3,
            recovered: false,
        }]
    );

    // Text that is not UTF-8, in the second object of the second segment:
    // the offset is where that object starts.
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::root().prop("title", PropertyValue::String("fine".into())));
    builder
        .segment()
        .object(Object::group("g").prop("title", PropertyValue::String("fine".into())))
        .object(
            Object::channel("g", "t\u{7f}")
                .generated(DataType::U8, 1)
                .prop("after", PropertyValue::I32(1)),
        );
    let mut text = builder.build();
    let path = after(&text.bytes, b"/'g'/'t");
    text.bytes[path] = 0xFF;
    let object = (path - "/'g'/'t".len() - 4) as u64;
    assert_eq!(
        text.bytes[path - 11..path - 7],
        9u32.to_le_bytes(),
        "the length of the path is where the object starts"
    );
    assert_eq!(
        open(&text.bytes, None).unwrap().anomalies(),
        [Anomaly::InvalidUtf8 {
            segment: 1,
            offset: object,
        }]
    );

    // Raw bytes in a segment whose only channel is declared without
    // values.
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .object(Object::channel("g", "a").generated(DataType::F64, 0))
        .extra_raw(&[1, 2, 3, 4, 5]);
    let empty = builder.build();
    let file = open(&empty.bytes, None).unwrap();
    assert_eq!(
        file.anomalies(),
        [Anomaly::UnindexedData {
            segment: 0,
            offset: empty.data_offsets[0],
            bytes: 5,
        }]
    );
    assert_eq!(file.channel("g", "a").unwrap().len(), 0);

    // A file cut inside the metadata of its last segment: the segment is
    // counted and ignored.
    let mut builder = FileBuilder::new();
    for _ in 0..2 {
        builder
            .segment()
            .object(Object::channel("g", "a").generated(DataType::I32, 4));
    }
    let two = builder.build();
    let start = two.segment_offsets[1];
    let cut = usize::try_from(start).unwrap() + 28 + 3;
    let whole = two.bytes.len() as u64 - start - 28;
    let file = open(&two.bytes[..cut], None).unwrap();
    assert_eq!(
        file.anomalies(),
        [
            Anomaly::TruncatedSegment {
                segment: 1,
                offset: start,
                missing: whole - 3,
            },
            Anomaly::TruncatedMetadata {
                segment: 1,
                offset: start,
            },
        ]
    );
    assert_eq!(file.segment_count(), 2);
    assert_eq!(file.channel("g", "a").unwrap().len(), 4);
    // A lead-in that is not all there.
    let file = open(&two.bytes[..cut - 10], None).unwrap();
    assert_eq!(
        file.anomalies(),
        [Anomaly::TruncatedLeadIn {
            offset: start,
            bytes: 21,
        }]
    );
    assert_eq!(file.segment_count(), 1);
}

#[test]
fn only_the_first_anomalies_of_a_file_are_kept() {
    // 5,000 segments that each have a version number no segment before
    // them had.
    let mut bytes = Vec::new();
    for number in 0..5_000u32 {
        bytes.extend(b"TDSm");
        bytes.extend(0u32.to_le_bytes());
        bytes.extend((10_000 + number).to_le_bytes());
        bytes.extend([0u8; 16]);
    }
    let file = TdmsFile::from_bytes(bytes).unwrap();
    assert_eq!(file.segment_count(), 5_000);
    assert_eq!(file.anomalies().len(), 1024);
    assert_eq!(
        file.anomalies()[1023],
        Anomaly::UnknownVersion {
            segment: 1023,
            version: 11_023,
        }
    );
    assert_eq!(file.versions().len(), 8);
    assert_eq!(file.versions()[7], 10_007);
}

#[test]
fn walking_a_long_object_list_again_and_again_is_refused() {
    // 2,000 channels, then 600 segments of some forty bytes that switch
    // one channel off and on. Only two layouts result, which the limit
    // below allows; what it does not allow is 600 walks of a list of 2,000
    // objects that the segments' few bytes do not pay for.
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
    let started = Instant::now();
    let outcome = OpenOptions::new()
        .max_layout_entries(5_000)
        .open_source(Arc::new(bytes.clone()), None);
    assert!(started.elapsed() < TIME_LIMIT);
    assert!(matches!(
        outcome,
        Err(Error::IndexTooLarge { limit: 5_000 })
    ));
    // A third of the segments stay within what the file's bytes pay for.
    let mut few = FileBuilder::new();
    let segment = few.segment();
    for i in 0..2_000 {
        segment.object(Object::channel("g", &format!("c{i}")).generated(DataType::U8, 1));
    }
    for i in 0..60 {
        let object = Object::channel("g", "c7");
        few.segment().object(if i % 2 == 0 {
            object.no_data()
        } else {
            object.same()
        });
    }
    let file = OpenOptions::new()
        .max_layout_entries(5_000)
        .open_source(Arc::new(few.build().bytes), None)
        .unwrap();
    assert_eq!(file.index_shape().1, 2);
    assert_eq!(file.channel("g", "c7").unwrap().len(), 31);
}
