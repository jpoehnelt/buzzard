//! Shared by the integration tests: the files the generator writes for the
//! conformance cross product, and the comparison of a reader's view with
//! the generator's model.

#![allow(
    dead_code,
    reason = "each test crate uses a different subset of these helpers"
)]

pub(crate) mod sha256;

use buzzard_tdms::build::{Built, FileBuilder, Object, ScalerSpec, Values};
use buzzard_tdms::{Complex32, Complex64, DataType, PropertyValue, TdmsFile, Timestamp};

/// Every fixed-size type a channel can have.
pub(crate) const FIXED: [DataType; 14] = [
    DataType::I8,
    DataType::I16,
    DataType::I32,
    DataType::I64,
    DataType::U8,
    DataType::U16,
    DataType::U32,
    DataType::U64,
    DataType::F32,
    DataType::F64,
    DataType::Bool,
    DataType::Timestamp,
    DataType::ComplexF32,
    DataType::ComplexF64,
];

/// One value of every kind a property can have.
pub(crate) fn every_property() -> Vec<(&'static str, PropertyValue)> {
    vec![
        ("p_i8", PropertyValue::I8(-8)),
        ("p_i16", PropertyValue::I16(-1600)),
        ("p_i32", PropertyValue::I32(-320_000)),
        ("p_i64", PropertyValue::I64(-64_000_000_000)),
        ("p_u8", PropertyValue::U8(200)),
        ("p_u16", PropertyValue::U16(60_000)),
        ("p_u32", PropertyValue::U32(4_000_000_000)),
        ("p_u64", PropertyValue::U64(18_000_000_000_000_000_000)),
        ("p_f32", PropertyValue::F32(1.5)),
        ("p_f64", PropertyValue::F64(-2.25e-300)),
        ("p_str", PropertyValue::String("naïve 'text' 日本".into())),
        ("p_empty", PropertyValue::String(String::new())),
        ("p_bool", PropertyValue::Bool(true)),
        (
            "p_time",
            PropertyValue::Timestamp(Timestamp::new(3_800_000_000, 1 << 62)),
        ),
        (
            "p_c32",
            PropertyValue::ComplexF32(Complex32 { re: 1.0, im: -2.0 }),
        ),
        (
            "p_c64",
            PropertyValue::ComplexF64(Complex64 { re: 3.5, im: 4.5 }),
        ),
    ]
}

/// The choices of the conformance cross product.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Shape {
    pub(crate) dtype: DataType,
    pub(crate) big_endian: bool,
    pub(crate) interleaved: bool,
    pub(crate) chunks: u64,
    pub(crate) version: u32,
    pub(crate) padding: usize,
}

/// Every combination of the choices. A string channel is never interleaved,
/// because the format does not allow that.
pub(crate) fn shapes() -> Vec<Shape> {
    let mut out = Vec::new();
    for dtype in FIXED.into_iter().chain([DataType::String]) {
        for big_endian in [false, true] {
            for interleaved in [false, true] {
                if interleaved && dtype == DataType::String {
                    continue;
                }
                for chunks in [1, 3] {
                    for version in [4712, 4713] {
                        for padding in [0, 5] {
                            out.push(Shape {
                                dtype,
                                big_endian,
                                interleaved,
                                chunks,
                                version,
                                padding,
                            });
                        }
                    }
                }
            }
        }
    }
    out
}

fn strings(n: usize, salt: usize) -> Values {
    Values::String(
        (0..n)
            .map(|i| match (i + salt) % 4 {
                0 => String::new(),
                1 => "é".repeat(i + 1),
                2 => format!("s{i}-{salt}"),
                _ => "日本'quote'".to_owned(),
            })
            .collect(),
    )
}

/// The object for a channel of the shape's type with `n` values per chunk.
fn main_channel(name: &str, shape: Shape, n: u64) -> Object {
    let object = Object::channel("Group one", name);
    if shape.dtype == DataType::String {
        // Every chunk of a string channel has to have the same byte size.
        let one = strings(usize::try_from(n).expect("a small count"), name.len());
        let Values::String(one) = one else {
            unreachable!()
        };
        let all: Vec<String> = (0..shape.chunks).flat_map(|_| one.clone()).collect();
        object.data(Values::String(all))
    } else {
        object.generated(shape.dtype, n)
    }
}

