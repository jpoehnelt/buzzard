//! A writer of synthetic TDMS files for tests and benchmarks, with a model
//! of what each file holds.
//!
//! This is test support, not a general writer. It is written from the
//! format description alone and shares no code with the reader: it has its
//! own encoder and its own account of which channels a segment carries, so
//! that the contents it [expects](Built::channels) are an independent
//! statement of what a reader must find.
//!
//! A file is a list of [`Segment`]s. Each lists the [`Object`]s whose
//! metadata it writes. A channel that an earlier segment gave a raw data
//! index, and that this segment does not mention (or mentions with
//! [`Object::same`]), is written again with generated values, as the format
//! requires of a segment that leaves metadata out.

#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "generated test values are cut down from a hash on purpose"
)]

use std::collections::HashMap;

use crate::types::{Column, Complex32, Complex64, DataType, PropertyValue, Timestamp};

/// The values of one channel: an alias of [`Column`].
pub type Values = Column;

const TOC_META: u32 = 1 << 1;
const TOC_NEW_OBJ_LIST: u32 = 1 << 2;
const TOC_RAW: u32 = 1 << 3;
const TOC_INTERLEAVED: u32 = 1 << 5;
const TOC_BIG_ENDIAN: u32 = 1 << 6;
const TOC_DAQMX: u32 = 1 << 7;

/// One scaler of a channel in a driver's raw buffers.
#[derive(Clone, Debug, PartialEq)]
pub struct ScalerSpec {
    /// The scaler's values; an integer or float column. For a digital line
    /// the values must be 0 or 1.
    pub values: Values,
    /// Which raw buffer the values are in.
    pub buffer: u32,
    /// Byte offset of the value in a row of the buffer; for a digital line,
    /// the bit offset.
    pub offset: u32,
    /// The number scaling properties refer to the scaler by.
    pub scale_id: u32,
}

#[derive(Clone, Debug, PartialEq)]
enum IndexSpec {
    None,
    Same,
    Data(Values),
    Generated(DataType, u64),
    Daqmx {
        raw_type: bool,
        digital: bool,
        scalers: Vec<ScalerSpec>,
    },
}

/// The metadata one segment writes for one object.
#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    path: String,
    index: IndexSpec,
    with_unit: bool,
    props: Vec<(String, DataType, PropertyValue)>,
    index_length: Option<u32>,
}

fn quote(name: &str) -> String {
    format!("/'{}'", name.replace('\'', "''"))
}

impl Object {
    fn at(path: String) -> Self {
        Self {
            path,
            index: IndexSpec::None,
            with_unit: false,
            props: Vec::new(),
            index_length: None,
        }
    }

    /// The file object, `/`.
    pub fn root() -> Self {
        Self::at("/".to_owned())
    }

    /// A group.
    pub fn group(name: &str) -> Self {
        Self::at(quote(name))
    }

    /// A channel. Without one of the methods below it has no raw data in
    /// the segment.
    pub fn channel(group: &str, name: &str) -> Self {
        Self::at(quote(group) + &quote(name))
    }

    /// An object with a path written exactly as given, valid or not.
    pub fn raw_path(path: &str) -> Self {
        Self::at(path.to_owned())
    }

    /// Writes a new raw data index and these values. In a segment of
    /// several chunks the values are divided evenly over the chunks.
    #[must_use]
    pub fn data(mut self, values: Values) -> Self {
        self.index = IndexSpec::Data(values);
        self
    }

    /// Writes a new raw data index for `n` values per chunk of generated
    /// data.
    #[must_use]
    pub fn generated(mut self, dtype: DataType, n: u64) -> Self {
        self.index = IndexSpec::Generated(dtype, n);
        self
    }

    /// Writes the "same raw data index as before" marker; the values are
    /// generated.
    #[must_use]
    pub fn same(mut self) -> Self {
        self.index = IndexSpec::Same;
        self
    }

    /// Writes the "no raw data in this segment" marker. This is the
    /// default.
    #[must_use]
    pub fn no_data(mut self) -> Self {
        self.index = IndexSpec::None;
        self
    }

    /// Writes a raw data index for a driver's raw buffers. `raw_type`
    /// gives the channel the raw-data type code; otherwise it has the type
    /// of its single scaler. `digital` writes digital line scalers.
    #[must_use]
    pub fn daqmx(mut self, raw_type: bool, digital: bool, scalers: Vec<ScalerSpec>) -> Self {
        self.index = IndexSpec::Daqmx {
            raw_type,
            digital,
            scalers,
        };
        self
    }

