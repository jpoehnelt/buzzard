//! Shared by the integration tests: the files the generator writes for the
//! conformance cross product, and the comparison of a reader's view with
//! the generator's model.

#![allow(
    dead_code,
    reason = "each test crate uses a different subset of these helpers"
)]

use buzzard_tdms::build::{Built, FileBuilder, Object, ScalerSpec, Values};
use buzzard_tdms::{Complex32, Complex64, DataType, PropertyValue, TdmsFile, Timestamp};

/// Every fixed-size type a channel can have, and strings.
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

/// Every combination; strings only where the format allows them.
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
    for (name, value) in every_property() {
        root = root.prop(name, value);
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

/// A file with channels in a driver's raw buffers: two buffers, a channel
/// with one scaler, a channel with two, and a typed channel, followed by a
/// segment without metadata.
pub(crate) fn daqmx_scenario(big_endian: bool, chunks: u64) -> Built {
    use buzzard_tdms::build::generate;
    let n = 5 * chunks;
    let scaler = |dtype, seed, buffer, offset, scale_id| ScalerSpec {
        values: generate(dtype, seed, 0, n),
        buffer,
        offset,
        scale_id,
    };
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .big_endian(big_endian)
        .chunks(chunks)
        .daqmx_widths(&[12, 9])
        .object(Object::group("Measured"))
        .object(
            Object::channel("Measured", "ai0")
                .daqmx(true, false, vec![scaler(DataType::I16, 1, 0, 0, 0)])
                .prop(
                    "NI_Scaling_Status",
                    PropertyValue::String("unscaled".into()),
                )
                .prop("NI_Number_Of_Scales", PropertyValue::U32(2))
                .prop(
                    "NI_Scale[1]_Scale_Type",
                    PropertyValue::String("Linear".into()),
                )
                .prop("NI_Scale[1]_Linear_Slope", PropertyValue::F64(0.25))
                .prop("NI_Scale[1]_Linear_Y_Intercept", PropertyValue::F64(-3.0))
                .prop("NI_Scale[1]_Linear_Input_Source", PropertyValue::U32(0)),
        )
        .object(Object::channel("Measured", "ai1").daqmx(
            true,
            false,
            vec![
                scaler(DataType::I32, 2, 0, 4, 0),
                scaler(DataType::F32, 3, 1, 1, 1),
            ],
        ))
        .object(Object::channel("Measured", "typed").daqmx(
            false,
            false,
            vec![scaler(DataType::U16, 4, 0, 10, 0)],
        ))
        .object(Object::channel("Measured", "wide").daqmx(
            true,
            false,
            vec![scaler(DataType::F32, 5, 1, 5, 0)],
        ));
    builder
        .segment()
        .big_endian(big_endian)
        .chunks(chunks)
        .daqmx_widths(&[12, 9])
        .no_metadata();
    builder.build()
}

/// A file with digital lines: four lines of one two-byte port.
pub(crate) fn digital_scenario() -> Built {
    let line = |bit: u32, pattern: [u8; 6]| ScalerSpec {
        values: Values::U8(pattern.to_vec()),
        buffer: 0,
        offset: bit,
        scale_id: 0,
    };
    let mut builder = FileBuilder::new();
    builder
        .segment()
        .daqmx_widths(&[2])
        .object(Object::channel("port", "line0").daqmx(
            true,
            true,
            vec![line(0, [1, 0, 1, 1, 0, 0])],
        ))
        .object(Object::channel("port", "line3").daqmx(
            true,
            true,
            vec![line(3, [0, 0, 1, 0, 1, 1])],
        ))
        .object(Object::channel("port", "line9").daqmx(
            true,
            true,
            vec![line(9, [1, 1, 1, 0, 0, 1])],
        ))
        .object(Object::channel("port", "line15").daqmx(
            true,
            true,
            vec![line(15, [0, 1, 0, 1, 0, 1])],
        ));
    builder.build()
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