/// A file of nine segments that walks through the metadata rules:
/// everything new, no metadata, one property changed and a channel added,
/// metadata only, a new object list that drops and reorders, a resize, a
/// channel switched off and back on.
pub(crate) fn scenario(shape: Shape) -> Built {
    const UNICODE: &str = "Température 'x' 日本";
    let mut builder = FileBuilder::new();
    let width_mate = if shape.dtype == DataType::I16 {
        DataType::F64
    } else {
        DataType::I16
    };
    let set = |segment: &mut buzzard_tdms::build::Segment| {
        segment
            .big_endian(shape.big_endian)
            .interleaved(shape.interleaved)
            .chunks(shape.chunks)
            .version(shape.version)
            .padding(shape.padding);
    };

    let segment = builder.segment();
    set(segment);
    let mut root = Object::root();
    // Complex properties have a test of their own, because npTDMS refuses a
    // file that has one.
    for (name, value) in every_property() {
        if !matches!(
            value,
            PropertyValue::ComplexF32(_) | PropertyValue::ComplexF64(_)
        ) {
            root = root.prop(name, value);
        }
    }
    segment
        .object(root)
        .object(Object::group("Group one").prop("title", PropertyValue::String("first".into())))
        .object(
            main_channel("a", shape, 4)
                .prop("unit_string", PropertyValue::String("bar".into()))
                .prop("status", PropertyValue::String("valid".into())),
        )
        .object(Object::channel("Group one", "b").generated(width_mate, 4))
        .object(main_channel(UNICODE, shape, 4));
    if !shape.interleaved {
        segment
            .object(Object::channel("Group one", "empty").generated(DataType::F64, 0))
            .object(Object::channel("Dr. T's Events", "never"));
    }

    set(builder.segment().no_metadata());

    let segment = builder.segment();
    set(segment);
    segment
        .object(
            Object::channel("Group one", "a")
                .same()
                .prop("status", PropertyValue::String("error".into()))
                .prop("added", PropertyValue::I32(7)),
        )
        .object(Object::channel("Second", "c").generated(DataType::F32, 4));

    let segment = builder.segment();
    set(segment);
    segment
        .no_raw()
        .object(Object::group("Group one").prop("title", PropertyValue::String("second".into())))
        .object(Object::group("Second"));

    let segment = builder.segment();
    set(segment);
    segment
        .new_object_list(true)
        .object(Object::channel("Group one", "b").same())
        .object(Object::channel("Group one", "a").same());

    let segment = builder.segment();
    set(segment);
    segment
        .object(main_channel("a", shape, 6))
        .object(Object::channel("Group one", "b").generated(width_mate, 6));

    let segment = builder.segment();
    set(segment);
    segment.object(Object::channel("Group one", "b").no_data());

    let segment = builder.segment();
    set(segment);
    segment.object(Object::channel("Group one", "b").same());

    set(builder.segment().no_metadata());
    builder.build()
}

/// The properties of a linear scale. Scale `number` is `slope * x +
/// intercept` of scale (or scaler) `source`.
fn linear(object: Object, number: u32, slope: f64, intercept: f64, source: u32) -> Object {
    object
        .prop(
            &format!("NI_Scale[{number}]_Scale_Type"),
            PropertyValue::String("Linear".into()),
        )
        .prop(
            &format!("NI_Scale[{number}]_Linear_Slope"),
            PropertyValue::F64(slope),
        )
        .prop(
            &format!("NI_Scale[{number}]_Linear_Y_Intercept"),
            PropertyValue::F64(intercept),
        )
        .prop(
            &format!("NI_Scale[{number}]_Linear_Input_Source"),
            PropertyValue::U32(source),
        )
}