    /// Stores a float channel under the "float with unit" type code.
    #[must_use]
    pub fn with_unit(mut self) -> Self {
        self.with_unit = true;
        self
    }

    /// Sets a property.
    #[must_use]
    pub fn prop(mut self, name: &str, value: PropertyValue) -> Self {
        let dtype = match value {
            PropertyValue::I8(_) => DataType::I8,
            PropertyValue::I16(_) => DataType::I16,
            PropertyValue::I32(_) => DataType::I32,
            PropertyValue::I64(_) => DataType::I64,
            PropertyValue::U8(_) => DataType::U8,
            PropertyValue::U16(_) => DataType::U16,
            PropertyValue::U32(_) => DataType::U32,
            PropertyValue::U64(_) => DataType::U64,
            PropertyValue::F32(_) => DataType::F32,
            PropertyValue::F64(_) => DataType::F64,
            PropertyValue::String(_) => DataType::String,
            PropertyValue::Bool(_) => DataType::Bool,
            PropertyValue::Timestamp(_) => DataType::Timestamp,
            PropertyValue::ComplexF32(_) => DataType::ComplexF32,
            PropertyValue::ComplexF64(_) => DataType::ComplexF64,
        };
        self.props.push((name.to_owned(), dtype, value));
        self
    }

    /// Writes this number as the length of the raw data index, whatever
    /// the index holds. For tests of damaged files.
    #[must_use]
    pub fn index_length(mut self, length: u32) -> Self {
        self.index_length = Some(length);
        self
    }
}

/// One segment to write.
#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is one independent choice of the format"
)]
pub struct Segment {
    big_endian: bool,
    interleaved: bool,
    new_object_list: Option<bool>,
    metadata: bool,
    version: u32,
    chunks: u64,
    padding: usize,
    unfinished: bool,
    objects: Vec<Object>,
    widths: Vec<u32>,
    extra: Vec<u8>,
    toc_xor: u32,
    raw: bool,
}

impl Segment {
    fn new() -> Self {
        Self {
            big_endian: false,
            interleaved: false,
            new_object_list: None,
            metadata: true,
            version: 4713,
            chunks: 1,
            padding: 0,
            unfinished: false,
            objects: Vec::new(),
            widths: Vec::new(),
            extra: Vec::new(),
            toc_xor: 0,
            raw: true,
        }
    }

    /// Writes every number of the segment big-endian.
    pub fn big_endian(&mut self, yes: bool) -> &mut Self {
        self.big_endian = yes;
        self
    }

    /// Interleaves the channels' values.
    pub fn interleaved(&mut self, yes: bool) -> &mut Self {
        self.interleaved = yes;
        self
    }

    /// Sets or clears the "new object list" flag: the segment's channels
    /// are then exactly the objects it lists. By default the first segment
    /// sets it and the others do not.
    pub fn new_object_list(&mut self, yes: bool) -> &mut Self {
        self.new_object_list = Some(yes);
        self
    }

    /// Writes no metadata at all; the segment's objects are ignored.
    pub fn no_metadata(&mut self) -> &mut Self {
        self.metadata = false;
        self
    }

    /// The version number in the lead-in (4713 by default).
    pub fn version(&mut self, version: u32) -> &mut Self {
        self.version = version;
        self
    }

    /// Writes this many chunks of raw data after one lead-in.
    pub fn chunks(&mut self, chunks: u64) -> &mut Self {
        self.chunks = chunks;
        self
    }

    /// Writes this many unused bytes between the metadata and the raw data.
    pub fn padding(&mut self, bytes: usize) -> &mut Self {
        self.padding = bytes;
        self
    }

    /// Leaves the segment's length field as a crashed writer does: all
    /// bits set.
    pub fn unfinished(&mut self) -> &mut Self {
        self.unfinished = true;
        self
    }

    /// The widths of the driver's raw buffers, for segments with
    /// [`Object::daqmx`] channels.
    pub fn daqmx_widths(&mut self, widths: &[u32]) -> &mut Self {
        self.widths = widths.to_vec();
        self
    }

    /// Appends bytes to the segment's raw data that the model does not
    /// account for.
    pub fn extra_raw(&mut self, bytes: &[u8]) -> &mut Self {
        self.extra = bytes.to_vec();
        self
    }

    /// Writes no raw data, whatever channels the segment has.
    pub fn no_raw(&mut self) -> &mut Self {
        self.raw = false;
        self
    }

