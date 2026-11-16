//! Property-based tests: random sequences of segments from the generator,
//! whose model says what each channel holds, must read back exactly, and
//! any prefix of such a file must give a prefix of every channel.

mod common;

use buzzard_tdms::build::{prefix, Built, FileBuilder, Object, Values};
use buzzard_tdms::{DataType, PropertyValue};
use common::{assert_matches, open, FIXED};
use proptest::prelude::*;

/// What a segment does to one channel.
#[derive(Clone, Debug)]
enum Op {
    /// A new raw data index for this many values per chunk.
    Index(u64),
    /// The "same as before" marker; a new index when there is none.
    Same,
    /// The "no data" marker.
    Off,
    /// The same as `Same`, with a property changed.
    Property(i32),
}

#[derive(Clone, Debug)]
enum Kind {
    /// The listed channels change; the others carry on.
    Incremental,
    /// A new object list of exactly the listed channels.
    NewList,
    /// A new object list of interleaved channels with this many rows.
    Interleaved(u64),
    /// No metadata at all.
    NoMetadata,
    /// Metadata and no raw data.
    MetadataOnly,
}

#[derive(Clone, Debug)]
struct Plan {
    kind: Kind,
    big_endian: bool,
    chunks: u64,
    padding: usize,
    old_version: bool,
    ops: Vec<(usize, Op)>,
}

const CHANNELS: usize = 8;

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0u64..5).prop_map(Op::Index),
        Just(Op::Same),
        Just(Op::Off),
        any::<i32>().prop_map(Op::Property),
    ]
}

fn plan() -> impl Strategy<Value = Plan> {
    let kind = prop_oneof![
        4 => Just(Kind::Incremental),
        2 => Just(Kind::NewList),
        2 => (1u64..5).prop_map(Kind::Interleaved),
        2 => Just(Kind::NoMetadata),
        1 => Just(Kind::MetadataOnly),
    ];
    (
        kind,
        any::<bool>(),
        1u64..4,
        0usize..4,
        any::<bool>(),
        prop::collection::vec((0..CHANNELS, op()), 0..6),
    )
        .prop_map(
            |(kind, big_endian, chunks, padding, old_version, ops)| Plan {
                kind,
                big_endian,
                chunks,
                padding,
                old_version,
                ops,
            },
        )
}

/// The type of each channel: fixed for the life of a file, as the format
/// requires, and covering every type over the channels.
fn dtype(channel: usize, salt: usize) -> DataType {
    let all: Vec<DataType> = FIXED.into_iter().chain([DataType::String]).collect();
    all[(channel * 2 + salt) % all.len()]
}

fn indexed(object: Object, dtype: DataType, n: u64, chunks: u64, salt: usize) -> Object {
    if dtype == DataType::String {
        // Chunks of a string channel must have the same size in bytes.
        let one: Vec<String> = (0..n)
            .map(|i| "ab€".repeat(usize::try_from(i).expect("small") + salt % 3))
            .collect();
        object.data(Values::String(
            (0..chunks).flat_map(|_| one.clone()).collect(),
        ))
    } else {
        object.generated(dtype, n)
    }
}

fn build(plans: &[Plan], salt: usize) -> Built {
    let mut builder = FileBuilder::new();
    let mut indexed_before = [false; CHANNELS];
    for (number, plan) in plans.iter().enumerate() {
        let segment = builder.segment();
        segment
            .big_endian(plan.big_endian)
            .chunks(plan.chunks)
            .padding(plan.padding)
            .version(if plan.old_version { 4712 } else { 4713 });
        let object =
            |channel: usize| Object::channel(&format!("g{}", channel % 3), &format!("c{channel}"));
        match plan.kind {
            Kind::NoMetadata => {
                segment.no_metadata();
                continue;
            }
            Kind::Interleaved(rows) => {
                segment.interleaved(true).new_object_list(true);
                let mut listed = Vec::new();
                for &(channel, _) in &plan.ops {
                    let dtype = dtype(channel, salt);
                    if dtype == DataType::String || listed.contains(&channel) {
                        continue;
                    }
                    listed.push(channel);
                    indexed_before[channel] = true;
                    segment.object(object(channel).generated(dtype, rows));
                }
                continue;
            }
            Kind::NewList => {
                segment.new_object_list(true);
            }
            Kind::MetadataOnly => {
                segment.no_raw();
            }
            Kind::Incremental => {
                segment.new_object_list(false);
            }
        }
        let mut listed = Vec::new();
        for (channel, op) in &plan.ops {
            if listed.contains(channel) {
                continue;
            }
            listed.push(*channel);
            let dtype = dtype(*channel, salt);
            let base = object(*channel);
            let again = |object: Object| {
                if indexed_before[*channel] {
                    object.same()
                } else {
                    indexed(object, dtype, 2, plan.chunks, salt)
                }
            };
            let object = match op {
                Op::Index(n) => indexed(base, dtype, *n, plan.chunks, salt + number),
                Op::Same => again(base),
                Op::Off => base.no_data(),
                Op::Property(value) => again(base).prop("changed", PropertyValue::I32(*value)),
            };
            if !matches!(op, Op::Off) {
                indexed_before[*channel] = true;
            }
            segment.object(object);
        }
    }
    builder.build()
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 300,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn random_segment_sequences_read_back(
        plans in prop::collection::vec(plan(), 1..12),
        salt in 0usize..15,
    ) {
        let built = build(&plans, salt);
        let file = open(&built.bytes, None).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(file.anomalies(), []);
        prop_assert_eq!(file.segment_count(), plans.len() as u64);
        assert_matches(&built, &file, "random file");
        let indexed = open(&built.bytes, Some(&built.index_bytes))
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(indexed.index_source(), buzzard_tdms::IndexSource::IndexFile);
        assert_matches(&built, &indexed, "random file through its index file");
    }

    #[test]
    fn a_prefix_of_a_file_gives_a_prefix_of_every_channel(
        plans in prop::collection::vec(plan(), 1..8),
        salt in 0usize..15,
        cut in 0.0f64..1.0,
    ) {
        let built = build(&plans, salt);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_precision_loss,
            clippy::cast_sign_loss,
            reason = "a fraction of a small length"
        )]
        let len = (built.bytes.len() as f64 * cut) as usize;
        // Nothing of a file may be an error, a panic, or invented values.
        let Ok(file) = open(&built.bytes[..len], None) else {
            prop_assert!(len < 28, "a file cut at {len} must open");
            return Ok(());
        };
        for channel in file.channels() {
            let expected = built
                .channels
                .iter()
                .find(|c| c.group == channel.group_name() && c.name == channel.name());
            let Some(expected) = expected else {
                return Err(TestCaseError::fail("a channel that was never written"));
            };
            let found = channel
                .read_column(0..channel.len())
                .map_err(|e| TestCaseError::fail(e.to_string()));
            if channel.dtype() == DataType::Void {
                prop_assert_eq!(channel.len(), 0);
                continue;
            }
            let found = found?;
            let all = expected.values.as_ref().expect("a typed channel has values");
            prop_assert!(found.len() <= all.len());
            prop_assert_eq!(&found, &prefix(all, found.len()));
        }
    }
}