fn unscaled(object: Object, scales: u32) -> Object {
    object
        .prop(
            "NI_Scaling_Status",
            PropertyValue::String("unscaled".into()),
        )
        .prop("NI_Number_Of_Scales", PropertyValue::U32(scales))
}

/// A file with channels in a driver's raw buffers: two buffers, a channel
/// with one scaler, a channel with two, a typed channel, and a channel with
/// a chain of two linear scales, followed by a segment without metadata.
/// Every channel of the raw type has scaling properties, as in files
/// written by acquisition drivers.
pub(crate) fn daqmx_scenario(big_endian: bool, chunks: u64) -> Built {
    use buzzard_tdms::build::generate;
    let n = 5 * chunks;
    let scaler = |dtype, seed, buffer, offset, scale_id| ScalerSpec {
        values: generate(dtype, seed, 0, n),
        buffer,
        offset,
        scale_id,
    };
    let channel = |name: &str| Object::channel("Measured", name);
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .big_endian(big_endian)
        .chunks(chunks)
        .daqmx_widths(&[12, 9])
        .object(Object::group("Measured"))
        .object(linear(
            unscaled(
                channel("ai0").daqmx(true, false, vec![scaler(DataType::I16, 1, 0, 0, 0)]),
                2,
            ),
            1,
            0.25,
            -3.0,
            0,
        ))
        .object(linear(
            unscaled(
                channel("ai1").daqmx(
                    true,
                    false,
                    vec![
                        scaler(DataType::I32, 2, 0, 4, 0),
                        scaler(DataType::F32, 3, 1, 1, 1),
                    ],
                ),
                3,
            ),
            2,
            2.0,
            0.5,
            1,
        ))
        .object(channel("typed").daqmx(false, false, vec![scaler(DataType::U16, 4, 0, 10, 0)]))
        .object(linear(
            linear(
                unscaled(
                    channel("wide").daqmx(true, false, vec![scaler(DataType::F32, 5, 1, 5, 0)]),
                    3,
                ),
                1,
                10.0,
                1.0,
                0,
            ),
            2,
            -0.5,
            7.0,
            1,
        ));
    builder
        .segment()
        .big_endian(big_endian)
        .chunks(chunks)
        .daqmx_widths(&[12, 9])
        .no_metadata();
    builder.build()
}

/// A repeatable sequence of bits that is different for every `salt`.
pub(crate) fn bit_pattern(salt: u64, n: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64 ^ (salt + 1).wrapping_mul(0xD6E8_FEB8_6659_FD93);
    std::iter::repeat_with(|| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        u8::from((state >> 40) & 1 == 1)
    })
    .take(n)
    .collect()
}

/// A file with digital lines: all sixteen lines of one two-byte port, typed
/// as acquisition drivers write them. Every line has a pattern of its own,
/// so a reader that takes the bit of a neighboring line reads a wrong value.
pub(crate) fn digital_scenario() -> Built {
    let mut builder = FileBuilder::new();
    let segment = builder.segment().daqmx_widths(&[2]);
    for bit in 0..16u32 {
        segment.object(Object::channel("port", &format!("line{bit}")).daqmx(
            false,
            true,
            vec![ScalerSpec {
                values: Values::U8(bit_pattern(u64::from(bit), 24)),
                buffer: 0,
                offset: bit,
                scale_id: 0,
            }],
        ));
    }
    builder.build()
}

/// The types a scaler can have that other readers know too.
pub(crate) const SCALER_TYPES: [DataType; 10] = [
    DataType::U8,
    DataType::I8,
    DataType::U16,
    DataType::I16,
    DataType::U32,
    DataType::I32,
    DataType::U64,
    DataType::I64,
    DataType::F32,
    DataType::F64,
];