    /// Flips these bits of the table of contents after it is computed.
    pub fn toc_xor(&mut self, bits: u32) -> &mut Self {
        self.toc_xor = bits;
        self
    }

    /// Adds an object to the segment's metadata.
    pub fn object(&mut self, object: Object) -> &mut Self {
        self.objects.push(object);
        self
    }
}

/// Properties in the order they were first set: name, type code, value.
pub type Props = Vec<(String, DataType, PropertyValue)>;

/// What a reader must find in one channel.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpectedChannel {
    /// Name of the group.
    pub group: String,
    /// Name of the channel.
    pub name: String,
    /// The type code the file gives the channel; `None` when it never had
    /// a raw data index.
    pub dtype: Option<DataType>,
    /// Every value, in order. For a driver channel, those of its first
    /// scaler.
    pub values: Option<Values>,
    /// For a driver channel: every scaler's values by scale id.
    pub scalers: Vec<(u32, Values)>,
    /// The properties, in the order they were first set.
    pub props: Props,
}

/// A written file and what it holds.
#[derive(Clone, Debug)]
pub struct Built {
    /// The data file.
    pub bytes: Vec<u8>,
    /// The matching index file.
    pub index_bytes: Vec<u8>,
    /// Every channel, in order of first appearance.
    pub channels: Vec<ExpectedChannel>,
    /// The properties of the file object.
    pub file_props: Props,
    /// The groups that have an object of their own, with their properties.
    pub group_props: Vec<(String, Props)>,
    /// Where each segment starts in the data file.
    pub segment_offsets: Vec<u64>,
    /// Where each segment's raw data starts in the data file.
    pub data_offsets: Vec<u64>,
}

/// Builds a file segment by segment.
#[derive(Clone, Debug, Default)]
pub struct FileBuilder {
    segments: Vec<Segment>,
}

struct Enc {
    out: Vec<u8>,
    big: bool,
}

impl Enc {
    fn u16(&mut self, v: u16) {
        self.out.extend(if self.big {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        });
    }

    fn u32(&mut self, v: u32) {
        self.out.extend(if self.big {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        });
    }

    fn u64(&mut self, v: u64) {
        self.out.extend(if self.big {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        });
    }

    fn text(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.out.extend(s.as_bytes());
    }

    fn timestamp(&mut self, t: Timestamp) {
        if self.big {
            self.u64(t.seconds as u64);
            self.u64(t.fractions);
        } else {
            self.u64(t.fractions);
            self.u64(t.seconds as u64);
        }
    }

    fn property(&mut self, value: &PropertyValue) {
        match *value {
            PropertyValue::I8(v) => self.out.push(v as u8),
            PropertyValue::I16(v) => self.u16(v as u16),
            PropertyValue::I32(v) => self.u32(v as u32),
            PropertyValue::I64(v) => self.u64(v as u64),
            PropertyValue::U8(v) => self.out.push(v),
            PropertyValue::U16(v) => self.u16(v),
            PropertyValue::U32(v) => self.u32(v),
            PropertyValue::U64(v) => self.u64(v),
            PropertyValue::F32(v) => self.u32(v.to_bits()),
            PropertyValue::F64(v) => self.u64(v.to_bits()),
            PropertyValue::String(ref v) => self.text(v),
            PropertyValue::Bool(v) => self.out.push(u8::from(v)),
            PropertyValue::Timestamp(v) => self.timestamp(v),
            PropertyValue::ComplexF32(v) => {
                self.u32(v.re.to_bits());
                self.u32(v.im.to_bits());
            }
            PropertyValue::ComplexF64(v) => {
                self.u64(v.re.to_bits());
                self.u64(v.im.to_bits());
            }
        }
    }

    /// One value of a fixed-size column.
    fn value(&mut self, values: &Values, i: usize) {
        match values {
            Values::I8(v) => self.out.push(v[i] as u8),
            Values::I16(v) => self.u16(v[i] as u16),
            Values::I32(v) => self.u32(v[i] as u32),
            Values::I64(v) => self.u64(v[i] as u64),
            Values::U8(v) | Values::Bool(v) => self.out.push(v[i]),
            Values::U16(v) => self.u16(v[i]),
            Values::U32(v) => self.u32(v[i]),
            Values::U64(v) => self.u64(v[i]),
            Values::F32(v) => self.u32(v[i].to_bits()),
            Values::F64(v) => self.u64(v[i].to_bits()),
            Values::Timestamp(v) => self.timestamp(v[i]),
            Values::ComplexF32(v) => {
                self.u32(v[i].re.to_bits());
                self.u32(v[i].im.to_bits());
            }
            Values::ComplexF64(v) => {
                self.u64(v[i].re.to_bits());
                self.u64(v[i].im.to_bits());
            }
            Values::String(_) => panic!("a string is not a fixed-size value"),
        }
    }
}

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn seed_of(path: &str) -> u64 {
    path.bytes().fold(0xCBF2_9CE4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3)
    })
}

/// The values number `start..start + n` of the generated sequence for a
/// seed: every bit pattern for integers, finite numbers for floats.
///
/// # Panics
///
/// For a type without a fixed size.
pub fn generate(dtype: DataType, seed: u64, start: u64, n: u64) -> Values {
    let x = |k: u64| mix(seed ^ mix(start + k));
    let f = |k: u64| (x(k) >> 11) as f64 / (1u64 << 53) as f64 * 2000.0 - 1000.0;
    let ks = 0..n;
    match dtype.without_unit() {
        DataType::I8 => Values::I8(ks.map(|k| x(k) as i8).collect()),
        DataType::I16 => Values::I16(ks.map(|k| x(k) as i16).collect()),
        DataType::I32 => Values::I32(ks.map(|k| x(k) as i32).collect()),
        DataType::I64 => Values::I64(ks.map(|k| x(k) as i64).collect()),
        DataType::U8 => Values::U8(ks.map(|k| x(k) as u8).collect()),
        DataType::U16 => Values::U16(ks.map(|k| x(k) as u16).collect()),
        DataType::U32 => Values::U32(ks.map(|k| x(k) as u32).collect()),
        DataType::U64 => Values::U64(ks.map(x).collect()),
        DataType::F32 => Values::F32(ks.map(|k| f(k) as f32).collect()),
        DataType::F64 => Values::F64(ks.map(f).collect()),
        DataType::Bool => Values::Bool(ks.map(|k| (x(k) & 1) as u8).collect()),
        DataType::Timestamp => Values::Timestamp(
            ks.map(|k| Timestamp::new((x(k) >> 28) as i64 - (1 << 35), mix(x(k))))
                .collect(),
        ),
        DataType::ComplexF32 => Values::ComplexF32(
            ks.map(|k| Complex32 {
                re: f(k) as f32,
                im: f(k ^ (1 << 40)) as f32,
            })
            .collect(),
        ),
        DataType::ComplexF64 => Values::ComplexF64(
            ks.map(|k| Complex64 {
                re: f(k),
                im: f(k ^ (1 << 40)),
            })
            .collect(),
        ),
        other => panic!("cannot generate values of type {other}"),
    }
}

/// Generated strings of the given byte lengths.
fn generate_strings(seed: u64, start: u64, lens: &[usize]) -> Values {
    Values::String(
        lens.iter()
            .enumerate()
            .map(|(k, &len)| {
                let x = mix(seed ^ mix(start + k as u64));
                (0..len)
                    .map(|i| char::from(b'a' + ((x >> (i % 13 * 4)) % 26) as u8))
                    .collect()
            })
            .collect(),
    )
}

fn slice(values: &Values, from: usize, to: usize) -> Values {
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
    }
}

/// The first `n` values of a column.
pub fn prefix(values: &Values, n: usize) -> Values {
    slice(values, 0, n.min(values.len()))
}

/// Appends the values of `more` to `to`.
///
/// # Panics
///
/// When the two columns have different types.
pub fn append(to: &mut Values, more: &Values) {
    match (to, more) {
        (Values::I8(a), Values::I8(b)) => a.extend(b),
        (Values::I16(a), Values::I16(b)) => a.extend(b),
        (Values::I32(a), Values::I32(b)) => a.extend(b),
        (Values::I64(a), Values::I64(b)) => a.extend(b),
        (Values::U8(a), Values::U8(b)) | (Values::Bool(a), Values::Bool(b)) => a.extend(b),
        (Values::U16(a), Values::U16(b)) => a.extend(b),
        (Values::U32(a), Values::U32(b)) => a.extend(b),
        (Values::U64(a), Values::U64(b)) => a.extend(b),
        (Values::F32(a), Values::F32(b)) => a.extend(b),
        (Values::F64(a), Values::F64(b)) => a.extend(b),
        (Values::Timestamp(a), Values::Timestamp(b)) => a.extend(b),
        (Values::ComplexF32(a), Values::ComplexF32(b)) => a.extend(b),
        (Values::ComplexF64(a), Values::ComplexF64(b)) => a.extend(b),
        (Values::String(a), Values::String(b)) => a.extend(b.iter().cloned()),
        (a, b) => panic!("a {} channel cannot take {} values", a.dtype(), b.dtype()),
    }
}