/// A file with one driver channel for each of [`SCALER_TYPES`], side by
/// side in one raw buffer of 42 bytes a row, in two chunks and then a
/// segment without metadata. The channels have the raw-data type code or,
/// when `typed`, the type of their scaler.
pub(crate) fn every_scaler_scenario(big_endian: bool, typed: bool) -> Built {
    use buzzard_tdms::build::generate;
    let mut builder = FileBuilder::new();
    let segment = builder.segment();
    segment.big_endian(big_endian).chunks(2).daqmx_widths(&[42]);
    let mut offset = 0u32;
    for (number, dtype) in (0u64..).zip(SCALER_TYPES) {
        segment.object(Object::channel("types", dtype.name()).daqmx(
            !typed,
            false,
            vec![ScalerSpec {
                values: generate(dtype, number + 40, 0, 14),
                buffer: 0,
                offset,
                scale_id: 0,
            }],
        ));
        offset += u32::try_from(dtype.size().expect("a fixed size")).expect("small");
    }
    builder
        .segment()
        .big_endian(big_endian)
        .daqmx_widths(&[42])
        .no_metadata();
    builder.build()
}

/// Small files that each use one feature of the format that the cross
/// product does not reach. The generator's model describes every one of
/// them completely.
#[expect(
    clippy::too_many_lines,
    reason = "one list of cases, read top to bottom"
)]
pub(crate) fn feature_cases() -> Vec<(&'static str, Built)> {
    let channel = |name: &str| Object::channel("g", name);
    let text = |values: &[&str]| Values::String(values.iter().map(|s| (*s).to_owned()).collect());
    let mut cases = Vec::new();
    let mut case = |name: &'static str, fill: &dyn Fn(&mut FileBuilder)| {
        let mut builder = FileBuilder::new();
        fill(&mut builder);
        cases.push((name, builder.build()));
    };

    // The interleaved flag on a segment of a single string channel means
    // nothing. In a big-endian segment the string offsets are big-endian.
    case("be_interleaved_single_string", &|b| {
        b.segment()
            .big_endian(true)
            .interleaved(true)
            .object(channel("s").data(text(&["ab", "", "cde"])));
        b.segment().big_endian(true).interleaved(true).no_metadata();
    });
    case("byte_order_changes_without_metadata", &|b| {
        b.segment()
            .object(channel("a").generated(DataType::I32, 3))
            .object(channel("b").generated(DataType::F64, 2))
            .object(channel("t").generated(DataType::Timestamp, 2));
        b.segment().big_endian(true).no_metadata();
        b.segment().no_metadata();
        b.segment().big_endian(true).no_metadata().chunks(2);
    });
    case("float_with_unit_in_every_layout", &|b| {
        b.segment()
            .big_endian(true)
            .object(channel("single").generated(DataType::F32, 3).with_unit())
            .object(channel("double").generated(DataType::F64, 3).with_unit());
        b.segment().interleaved(true).chunks(2).no_metadata();
        b.segment().big_endian(true).interleaved(true).no_metadata();
        b.segment().no_metadata();
    });
    case("new_object_list_flag_without_metadata", &|b| {
        b.segment()
            .object(channel("a").generated(DataType::I32, 3))
            .object(channel("b").generated(DataType::F64, 2));
        b.segment().no_metadata().toc_xor(1 << 2);
        b.segment().object(channel("c").generated(DataType::I8, 5));
    });
    case("raw_data_flag_without_raw_data", &|b| {
        b.segment().object(channel("a").generated(DataType::I32, 4));
        b.segment()
            .no_raw()
            .toc_xor(1 << 3)
            .object(channel("a").same().prop("note", PropertyValue::I32(1)));
        b.segment().no_metadata().chunks(2);
    });
    case("no_data_then_an_index_keeps_the_position", &|b| {
        b.segment()
            .object(channel("a"))
            .object(channel("b").generated(DataType::I32, 3));
        b.segment().object(channel("a").generated(DataType::F64, 2));
        b.segment().no_metadata();
    });
    case("zero_length_index_then_same", &|b| {
        b.segment()
            .object(channel("a").generated(DataType::I32, 0))
            .object(channel("b").generated(DataType::I16, 2));
        b.segment().object(channel("a").same());
        b.segment().object(channel("a").generated(DataType::I32, 3));
        b.segment().object(channel("a").same());
    });
    case("metadata_only_new_list_then_data", &|b| {
        b.segment()
            .object(channel("a").generated(DataType::I32, 3))
            .object(channel("b").generated(DataType::F64, 2))
            .object(channel("c").generated(DataType::U8, 1));
        b.segment()
            .new_object_list(true)
            .no_raw()
            .object(channel("b").same())
            .object(channel("a").same());
        b.segment().no_metadata();
        b.segment().no_metadata().chunks(3);
    });
    case("interleaved_with_a_no_data_object_between", &|b| {
        b.segment()
            .interleaved(true)
            .chunks(2)
            .object(channel("a").generated(DataType::I32, 3))
            .object(Object::group("other"))
            .object(channel("quiet"))
            .object(channel("b").generated(DataType::F64, 3));
        b.segment().interleaved(true).no_metadata();
    });
    case("odd_names", &|b| {
        b.segment()
            .object(Object::channel("", "x").generated(DataType::I8, 2))
            .object(Object::channel("a/b", "c'd").generated(DataType::I16, 2))
            .object(Object::channel("a'", "'").generated(DataType::I32, 2))
            .object(Object::channel("g", "").generated(DataType::I64, 2))
            .object(Object::group("").prop("p", PropertyValue::I32(1)))
            .object(Object::group("a'").prop("p", PropertyValue::I32(2)));
        b.segment().no_metadata();
    });
    case("booleans_that_are_not_0_or_1", &|b| {
        b.segment()
            .object(channel("flags").data(Values::Bool(vec![0, 1, 2, 255, 128, 0])));
    });
    for big_endian in [false, true] {
        let name = if big_endian {
            "mixed_width_interleaved_be"
        } else {
            "mixed_width_interleaved_le"
        };
        case(name, &|b| {
            let segment = b.segment();
            segment.big_endian(big_endian).interleaved(true).chunks(4);
            for dtype in [
                DataType::I8,
                DataType::U16,
                DataType::F32,
                DataType::I64,
                DataType::Timestamp,
                DataType::Bool,
            ] {
                segment.object(channel(dtype.name()).generated(dtype, 3));
            }
            b.segment()
                .big_endian(!big_endian)
                .interleaved(true)
                .no_metadata();
        });
    }
    // In version 2.0 of the format a `name` property renames the object.
    // This reader keeps the path and the property (see the README).
    case("name_property", &|b| {
        b.segment().object(channel("a").generated(DataType::I32, 2));
        b.segment().object(
            channel("a")
                .same()
                .prop("name", PropertyValue::String("renamed".into())),
        );
    });
    // A data object joins the list after the layout of the list was used.
    case("data_object_appended_to_a_used_list", &|b| {
        b.segment().object(channel("a").generated(DataType::I32, 4));
        b.segment().no_metadata();
        b.segment()
            .new_object_list(false)
            .object(channel("b").generated(DataType::F64, 2));
        b.segment().no_metadata();
    });
    // An object that a new object list dropped joins the list again with
    // the "same as before" marker, and another with an index equal to its
    // last one. Neither entry says anything new about the object, and both
    // add it to the list.
    case("dropped_objects_return_to_a_used_list", &|b| {
        b.segment()
            .object(channel("a").generated(DataType::I32, 4))
            .object(channel("b").generated(DataType::F64, 2))
            .object(channel("c").generated(DataType::I16, 1));
        b.segment()
            .new_object_list(true)
            .object(channel("a").same());
        b.segment().no_metadata();
        b.segment().object(channel("b").same());
        b.segment().no_metadata();
        b.segment().object(channel("c").generated(DataType::I16, 1));
        b.segment().no_metadata();
    });
    case("property_changes_its_type", &|b| {
        b.segment()
            .object(Object::root().prop("p", PropertyValue::F64(1.5)))
            .object(
                channel("a")
                    .generated(DataType::I32, 2)
                    .prop("p", PropertyValue::I32(5))
                    .prop("q", PropertyValue::U8(9)),
            );
        b.segment()
            .object(Object::root().prop("p", PropertyValue::U8(2)))
            .object(
                channel("a")
                    .same()
                    .prop("p", PropertyValue::String("five".into())),
            );
        b.segment()
            .object(channel("a").same().prop("p", PropertyValue::F32(5.5)));
    });
    cases
}