fn daqmx_code(dtype: DataType) -> u32 {
    match dtype {
        DataType::U8 => 0,
        DataType::I8 => 1,
        DataType::U16 => 2,
        DataType::I16 => 3,
        DataType::U32 => 4,
        DataType::I32 => 5,
        DataType::U64 => 6,
        DataType::I64 => 7,
        DataType::F32 => 8,
        DataType::F64 => 9,
        DataType::Timestamp => 0xFFFF_FFFF,
        other => panic!("{other} is not a scaler type"),
    }
}

/// What the model remembers of a channel's last raw data index.
#[derive(Clone)]
enum Remembered {
    Plain {
        dtype: DataType,
        n: u64,
        /// For strings: the byte length of each string of a chunk.
        lens: Vec<usize>,
    },
    Daqmx {
        digital: bool,
        n: u64,
        /// With empty value columns.
        scalers: Vec<ScalerSpec>,
    },
}

struct Model {
    path: String,
    names: Option<(String, Option<String>)>,
    seed: u64,
    counter: u64,
    file_type: Option<DataType>,
    remembered: Option<Remembered>,
    values: Option<Values>,
    scalers: Vec<(u32, Values)>,
    props: Vec<(String, DataType, PropertyValue)>,
}

/// This segment's values for one channel that has data in it.
enum Payload {
    Plain(Values),
    Daqmx(Vec<ScalerSpec>, bool),
}

fn names_of(path: &str) -> Option<(String, Option<String>)> {
    // Paths made by `Object::group` and `Object::channel`; a raw path is
    // not modelled.
    let mut names = Vec::new();
    let mut rest = path;
    while let Some(after) = rest.strip_prefix("/'") {
        let mut name = String::new();
        let mut chars = after.char_indices();
        let end = loop {
            let (at, c) = chars.next()?;
            if c != '\'' {
                name.push(c);
            } else if after[at + 1..].starts_with('\'') {
                chars.next();
                name.push('\'');
            } else {
                break at + 1;
            }
        };
        names.push(name);
        rest = &after[end..];
    }
    if !rest.is_empty() {
        return None;
    }
    let mut names = names.into_iter();
    let group = names.next()?;
    let channel = names.next();
    names.next().is_none().then_some((group, channel))
}

impl FileBuilder {
    /// A builder without segments.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a segment and returns it to be filled in.
    ///
    /// # Panics
    ///
    /// Never; the segment was just added.
    pub fn segment(&mut self) -> &mut Segment {
        self.segments.push(Segment::new());
        self.segments.last_mut().expect("just pushed")
    }

    /// Writes the file.
    ///
    /// # Panics
    ///
    /// When a segment asks for something that cannot be written: values
    /// that do not divide evenly over the chunks, interleaved channels of
    /// different lengths or with strings, a "same index" marker for an
    /// object that never had an index, or string chunks of unequal size.
    #[expect(
        clippy::too_many_lines,
        reason = "one pass over the segments, kept in file order"
    )]
    pub fn build(&self) -> Built {
        let mut models: Vec<Model> = Vec::new();
        let mut model_of: HashMap<&str, usize> = HashMap::new();
        // Position of each model in `list`, when it is there.
        let mut listed: Vec<Option<usize>> = Vec::new();
        // The current object list: index into `models`, and whether the
        // object has data.
        let mut list: Vec<(usize, bool)> = Vec::new();
        let mut bytes = Vec::new();
        let mut index_bytes = Vec::new();
        let mut segment_offsets = Vec::new();
        let mut data_offsets = Vec::new();

        for (number, segment) in self.segments.iter().enumerate() {
            let big = segment.big_endian;
            let chunks = segment.chunks;
            let new_list = segment.new_object_list.unwrap_or(number == 0) && segment.metadata;
            let mut meta = Enc {
                out: Vec::new(),
                big,
            };
            // Explicit payloads of this segment, by model.
            let mut explicit: Vec<(usize, Payload)> = Vec::new();
            if segment.metadata {
                if new_list {
                    list.clear();
                    listed.fill(None);
                }
                meta.u32(segment.objects.len() as u32);
                for object in &segment.objects {
                    let at = *model_of.entry(object.path.as_str()).or_insert_with(|| {
                        models.push(Model {
                            path: object.path.clone(),
                            names: names_of(&object.path),
                            seed: seed_of(&object.path),
                            counter: 0,
                            file_type: None,
                            remembered: None,
                            values: None,
                            scalers: Vec::new(),
                            props: Vec::new(),
                        });
                        listed.push(None);
                        models.len() - 1
                    });
                    let model = &mut models[at];
                    meta.text(&object.path);
                    let has_data = match &object.index {
                        IndexSpec::None => {
                            meta.u32(0xFFFF_FFFF);
                            false
                        }
                        IndexSpec::Same => {
                            assert!(
                                model.remembered.is_some(),
                                "{} has no raw data index to repeat",
                                object.path
                            );
                            meta.u32(0);
                            true
                        }
                        IndexSpec::Data(_) | IndexSpec::Generated(..) => {
                            let (dtype, n, values) = match &object.index {
                                IndexSpec::Data(values) => {
                                    let len = values.len() as u64;
                                    assert!(
                                        len.is_multiple_of(chunks),
                                        "{}: {len} values do not divide over {chunks} chunks",
                                        object.path
                                    );
                                    (values.dtype(), len / chunks, Some(values.clone()))
                                }
                                IndexSpec::Generated(dtype, n) => (*dtype, *n, None),
                                _ => unreachable!("matched above"),
                            };
                            let file_type = match (dtype, object.with_unit) {
                                (DataType::F32, true) => DataType::F32WithUnit,
                                (DataType::F64, true) => DataType::F64WithUnit,
                                (other, _) => other,
                            };
                            let lens: Vec<usize> = match &values {
                                Some(Values::String(strings)) => {
                                    strings[..n as usize].iter().map(String::len).collect()
                                }
                                _ => Vec::new(),
                            };
                            let is_string = dtype == DataType::String;
                            meta.u32(object.index_length.unwrap_or(if is_string {
                                28
                            } else {
                                20
                            }));
                            meta.u32(file_type.code());
                            meta.u32(1);
                            meta.u64(n);
                            if is_string {
                                let chars: usize = lens.iter().sum();
                                meta.u64(n * 4 + chars as u64);
                            }
                            model.file_type = Some(file_type);
                            model.remembered = Some(Remembered::Plain { dtype, n, lens });
                            if let Some(values) = values {
                                explicit.push((at, Payload::Plain(values)));
                            }
                            true
                        }
                        IndexSpec::Daqmx {
                            raw_type,
                            digital,
                            scalers,
                        } => {
                            let len = scalers.first().map_or(0, |s| s.values.len()) as u64;
                            assert!(len.is_multiple_of(chunks), "{}: uneven chunks", object.path);
                            let n = len / chunks;
                            meta.u32(if *digital { 0x126A } else { 0x1269 });
                            let file_type = if *raw_type {
                                DataType::DaqmxRaw
                            } else {
                                scalers[0].values.dtype()
                            };
                            meta.u32(file_type.code());
                            meta.u32(1);
                            meta.u64(n);
                            meta.u32(scalers.len() as u32);
                            for scaler in scalers {
                                meta.u32(daqmx_code(scaler.values.dtype()));
                                meta.u32(scaler.buffer);
                                meta.u32(scaler.offset);
                                if *digital {
                                    meta.out.push(0);
                                } else {
                                    meta.u32(0);
                                }
                                meta.u32(scaler.scale_id);
                            }
                            meta.u32(segment.widths.len() as u32);
                            for &width in &segment.widths {
                                meta.u32(width);
                            }
                            model.file_type = Some(file_type);
                            model.remembered = Some(Remembered::Daqmx {
                                digital: *digital,
                                n,
                                scalers: scalers
                                    .iter()
                                    .map(|s| ScalerSpec {
                                        values: slice(&s.values, 0, 0),
                                        ..s.clone()
                                    })
                                    .collect(),
                            });
                            explicit.push((at, Payload::Daqmx(scalers.clone(), *digital)));
                            true
                        }
                    };
                    meta.u32(object.props.len() as u32);
                    for (name, dtype, value) in &object.props {
                        meta.text(name);
                        meta.u32(dtype.code());
                        meta.property(value);
                        match model.props.iter_mut().find(|(n, _, _)| n == name) {
                            Some(slot) => *slot = (name.clone(), *dtype, value.clone()),
                            None => model.props.push((name.clone(), *dtype, value.clone())),
                        }
                    }
                    if let Some(position) = listed[at] {
                        list[position].1 = has_data;
                    } else {
                        listed[at] = Some(list.len());
                        list.push((at, has_data));
                    }
                }
            }
            meta.out.extend(std::iter::repeat_n(0u8, segment.padding));

            // The channels with data, in list order, with their values.
            let mut payloads: Vec<(usize, Payload)> = Vec::new();
            for &(at, has_data) in &list {
                if !has_data || !segment.raw {
                    continue;
                }
                let payload = if let Some(found) = explicit.iter().position(|(m, _)| *m == at) {
                    explicit.swap_remove(found).1
                } else {
                    generated_payload(&models[at], chunks)
                };
                payloads.push((at, payload));
            }

            let any_daqmx = payloads
                .iter()
                .any(|(_, p)| matches!(p, Payload::Daqmx(..)));
            let mut raw = Enc {
                out: Vec::new(),
                big,
            };
            if any_daqmx {
                write_daqmx(&mut raw, &payloads, &segment.widths, chunks);
            } else if segment.interleaved && payloads.len() > 1 {
                let rows = match &payloads[0].1 {
                    Payload::Plain(v) => v.len(),
                    Payload::Daqmx(..) => 0,
                };
                for row in 0..rows {
                    for (_, payload) in &payloads {
                        let Payload::Plain(values) = payload else {
                            continue;
                        };
                        assert_eq!(values.len(), rows, "interleaved channels differ in length");
                        raw.value(values, row);
                    }
                }
            } else {
                for chunk in 0..chunks as usize {
                    for (_, payload) in &payloads {
                        let Payload::Plain(values) = payload else {
                            continue;
                        };
                        let n = values.len() / chunks as usize;
                        let (from, to) = (chunk * n, (chunk + 1) * n);
                        if let Values::String(strings) = values {
                            let part = &strings[from..to];
                            let first: usize = strings[..n].iter().map(String::len).sum();
                            let size: usize = part.iter().map(String::len).sum();
                            assert_eq!(size, first, "string chunks differ in size");
                            let mut end = 0u32;
                            for s in part {
                                end += s.len() as u32;
                                raw.u32(end);
                            }
                            for s in part {
                                raw.out.extend(s.as_bytes());
                            }
                        } else {
                            for i in from..to {
                                raw.value(values, i);
                            }
                        }
                    }
                }
            }
            raw.out.extend(&segment.extra);

            // The model takes the values.
            for (at, payload) in payloads {
                let model = &mut models[at];
                match payload {
                    Payload::Plain(values) => {
                        model.counter += values.len() as u64;
                        match &mut model.values {
                            Some(all) => append(all, &values),
                            None => model.values = Some(values),
                        }
                    }
                    Payload::Daqmx(scalers, _) => {
                        model.counter += scalers.first().map_or(0, |s| s.values.len()) as u64;
                        for (i, scaler) in scalers.into_iter().enumerate() {
                            if i == 0 {
                                match &mut model.values {
                                    Some(all) => append(all, &scaler.values),
                                    None => model.values = Some(scaler.values.clone()),
                                }
                            }
                            match model
                                .scalers
                                .iter_mut()
                                .find(|(id, _)| *id == scaler.scale_id)
                            {
                                Some((_, all)) => append(all, &scaler.values),
                                None => model.scalers.push((scaler.scale_id, scaler.values)),
                            }
                        }
                    }
                }
            }
            // A channel indexed with zero values still has its type.
            for model in &mut models {
                if model.values.is_none() {
                    if let Some(Remembered::Plain { dtype, .. }) = &model.remembered {
                        model.values = Column::empty(*dtype);
                    }
                }
            }

            let mut toc = 0;
            if segment.metadata {
                toc |= TOC_META;
            }
            if new_list {
                toc |= TOC_NEW_OBJ_LIST;
            }
            if !raw.out.is_empty() {
                toc |= TOC_RAW;
            }
            if segment.interleaved {
                toc |= TOC_INTERLEAVED;
            }
            if big {
                toc |= TOC_BIG_ENDIAN;
            }
            if any_daqmx {
                toc |= TOC_DAQMX | TOC_INTERLEAVED;
            }
            toc ^= segment.toc_xor;
            let raw_offset = meta.out.len() as u64;
            let next = if segment.unfinished {
                u64::MAX
            } else {
                raw_offset + raw.out.len() as u64
            };
            let mut lead = Enc {
                out: Vec::new(),
                big,
            };
            lead.out.extend(toc.to_le_bytes());
            lead.u32(segment.version);
            lead.u64(next);
            lead.u64(raw_offset);

            segment_offsets.push(bytes.len() as u64);
            bytes.extend(b"TDSm");
            bytes.extend(&lead.out);
            bytes.extend(&meta.out);
            data_offsets.push(bytes.len() as u64);
            bytes.extend(&raw.out);
            index_bytes.extend(b"TDSh");
            index_bytes.extend(&lead.out);
            index_bytes.extend(&meta.out);
        }

        let mut channels = Vec::new();
        let mut file_props = Vec::new();
        let mut group_props = Vec::new();
        for model in models {
            match (&model.names, model.path.as_str()) {
                (_, "/") => file_props = model.props,
                (Some((group, None)), _) => group_props.push((group.clone(), model.props)),
                (Some((group, Some(name))), _) => channels.push(ExpectedChannel {
                    group: group.clone(),
                    name: name.clone(),
                    dtype: model.file_type,
                    values: model.values,
                    scalers: model.scalers,
                    props: model.props,
                }),
                (None, _) => {}
            }
        }
        Built {
            bytes,
            index_bytes,
            channels,
            file_props,
            group_props,
            segment_offsets,
            data_offsets,
        }
    }
}