/// A file with two driver channels: `a` (16-bit) and `b` (32-bit). With
/// one width they share a raw buffer. With two widths, `a` is in the first
/// buffer and `b` in the second. Each channel has four values, and `tail`
/// adds segments after them.
pub(crate) fn two_driver_channels(widths: &[u32], tail: &dyn Fn(&mut FileBuilder)) -> Built {
    use buzzard_tdms::build::generate;
    let scaler = |dtype, seed, buffer, offset| ScalerSpec {
        values: generate(dtype, seed, 0, 4),
        buffer,
        offset,
        scale_id: 0,
    };
    let second = u32::from(widths.len() > 1);
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .daqmx_widths(widths)
        .object(Object::channel("g", "a").daqmx(
            false,
            false,
            vec![scaler(DataType::I16, 11, 0, 0)],
        ))
        .object(Object::channel("g", "b").daqmx(
            false,
            false,
            vec![scaler(DataType::I32, 12, second, 2)],
        ));
    tail(&mut builder);
    builder.build()
}

/// The offset just after occurrence number `nth` (from zero) of `needle`.
pub(crate) fn after_nth(bytes: &[u8], needle: &[u8], nth: usize) -> usize {
    bytes
        .windows(needle.len())
        .enumerate()
        .filter(|(_, window)| *window == needle)
        .nth(nth)
        .expect("the bytes are in the file")
        .0
        + needle.len()
}