/// Generated values for a channel that the segment gives no values for.
fn generated_payload(model: &Model, chunks: u64) -> Payload {
    match model.remembered.clone().expect("in the list with data") {
        Remembered::Plain { dtype, n, lens } => Payload::Plain(if dtype == DataType::String {
            let all: Vec<usize> = (0..chunks).flat_map(|_| lens.clone()).collect();
            generate_strings(model.seed, model.counter, &all)
        } else {
            generate(dtype, model.seed, model.counter, n * chunks)
        }),
        Remembered::Daqmx {
            digital,
            n,
            scalers,
        } => Payload::Daqmx(
            scalers
                .into_iter()
                .map(|s| {
                    let seed = model.seed ^ u64::from(s.scale_id);
                    let mut values = generate(s.values.dtype(), seed, model.counter, n * chunks);
                    if digital {
                        values = bits_only(&values);
                    }
                    ScalerSpec { values, ..s }
                })
                .collect(),
            digital,
        ),
    }
}

/// The lowest bit of each integer value.
fn bits_only(values: &Values) -> Values {
    match values {
        Values::U8(v) => Values::U8(v.iter().map(|x| x & 1).collect()),
        Values::U16(v) => Values::U16(v.iter().map(|x| x & 1).collect()),
        Values::U32(v) => Values::U32(v.iter().map(|x| x & 1).collect()),
        other => panic!("a digital line cannot be {}", other.dtype()),
    }
}