/// A file whose driver channel `a` declares other scalers in its second
/// segment than in its first. `more` adds a scaler, `type` changes the
/// scaler's type, and `id` changes its number.
pub(crate) fn scalers_change(how: &str) -> Built {
    let scaler = |values, offset, scale_id| ScalerSpec {
        values,
        buffer: 0,
        offset,
        scale_id,
    };
    let i16s = || Values::I16(vec![1, -2, 3, -4]);
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .daqmx_widths(&[8])
        .object(Object::channel("g", "a").daqmx(true, false, vec![scaler(i16s(), 0, 0)]));
    let second = match how {
        "more" => vec![scaler(i16s(), 0, 0), scaler(i16s(), 2, 1)],
        "id" => vec![scaler(i16s(), 0, 5)],
        "type" => vec![scaler(i16s(), 0, 0)],
        other => panic!("unknown case {other}"),
    };
    builder
        .segment()
        .daqmx_widths(&[8])
        .object(Object::channel("g", "a").daqmx(true, false, second));
    let mut built = builder.build();
    if how == "type" {
        // After the path come the marker, the channel's type, the dimension,
        // the count (8 bytes), the number of scalers, and the scaler's type.
        for bytes in [&mut built.bytes, &mut built.index_bytes] {
            let at = after_nth(bytes, b"/'g'/'a'", 1) + 24;
            bytes[at..at + 4].copy_from_slice(&5u32.to_le_bytes());
        }
    }
    built
}