/// Writes the raw buffers of a driver segment: per chunk, one block of rows
/// per buffer.
fn write_daqmx(raw: &mut Enc, payloads: &[(usize, Payload)], widths: &[u32], chunks: u64) {
    let rows = payloads
        .iter()
        .find_map(|(_, p)| match p {
            Payload::Daqmx(scalers, _) => scalers.first().map(|s| s.values.len() as u64 / chunks),
            Payload::Plain(_) => None,
        })
        .unwrap_or(0) as usize;
    for chunk in 0..chunks as usize {
        for (buffer, &width) in widths.iter().enumerate() {
            let width = width as usize;
            let start = raw.out.len();
            // Filler that a wrong offset would read as nonsense. Digital
            // rows start cleared so that only the lines' bits are set.
            let digital = payloads
                .iter()
                .any(|(_, p)| matches!(p, Payload::Daqmx(_, true)));
            raw.out
                .extend((0..rows * width).map(|i| if digital { 0 } else { 0xA5 ^ (i as u8) }));
            for (_, payload) in payloads {
                let Payload::Daqmx(scalers, digital) = payload else {
                    panic!("a segment cannot mix driver buffers and plain data");
                };
                for scaler in scalers.iter().filter(|s| s.buffer as usize == buffer) {
                    for row in 0..rows {
                        let i = chunk * rows + row;
                        if *digital {
                            let at = start + row * width + scaler.offset as usize / 8;
                            let bit = match &scaler.values {
                                Values::U8(v) => v[i] & 1,
                                Values::U16(v) => (v[i] & 1) as u8,
                                Values::U32(v) => (v[i] & 1) as u8,
                                other => panic!("a digital line cannot be {}", other.dtype()),
                            };
                            raw.out[at] |= bit << (scaler.offset % 8);
                        } else {
                            let mut one = Enc {
                                out: Vec::new(),
                                big: raw.big,
                            };
                            one.value(&scaler.values, i);
                            let at = start + row * width + scaler.offset as usize;
                            raw.out[at..at + one.out.len()].copy_from_slice(&one.out);
                        }
                    }
                }
            }
        }
    }
}