/// Checks everything the model says against the reader: groups, channels,
/// types, lengths, every value (read whole, in ranges and in chunks) and
/// every property.
pub(crate) fn assert_matches(built: &Built, file: &TdmsFile, what: &str) {
    let found: Vec<(String, String)> = file
        .channels()
        .map(|c| (c.group_name().to_owned(), c.name().to_owned()))
        .collect();
    let mut expected: Vec<(String, String)> = built
        .channels
        .iter()
        .map(|c| (c.group.clone(), c.name.clone()))
        .collect();
    let mut sorted = found.clone();
    sorted.sort();
    expected.sort();
    assert_eq!(sorted, expected, "{what}: channels");

    let props = |p: &[buzzard_tdms::Property]| -> Vec<(String, DataType, PropertyValue)> {
        p.iter()
            .map(|p| (p.name.clone(), p.dtype, p.value.clone()))
            .collect()
    };
    assert_eq!(
        props(file.properties()),
        built.file_props,
        "{what}: file properties"
    );
    for (group, expected) in &built.group_props {
        let group = file
            .group(group)
            .unwrap_or_else(|| panic!("{what}: group {group}"));
        assert_eq!(
            &props(group.properties()),
            expected,
            "{what}: group properties"
        );
    }

    for expected in &built.channels {
        let label = format!("{what}: /'{}'/'{}'", expected.group, expected.name);
        let channel = file
            .channel(&expected.group, &expected.name)
            .unwrap_or_else(|| panic!("{label} is missing"));
        assert_eq!(
            props(channel.properties()),
            expected.props,
            "{label}: properties"
        );
        assert_eq!(
            channel.dtype(),
            expected.dtype.unwrap_or(DataType::Void),
            "{label}: type"
        );
        let Some(values) = &expected.values else {
            assert_eq!(channel.len(), 0, "{label}: length");
            continue;
        };
        let len = values.len() as u64;
        assert_eq!(channel.len(), len, "{label}: length");
        if expected.scalers.len() > 1 {
            for (id, values) in &expected.scalers {
                let read = channel
                    .read_scaler_column(*id, 0..len)
                    .unwrap_or_else(|e| panic!("{label}: scaler {id}: {e}"));
                assert_eq!(&read, values, "{label}: scaler {id}");
            }
            continue;
        }
        let all = channel
            .read_column(0..len)
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(&all, values, "{label}: values");
        // Ranges that start and end inside segments and chunks.
        for (from, to) in [
            (0, len.min(1)),
            (len / 3, len - len / 4),
            (len.saturating_sub(1), len),
        ] {
            let part = channel
                .read_column(from..to)
                .unwrap_or_else(|e| panic!("{label}: {from}..{to}: {e}"));
            let want = slice(values, from, to);
            assert_eq!(part, want, "{label}: values {from}..{to}");
        }
        assert!(
            channel.read_column(0..len + 1).is_err(),
            "{label}: past the end"
        );
    }
}

/// The values `from..to` of a column.
pub(crate) fn slice(values: &Values, from: u64, to: u64) -> Values {
    let from = usize::try_from(from).expect("a small index");
    let to = usize::try_from(to).expect("a small index");
    match values {
        Values::I8(v) => Values::I8(v[from..to].to_vec()),
        Values::I16(v) => Values::I16(v[from..to].to_vec()),
        Values::I32(v) => Values::I32(v[from..to].to_vec()),
        Values::I64(v) => Values::I64(v[from..to].to_vec()),
        Values::U8(v) => Values::U8(v[from..to].to_vec()),
        Values::U16(v) => Values::U16(v[from..to].to_vec()),
        Values::U32(v) => Values::U32(v[from..to].to_vec()),
        Values::U64(v) => Values::U64(v[from..to].to_vec()),
        Values::F32(v) => Values::F32(v[from..to].to_vec()),
        Values::F64(v) => Values::F64(v[from..to].to_vec()),
        Values::Bool(v) => Values::Bool(v[from..to].to_vec()),
        Values::Timestamp(v) => Values::Timestamp(v[from..to].to_vec()),
        Values::ComplexF32(v) => Values::ComplexF32(v[from..to].to_vec()),
        Values::ComplexF64(v) => Values::ComplexF64(v[from..to].to_vec()),
        Values::String(v) => Values::String(v[from..to].to_vec()),
        _ => unreachable!("a new kind of column"),
    }
}

/// Opens a file from memory, with or without an index file.
pub(crate) fn open(bytes: &[u8], index: Option<&[u8]>) -> buzzard_tdms::Result<TdmsFile> {
    let index = index.map(<[u8]>::to_vec);
    let index: Option<&dyn buzzard_tdms::ReadAt> = match &index {
        Some(bytes) => Some(bytes),
        None => None,
    };
    buzzard_tdms::OpenOptions::new().open_source(std::sync::Arc::new(bytes.to_vec()), index)
}
