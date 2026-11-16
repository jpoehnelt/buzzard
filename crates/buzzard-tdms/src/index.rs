//! Pass one: walk every segment's lead-in and metadata, never the raw data,
//! and record where each channel's values are.
//!
//! The result is small for the files data loggers write. Segments that
//! repeat the same layout collapse into one [`Block`], and a layout (which
//! channel sits where in a chunk) is stored once however often it is used,
//! so the work per segment without changed metadata is constant and does
//! not grow with the number of channels.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{Anomaly, Error, Result};
use crate::io::{ReadAt, Window};
use crate::types::{Complex32, Complex64, DataType, Property, PropertyValue, Timestamp};
use crate::OpenOptions;

/// Bytes in a segment's lead-in.
pub(crate) const LEAD_IN: u64 = 28;
const LEAD_IN_LEN: usize = 28;

pub(crate) const TOC_META: u32 = 1 << 1;
pub(crate) const TOC_NEW_OBJ_LIST: u32 = 1 << 2;
pub(crate) const TOC_RAW: u32 = 1 << 3;
pub(crate) const TOC_INTERLEAVED: u32 = 1 << 5;
pub(crate) const TOC_BIG_ENDIAN: u32 = 1 << 6;

const INDEX_NONE: u32 = 0xFFFF_FFFF;
const INDEX_SAME: u32 = 0;
const INDEX_DAQMX_FORMAT: u32 = 0x1269;
const INDEX_DAQMX_DIGITAL: u32 = 0x126A;

const ABSENT: u32 = u32::MAX;

/// How many anomalies of one file are kept.
pub(crate) const MAX_ANOMALIES: usize = 1024;

/// Where the values of one channel (or one scaler) sit inside a chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Loc {
    /// Bytes from the start of the chunk to the first value.
    pub(crate) offset: u64,
    /// Bytes from one value to the next. Equal to the value's size when the
    /// values are contiguous.
    pub(crate) stride: u64,
    /// Bytes from the start of a row to the value, when values are in rows.
    pub(crate) col: u64,
}

/// One scaler of a channel in a driver's raw buffers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Scaler {
    pub(crate) scale_id: u32,
    pub(crate) dtype: DataType,
    pub(crate) loc: Loc,
    /// For a digital line: which bit of the byte at `loc`.
    pub(crate) bit: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Source {
    Fixed(Loc),
    Strings { offset: u64, total: u64 },
    Daqmx(Arc<[Scaler]>),
}

/// One channel's part of a chunk.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Entry {
    pub(crate) obj: u32,
    /// Values per chunk; never zero.
    pub(crate) n: u64,
    pub(crate) source: Source,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    Contiguous,
    Interleaved,
    Daqmx,
}

/// The arrangement of one chunk of raw data.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Layout {
    pub(crate) chunk_size: u64,
    pub(crate) big_endian: bool,
    pub(crate) kind: Kind,
    /// In the order the channels' data appear in the chunk.
    pub(crate) entries: Vec<Entry>,
}

/// `repeat` segments in a row, `stride` bytes apart, each holding `chunks`
/// chunks of the same layout.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Block {
    pub(crate) layout: u32,
    pub(crate) start: u64,
    pub(crate) chunks: u64,
    pub(crate) repeat: u64,
    pub(crate) stride: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ObjKind {
    Root,
    Group(String),
    Channel(String, String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RawScaler {
    dtype: DataType,
    buffer: u32,
    offset: u32,
    scale_id: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RawIndex {
    Plain {
        n: u64,
        total: Option<u64>,
    },
    Daqmx {
        n: u64,
        digital: bool,
        scalers: Vec<RawScaler>,
        widths: Vec<u32>,
    },
}

/// The scalers a channel's most recent raw data index declared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScalerInfo {
    pub(crate) scale_id: u32,
    pub(crate) dtype: DataType,
    pub(crate) digital: bool,
}

#[derive(Debug)]
pub(crate) struct Object {
    pub(crate) path: String,
    pub(crate) kind: ObjKind,
    pub(crate) props: Vec<Property>,
    prop_at: HashMap<String, usize>,
    pub(crate) dtype: Option<DataType>,
    index: Option<RawIndex>,
    /// Total number of values, known after the walk.
    pub(crate) len: u64,
    /// The layouts this object has data in, by ascending layout number,
    /// with the position of its entry.
    pub(crate) layouts: Vec<(u32, u32)>,
    pub(crate) scalers: Vec<ScalerInfo>,
}

/// Which file the metadata was read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexSource {
    /// The data file itself.
    DataFile,
    /// The `.tdms_index` file beside it, after it was checked against the
    /// data file.
    IndexFile,
    /// The data file, because the `.tdms_index` file beside it is stale,
    /// damaged or does not belong to it.
    DataFileIndexRejected,
}

#[derive(Debug)]
pub(crate) struct Index {
    pub(crate) objects: Vec<Object>,
    pub(crate) by_path: HashMap<String, u32>,
    pub(crate) layouts: Vec<Arc<Layout>>,
    pub(crate) blocks: Vec<Block>,
    pub(crate) anomalies: Vec<Anomaly>,
    pub(crate) segments: u64,
    pub(crate) versions: Vec<u32>,
    pub(crate) source: IndexSource,
}

struct Cur<'a> {
    buf: &'a [u8],
    at: usize,
    big: bool,
    base: u64,
    segment: u64,
}

impl<'a> Cur<'a> {
    fn offset(&self) -> u64 {
        self.base + self.at as u64
    }

    fn corrupt(&self, reason: &'static str) -> Error {
        Error::Corrupt {
            offset: self.offset(),
            segment: self.segment,
            reason,
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let bytes = self
            .at
            .checked_add(n)
            .and_then(|end| self.buf.get(self.at..end))
            .ok_or_else(|| self.corrupt("the metadata ends inside a field"))?;
        self.at += n;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        let b = self.array()?;
        Ok(if self.big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.array()?;
        Ok(if self.big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    }

    fn u64(&mut self) -> Result<u64> {
        let b = self.array()?;
        Ok(if self.big {
            u64::from_be_bytes(b)
        } else {
            u64::from_le_bytes(b)
        })
    }

    /// A length-prefixed string. The flag is set when bytes were replaced.
    fn text(&mut self) -> Result<(Cow<'a, str>, bool)> {
        let len = self.u32()?;
        let len = usize::try_from(len).map_err(|_| self.corrupt("a string is too long"))?;
        let bytes = self.take(len)?;
        let text = String::from_utf8_lossy(bytes);
        let lossy = matches!(text, Cow::Owned(_));
        Ok((text, lossy))
    }

    fn timestamp(&mut self) -> Result<Timestamp> {
        if self.big {
            let seconds = i64::from_be_bytes(self.array()?);
            let fractions = u64::from_be_bytes(self.array()?);
            Ok(Timestamp::new(seconds, fractions))
        } else {
            let fractions = u64::from_le_bytes(self.array()?);
            let seconds = i64::from_le_bytes(self.array()?);
            Ok(Timestamp::new(seconds, fractions))
        }
    }

    fn value(&mut self, dtype: DataType, lossy: &mut bool) -> Result<PropertyValue> {
        Ok(match dtype {
            DataType::I8 => PropertyValue::I8(i8::from_le_bytes(self.array()?)),
            DataType::I16 => PropertyValue::I16(self.u16()?.cast_signed()),
            DataType::I32 => PropertyValue::I32(self.u32()?.cast_signed()),
            DataType::I64 => PropertyValue::I64(self.u64()?.cast_signed()),
            DataType::U8 => PropertyValue::U8(self.u8()?),
            DataType::U16 => PropertyValue::U16(self.u16()?),
            DataType::U32 => PropertyValue::U32(self.u32()?),
            DataType::U64 => PropertyValue::U64(self.u64()?),
            DataType::F32 | DataType::F32WithUnit => {
                PropertyValue::F32(f32::from_bits(self.u32()?))
            }
            DataType::F64 | DataType::F64WithUnit => {
                PropertyValue::F64(f64::from_bits(self.u64()?))
            }
            DataType::String => {
                let (text, bad) = self.text()?;
                *lossy |= bad;
                PropertyValue::String(text.into_owned())
            }
            DataType::Bool => PropertyValue::Bool(self.u8()? != 0),
            DataType::Timestamp => PropertyValue::Timestamp(self.timestamp()?),
            DataType::ComplexF32 => PropertyValue::ComplexF32(Complex32 {
                re: f32::from_bits(self.u32()?),
                im: f32::from_bits(self.u32()?),
            }),
            DataType::ComplexF64 => PropertyValue::ComplexF64(Complex64 {
                re: f64::from_bits(self.u64()?),
                im: f64::from_bits(self.u64()?),
            }),
            DataType::Void
            | DataType::ExtendedFloat
            | DataType::ExtendedFloatWithUnit
            | DataType::FixedPoint
            | DataType::DaqmxRaw => {
                return Err(Error::Unsupported {
                    offset: self.offset(),
                    segment: self.segment,
                    what: format!("a property of type {dtype}"),
                })
            }
        })
    }
}

/// Splits `/'group'/'channel'` into its names.
fn parse_path(path: &str) -> Option<ObjKind> {
    if path == "/" {
        return Some(ObjKind::Root);
    }
    let mut names = Vec::with_capacity(2);
    let mut rest = path;
    while !rest.is_empty() {
        rest = rest.strip_prefix("/'")?;
        let mut name = String::new();
        loop {
            let quote = rest.find('\'')?;
            name.push_str(&rest[..quote]);
            rest = &rest[quote + 1..];
            if let Some(after) = rest.strip_prefix('\'') {
                name.push('\'');
                rest = after;
            } else {
                break;
            }
        }
        if names.len() == 2 {
            return None;
        }
        names.push(name);
    }
    let mut names = names.into_iter();
    match (names.next(), names.next()) {
        (Some(group), None) => Some(ObjKind::Group(group)),
        (Some(group), Some(channel)) => Some(ObjKind::Channel(group, channel)),
        _ => None,
    }
}

/// The path of an object as the file spells it.
pub(crate) fn object_path(group: &str, channel: Option<&str>) -> String {
    let quote = |name: &str| format!("/'{}'", name.replace('\'', "''"));
    match channel {
        Some(channel) => quote(group) + &quote(channel),
        None => quote(group),
    }
}

fn daqmx_type(code: u32) -> Option<DataType> {
    Some(match code {
        0 => DataType::U8,
        1 => DataType::I8,
        2 => DataType::U16,
        3 => DataType::I16,
        4 => DataType::U32,
        5 => DataType::I32,
        6 => DataType::U64,
        7 => DataType::I64,
        8 => DataType::F32,
        9 => DataType::F64,
        0xFFFF_FFFF => DataType::Timestamp,
        _ => return None,
    })
}

struct Lead {
    toc: u32,
    version: u32,
    next: u64,
    raw: u64,
}

fn parse_lead(bytes: &[u8]) -> Lead {
    let word = |at: usize| {
        let mut b = [0; 4];
        b.copy_from_slice(&bytes[at..at + 4]);
        b
    };
    let long = |at: usize| {
        let mut b = [0; 8];
        b.copy_from_slice(&bytes[at..at + 8]);
        b
    };
    // The table of contents is little-endian whatever it says.
    let toc = u32::from_le_bytes(word(4));
    if toc & TOC_BIG_ENDIAN != 0 {
        Lead {
            toc,
            version: u32::from_be_bytes(word(8)),
            next: u64::from_be_bytes(long(12)),
            raw: u64::from_be_bytes(long(20)),
        }
    } else {
        Lead {
            toc,
            version: u32::from_le_bytes(word(8)),
            next: u64::from_le_bytes(long(12)),
            raw: u64::from_le_bytes(long(20)),
        }
    }
}

/// One segment, as the walk hands it to the builder.
struct Segment<'a> {
    number: u64,
    /// Offset of the lead-in in the data file.
    pos: u64,
    lead: &'a Lead,
    /// Bytes after the lead-in that are present.
    len: u64,
    /// Whether the segment was cut short or never finished.
    incomplete: bool,
    meta: Option<&'a [u8]>,
    /// Offset of the metadata in the file it was read from.
    meta_base: u64,
}

struct Builder {
    objects: Vec<Object>,
    by_path: HashMap<String, u32>,
    /// The current segment's objects in order, with whether each has data.
    list: Vec<(u32, bool)>,
    /// Position of each object in `list`, or `ABSENT`.
    list_at: Vec<u32>,
    layouts: Vec<Arc<Layout>>,
    layout_ids: HashMap<Arc<Layout>, u32>,
    layout_chunks: Vec<u64>,
    layout_entries: u64,
    max_layout_entries: u64,
    /// Entries visited while building layouts, stored or not.
    layout_work: u64,
    /// Bytes of metadata parsed so far.
    meta_bytes: u64,
    /// The layout of the current list, and the flags it was built for.
    current: Option<(u32, u32)>,
    blocks: Vec<Block>,
    anomalies: Vec<Anomaly>,
    versions: Vec<u32>,
    segments: u64,
}

const LAYOUT_FLAGS: u32 = TOC_INTERLEAVED | TOC_BIG_ENDIAN;

impl Builder {
    fn new(options: &OpenOptions) -> Self {
        Self {
            objects: Vec::new(),
            by_path: HashMap::new(),
            list: Vec::new(),
            list_at: Vec::new(),
            layouts: Vec::new(),
            layout_ids: HashMap::new(),
            layout_chunks: Vec::new(),
            layout_entries: 0,
            max_layout_entries: options.max_layout_entries,
            layout_work: 0,
            meta_bytes: 0,
            current: None,
            blocks: Vec::new(),
            anomalies: Vec::new(),
            versions: Vec::new(),
            segments: 0,
        }
    }

    /// Records an anomaly. A damaged file can have one in every segment,
    /// so only the first [`MAX_ANOMALIES`] are kept.
    fn note(&mut self, anomaly: Anomaly) {
        if self.anomalies.len() < MAX_ANOMALIES {
            self.anomalies.push(anomaly);
        }
    }

    fn object(&mut self, path: &str, cur: &Cur<'_>) -> Result<u32> {
        if let Some(&id) = self.by_path.get(path) {
            return Ok(id);
        }
        let kind = parse_path(path).ok_or_else(|| cur.corrupt("an object path is malformed"))?;
        let id = u32::try_from(self.objects.len())
            .ok()
            .filter(|&id| id != ABSENT)
            .ok_or_else(|| cur.corrupt("too many objects"))?;
        self.objects.push(Object {
            path: path.to_owned(),
            kind,
            props: Vec::new(),
            prop_at: HashMap::new(),
            dtype: None,
            index: None,
            len: 0,
            layouts: Vec::new(),
            scalers: Vec::new(),
        });
        self.list_at.push(ABSENT);
        self.by_path.insert(path.to_owned(), id);
        Ok(id)
    }

    /// Puts the object in the segment's list, or updates its entry.
    fn list_set(&mut self, id: u32, has_data: bool) {
        let at = self.list_at[id as usize];
        if at == ABSENT {
            // The list cannot outgrow the objects, whose count fits.
            self.list_at[id as usize] = u32::try_from(self.list.len()).unwrap_or(ABSENT);
            self.list.push((id, has_data));
            if has_data {
                self.current = None;
            }
        } else if self.list[at as usize].1 != has_data {
            self.list[at as usize].1 = has_data;
            self.current = None;
        }
    }

    fn set_dtype(&mut self, id: u32, dtype: DataType, cur: &Cur<'_>) -> Result<()> {
        let object = &mut self.objects[id as usize];
        match object.dtype {
            Some(old) if old != dtype => Err(cur.corrupt("a channel changes its data type")),
            _ => {
                object.dtype = Some(dtype);
                Ok(())
            }
        }
    }

    fn set_index(&mut self, id: u32, index: RawIndex) {
        let object = &mut self.objects[id as usize];
        if object.index.as_ref() != Some(&index) {
            if let RawIndex::Daqmx {
                scalers, digital, ..
            } = &index
            {
                object.scalers = scalers
                    .iter()
                    .map(|s| ScalerInfo {
                        scale_id: s.scale_id,
                        dtype: s.dtype,
                        digital: *digital,
                    })
                    .collect();
            }
            object.index = Some(index);
            self.current = None;
        }
        self.list_set(id, true);
    }

    fn daqmx_index(&mut self, id: u32, digital: bool, cur: &mut Cur<'_>) -> Result<()> {
        let code = cur.u32()?;
        let dtype =
            DataType::from_code(code).ok_or_else(|| cur.corrupt("unknown data type code"))?;
        if cur.u32()? != 1 {
            return Err(cur.corrupt("the array dimension is not 1"));
        }
        let n = cur.u64()?;
        let count = cur.u32()?;
        let mut scalers = Vec::new();
        for _ in 0..count {
            let code = cur.u32()?;
            let scaler_type =
                daqmx_type(code).ok_or_else(|| cur.corrupt("unknown scaler data type code"))?;
            let buffer = cur.u32()?;
            let offset = cur.u32()?;
            // The sample format bitmap: one byte for a digital line scaler,
            // four for a format changing scaler. Its meaning is not public.
            if digital {
                cur.u8()?;
            } else {
                cur.u32()?;
            }
            let scale_id = cur.u32()?;
            scalers.push(RawScaler {
                dtype: scaler_type,
                buffer,
                offset,
                scale_id,
            });
        }
        let count = cur.u32()?;
        let mut widths = Vec::new();
        for _ in 0..count {
            widths.push(cur.u32()?);
        }
        if dtype != DataType::DaqmxRaw && (scalers.len() != 1 || scalers[0].dtype != dtype) {
            return Err(cur.corrupt("a typed channel needs exactly one scaler of its type"));
        }
        self.set_dtype(id, dtype, cur)?;
        self.set_index(
            id,
            RawIndex::Daqmx {
                n,
                digital,
                scalers,
                widths,
            },
        );
        Ok(())
    }

    fn plain_index(&mut self, id: u32, len: u32, cur: &mut Cur<'_>) -> Result<()> {
        let code = cur.u32()?;
        let dtype =
            DataType::from_code(code).ok_or_else(|| cur.corrupt("unknown data type code"))?;
        if cur.u32()? != 1 {
            return Err(cur.corrupt("the array dimension is not 1"));
        }
        let n = cur.u64()?;
        let total = if dtype == DataType::String {
            Some(cur.u64()?)
        } else {
            None
        };
        if len != if total.is_some() { 28 } else { 20 } {
            return Err(cur.corrupt("the raw data index length disagrees with its data type"));
        }
        if dtype == DataType::DaqmxRaw {
            return Err(cur.corrupt("a plain raw data index has the DAQmx data type"));
        }
        self.set_dtype(id, dtype, cur)?;
        self.set_index(id, RawIndex::Plain { n, total });
        Ok(())
    }

    fn metadata(&mut self, seg: &Segment<'_>, meta: &[u8]) -> Result<()> {
        let mut cur = Cur {
            buf: meta,
            at: 0,
            big: seg.lead.toc & TOC_BIG_ENDIAN != 0,
            base: seg.meta_base,
            segment: seg.number,
        };
        let count = cur.u32()?;
        for _ in 0..count {
            let at = cur.offset();
            let (path, mut lossy) = cur.text()?;
            let id = self.object(&path, &cur)?;
            match cur.u32()? {
                INDEX_NONE => self.list_set(id, false),
                INDEX_SAME => {
                    if self.objects[id as usize].index.is_none() {
                        return Err(
                            cur.corrupt("an object repeats a raw data index that it never had")
                        );
                    }
                    self.list_set(id, true);
                }
                INDEX_DAQMX_FORMAT => self.daqmx_index(id, false, &mut cur)?,
                INDEX_DAQMX_DIGITAL => self.daqmx_index(id, true, &mut cur)?,
                len => self.plain_index(id, len, &mut cur)?,
            }
            let props = cur.u32()?;
            for _ in 0..props {
                let (name, bad_name) = cur.text()?;
                let code = cur.u32()?;
                let dtype = DataType::from_code(code)
                    .ok_or_else(|| cur.corrupt("unknown property type code"))?;
                let mut bad_value = false;
                let value = cur.value(dtype, &mut bad_value)?;
                lossy |= bad_name | bad_value;
                let object = &mut self.objects[id as usize];
                if let Some(&at) = object.prop_at.get(name.as_ref()) {
                    object.props[at].dtype = dtype;
                    object.props[at].value = value;
                } else {
                    object.prop_at.insert(name.to_string(), object.props.len());
                    object.props.push(Property {
                        name: name.into_owned(),
                        dtype,
                        value,
                    });
                }
            }
            if lossy {
                self.note(Anomaly::InvalidUtf8 {
                    segment: seg.number,
                    offset: at,
                });
            }
        }
        Ok(())
    }

    fn intern(&mut self, layout: Layout) -> Result<u32> {
        if let Some(&id) = self.layout_ids.get(&layout) {
            return Ok(id);
        }
        self.layout_entries += layout.entries.len() as u64;
        // A layout that the file spells out in its metadata is paid for by
        // those bytes; only layouts beyond that count against the limit.
        if self.layout_entries > self.max_layout_entries.max(self.meta_bytes / 32) {
            return Err(Error::IndexTooLarge {
                limit: self.max_layout_entries,
            });
        }
        let id = u32::try_from(self.layouts.len()).map_err(|_| Error::IndexTooLarge {
            limit: self.max_layout_entries,
        })?;
        let layout = Arc::new(layout);
        self.layouts.push(Arc::clone(&layout));
        self.layout_ids.insert(layout, id);
        self.layout_chunks.push(0);
        Ok(id)
    }

    /// The layout of a chunk for the current list and flags.
    fn layout(&mut self, seg: &Segment<'_>) -> Result<u32> {
        let flags = seg.lead.toc & LAYOUT_FLAGS;
        if let Some((id, built_for)) = self.current {
            if built_for == flags {
                return Ok(id);
            }
        }
        let at = At {
            offset: seg.pos,
            segment: seg.number,
        };
        // The same budget for time: a file cannot make the reader walk a
        // long object list again and again with a few bytes per segment.
        self.layout_work = self.layout_work.saturating_add(self.list.len() as u64);
        if self.layout_work
            > self
                .max_layout_entries
                .saturating_mul(16)
                .saturating_add(self.meta_bytes)
        {
            return Err(Error::IndexTooLarge {
                limit: self.max_layout_entries,
            });
        }
        let data: Vec<Indexed<'_>> = self
            .list
            .iter()
            .filter(|(_, has_data)| *has_data)
            .filter_map(|&(id, _)| {
                let object = &self.objects[id as usize];
                let dtype = object.dtype.unwrap_or(DataType::Void);
                object.index.as_ref().map(|index| (id, dtype, index))
            })
            .collect();
        let daqmx = data
            .iter()
            .filter(|(_, _, index)| matches!(index, RawIndex::Daqmx { .. }))
            .count();
        let mut entries = Vec::new();
        let (kind, chunk_size) = if daqmx > 0 {
            if daqmx != data.len() {
                return Err(at.corrupt("a segment mixes DAQmx and other raw data"));
            }
            (Kind::Daqmx, daqmx_entries(&data, &mut entries, at)?)
        } else if flags & TOC_INTERLEAVED != 0 && data.len() > 1 {
            (
                Kind::Interleaved,
                interleaved_entries(&data, &mut entries, at)?,
            )
        } else {
            (
                Kind::Contiguous,
                contiguous_entries(&data, &mut entries, at)?,
            )
        };
        let id = self.intern(Layout {
            chunk_size,
            big_endian: flags & TOC_BIG_ENDIAN != 0,
            kind,
            entries,
        })?;
        self.current = Some((id, flags));
        Ok(id)
    }

    fn add_block(&mut self, layout: u32, start: u64, chunks: u64) {
        self.layout_chunks[layout as usize] =
            self.layout_chunks[layout as usize].saturating_add(chunks);
        if let Some(last) = self.blocks.last_mut() {
            if last.layout == layout && last.chunks == chunks && start > last.start {
                if last.repeat == 1 {
                    last.stride = start - last.start;
                    last.repeat = 2;
                    return;
                }
                let expected = last
                    .stride
                    .checked_mul(last.repeat)
                    .and_then(|span| span.checked_add(last.start));
                if expected == Some(start) {
                    last.repeat += 1;
                    return;
                }
            }
        }
        self.blocks.push(Block {
            layout,
            start,
            chunks,
            repeat: 1,
            stride: 0,
        });
    }

    fn segment(&mut self, seg: &Segment<'_>) -> Result<()> {
        self.segments = seg.number + 1;
        let version = seg.lead.version;
        if !self.versions.contains(&version) {
            if version != 4712 && version != 4713 {
                self.note(Anomaly::UnknownVersion {
                    segment: seg.number,
                    version,
                });
            }
            // A hostile file could list a new number in every segment.
            if self.versions.len() < 8 {
                self.versions.push(version);
            }
        }
        if let Some(meta) = seg.meta {
            self.meta_bytes = self.meta_bytes.saturating_add(meta.len() as u64);
            if seg.lead.toc & TOC_NEW_OBJ_LIST != 0 {
                for &(id, _) in &self.list {
                    self.list_at[id as usize] = ABSENT;
                }
                self.list.clear();
                self.current = None;
            }
            self.metadata(seg, meta)?;
        }
        let data_start = seg.pos + LEAD_IN + seg.lead.raw;
        let available = seg.len - seg.lead.raw;
        if available == 0 {
            return Ok(());
        }
        if seg.lead.toc & TOC_RAW == 0 {
            self.note(Anomaly::UnindexedData {
                segment: seg.number,
                offset: data_start,
                bytes: available,
            });
            return Ok(());
        }
        let id = self.layout(seg)?;
        let layout = Arc::clone(&self.layouts[id as usize]);
        if layout.chunk_size == 0 {
            self.note(Anomaly::UnindexedData {
                segment: seg.number,
                offset: data_start,
                bytes: available,
            });
            return Ok(());
        }
        let chunks = available / layout.chunk_size;
        let rest = available % layout.chunk_size;
        if chunks > 0 {
            self.add_block(id, data_start, chunks);
        }
        if rest > 0 {
            let offset = data_start + chunks * layout.chunk_size;
            let partial = partial_layout(&layout, rest, seg.incomplete);
            let recovered = partial.is_some();
            if let Some(partial) = partial {
                let id = self.intern(partial)?;
                self.add_block(id, offset, 1);
            }
            self.note(Anomaly::PartialChunk {
                segment: seg.number,
                offset,
                bytes: rest,
                recovered,
            });
        }
        Ok(())
    }

    fn finish(mut self, source: IndexSource) -> Index {
        for (id, layout) in self.layouts.iter().enumerate() {
            let chunks = self.layout_chunks[id];
            for (at, entry) in layout.entries.iter().enumerate() {
                let object = &mut self.objects[entry.obj as usize];
                object.len = object.len.saturating_add(entry.n.saturating_mul(chunks));
                // Both counts were checked to fit when the layout was made.
                object.layouts.push((
                    u32::try_from(id).unwrap_or(ABSENT),
                    u32::try_from(at).unwrap_or(ABSENT),
                ));
            }
        }
        for object in &mut self.objects {
            object.prop_at = HashMap::new();
        }
        Index {
            objects: self.objects,
            by_path: self.by_path,
            layouts: self.layouts,
            blocks: self.blocks,
            anomalies: self.anomalies,
            segments: self.segments,
            versions: self.versions,
            source,
        }
    }
}

/// A data object of a segment: its number, type and raw data index.
type Indexed<'a> = (u32, DataType, &'a RawIndex);

/// Where a segment is, for error messages.
#[derive(Clone, Copy)]
struct At {
    offset: u64,
    segment: u64,
}

impl At {
    fn corrupt(self, reason: &'static str) -> Error {
        Error::Corrupt {
            offset: self.offset,
            segment: self.segment,
            reason,
        }
    }

    fn overflow(self) -> Error {
        self.corrupt("the size of a chunk overflows")
    }

    fn unsupported(self, dtype: DataType) -> Error {
        Error::Unsupported {
            offset: self.offset,
            segment: self.segment,
            what: format!("channel data of type {dtype}"),
        }
    }
}

/// Lays out a chunk of rows that hold one value of every channel.
fn interleaved_entries(data: &[Indexed<'_>], entries: &mut Vec<Entry>, at: At) -> Result<u64> {
    let mut rows = None;
    let mut width = 0u64;
    let mut cols = Vec::with_capacity(data.len());
    for &(id, dtype, index) in data {
        let RawIndex::Plain { n, .. } = *index else {
            continue;
        };
        if *rows.get_or_insert(n) != n {
            return Err(at.corrupt("interleaved channels differ in length"));
        }
        let size = dtype.size().ok_or_else(|| at.unsupported(dtype))?;
        cols.push((id, width));
        width = width.checked_add(size).ok_or_else(|| at.overflow())?;
    }
    let rows = rows.unwrap_or(0);
    if rows > 0 {
        entries.extend(cols.into_iter().map(|(obj, col)| Entry {
            obj,
            n: rows,
            source: Source::Fixed(Loc {
                offset: col,
                stride: width,
                col,
            }),
        }));
    }
    width.checked_mul(rows).ok_or_else(|| at.overflow())
}

/// Lays out a chunk of one array per channel, one after another.
fn contiguous_entries(data: &[Indexed<'_>], entries: &mut Vec<Entry>, at: At) -> Result<u64> {
    let mut chunk_size = 0u64;
    for &(id, dtype, index) in data {
        let RawIndex::Plain { n, total } = *index else {
            continue;
        };
        if n == 0 && total.unwrap_or(0) == 0 {
            continue;
        }
        let (bytes, source) = if let Some(total) = total {
            if n.checked_mul(4).is_none_or(|table| table > total) {
                return Err(at.corrupt("a string channel is smaller than its offset table"));
            }
            let offset = chunk_size;
            (total, Source::Strings { offset, total })
        } else {
            let size = dtype.size().ok_or_else(|| at.unsupported(dtype))?;
            (
                size.checked_mul(n).ok_or_else(|| at.overflow())?,
                Source::Fixed(Loc {
                    offset: chunk_size,
                    stride: size,
                    col: 0,
                }),
            )
        };
        if n > 0 {
            entries.push(Entry { obj: id, n, source });
        }
        chunk_size = chunk_size.checked_add(bytes).ok_or_else(|| at.overflow())?;
    }
    Ok(chunk_size)
}

/// Lays out a chunk of driver buffers: one block of rows per buffer.
fn daqmx_entries(data: &[Indexed<'_>], entries: &mut Vec<Entry>, at: At) -> Result<u64> {
    let corrupt = |reason| at.corrupt(reason);
    let overflow = || at.overflow();
    let mut rows = None;
    let mut shared: Option<&[u32]> = None;
    for (_, _, index) in data {
        let RawIndex::Daqmx { n, widths, .. } = index else {
            continue;
        };
        if *shared.get_or_insert(widths) != widths.as_slice() {
            return Err(corrupt("DAQmx channels disagree on the raw buffer widths"));
        }
        if *n > 0 && *rows.get_or_insert(*n) != *n {
            return Err(corrupt("DAQmx channels differ in length"));
        }
    }
    let (Some(rows), Some(widths)) = (rows, shared) else {
        return Ok(0);
    };
    let mut starts = Vec::with_capacity(widths.len());
    let mut chunk_size = 0u64;
    for &width in widths {
        starts.push(chunk_size);
        let bytes = u64::from(width).checked_mul(rows).ok_or_else(overflow)?;
        chunk_size = chunk_size.checked_add(bytes).ok_or_else(overflow)?;
    }
    for &(id, _, index) in data {
        let RawIndex::Daqmx {
            n,
            digital,
            scalers,
            ..
        } = index
        else {
            continue;
        };
        if *n == 0 {
            continue;
        }
        let mut placed = Vec::with_capacity(scalers.len());
        for scaler in scalers {
            let buffer = scaler.buffer as usize;
            let (Some(&start), Some(&width)) = (starts.get(buffer), widths.get(buffer)) else {
                return Err(corrupt("a scaler names a raw buffer that does not exist"));
            };
            let width = u64::from(width);
            let size = scaler.dtype.size().unwrap_or(1);
            let (col, size, bit) = if *digital {
                (
                    u64::from(scaler.offset / 8),
                    1,
                    u8::try_from(scaler.offset % 8).ok(),
                )
            } else {
                (u64::from(scaler.offset), size, None)
            };
            if col + size > width {
                return Err(corrupt(
                    "a scaler reaches beyond the width of its raw buffer",
                ));
            }
            placed.push(Scaler {
                scale_id: scaler.scale_id,
                dtype: scaler.dtype,
                loc: Loc {
                    offset: start + col,
                    stride: width,
                    col,
                },
                bit,
            });
        }
        entries.push(Entry {
            obj: id,
            n: rows,
            source: Source::Daqmx(placed.into()),
        });
    }
    Ok(chunk_size)
}

/// What can be read of a chunk of which only `available` bytes exist.
fn partial_layout(full: &Layout, available: u64, incomplete: bool) -> Option<Layout> {
    let mut entries = Vec::new();
    match full.kind {
        Kind::Interleaved => {
            let width = match full.entries.first()?.source {
                Source::Fixed(loc) => loc.stride,
                _ => return None,
            };
            let rows = available / width;
            if rows == 0 {
                return None;
            }
            entries.extend(full.entries.iter().map(|e| Entry {
                obj: e.obj,
                n: rows,
                source: e.source.clone(),
            }));
        }
        Kind::Daqmx => {
            // Rows can be counted only when there is a single raw buffer.
            let width = match &full.entries.first()?.source {
                Source::Daqmx(scalers) => scalers.first()?.loc.stride,
                _ => return None,
            };
            let first = full.entries.first()?;
            if width.checked_mul(first.n) != Some(full.chunk_size) {
                return None;
            }
            let rows = available / width;
            if rows == 0 {
                return None;
            }
            entries.extend(full.entries.iter().map(|e| Entry {
                obj: e.obj,
                n: rows,
                source: e.source.clone(),
            }));
        }
        Kind::Contiguous => {
            if full
                .entries
                .iter()
                .any(|e| matches!(e.source, Source::Strings { .. }))
            {
                return None;
            }
            let mut offset = 0u64;
            for entry in &full.entries {
                let Source::Fixed(loc) = entry.source else {
                    return None;
                };
                let n = if incomplete {
                    // The writer stopped mid-chunk: channels are complete in
                    // order until the bytes run out.
                    entry
                        .n
                        .min(available.saturating_sub(loc.offset) / loc.stride)
                } else {
                    // A finished segment with a ragged tail holds a chunk in
                    // which every channel is shorter in proportion.
                    let scaled =
                        u128::from(entry.n) * u128::from(available) / u128::from(full.chunk_size);
                    u64::try_from(scaled).unwrap_or(0)
                };
                let at = if incomplete { loc.offset } else { offset };
                if !incomplete {
                    offset += n * loc.stride;
                }
                if n > 0 {
                    entries.push(Entry {
                        obj: entry.obj,
                        n,
                        source: Source::Fixed(Loc {
                            offset: at,
                            stride: loc.stride,
                            col: 0,
                        }),
                    });
                }
            }
        }
    }
    if entries.is_empty() {
        return None;
    }
    Some(Layout {
        chunk_size: available,
        big_endian: full.big_endian,
        kind: full.kind,
        entries,
    })
}

fn to_len(value: u64) -> Result<usize> {
    usize::try_from(value).map_err(|_| Error::Alloc { bytes: value })
}

/// Walks the data file itself.
fn walk_data(data: &dyn ReadAt, options: &OpenOptions, source: IndexSource) -> Result<Index> {
    let size = data.size();
    let mut window = Window::new(data, 4096);
    let mut builder = Builder::new(options);
    let mut pos = 0u64;
    let mut number = 0u64;
    if size < 4 {
        return Err(Error::NotTdms);
    }
    while pos < size {
        let left = size - pos;
        let head = window.fetch(pos, LEAD_IN_LEN.min(to_len(left)?))?;
        if left >= 4 && &head[..4] != b"TDSm" {
            return Err(if number == 0 {
                Error::NotTdms
            } else {
                Error::Corrupt {
                    offset: pos,
                    segment: number,
                    reason: "a segment does not start with the segment tag",
                }
            });
        }
        if left < LEAD_IN {
            builder.note(Anomaly::TruncatedLeadIn {
                offset: pos,
                bytes: left,
            });
            break;
        }
        let lead = parse_lead(head);
        let room = left - LEAD_IN;
        let (len, incomplete) = if lead.next == u64::MAX {
            builder.note(Anomaly::UnfinishedSegment {
                segment: number,
                offset: pos,
            });
            (room, true)
        } else if lead.next > room {
            builder.note(Anomaly::TruncatedSegment {
                segment: number,
                offset: pos,
                missing: lead.next - room,
            });
            (room, true)
        } else {
            (lead.next, false)
        };
        if lead.raw > len {
            if incomplete {
                builder.note(Anomaly::TruncatedMetadata {
                    segment: number,
                    offset: pos,
                });
                // The segment still counts as present in the file.
                builder.segments = number + 1;
                break;
            }
            return Err(Error::Corrupt {
                offset: pos + 20,
                segment: number,
                reason: "the metadata is longer than its segment",
            });
        }
        let meta = if lead.toc & TOC_META != 0 && lead.raw > 0 {
            Some(window.fetch(pos + LEAD_IN, to_len(lead.raw)?)?)
        } else {
            None
        };
        builder.segment(&Segment {
            number,
            pos,
            lead: &lead,
            len,
            incomplete,
            meta,
            meta_base: pos + LEAD_IN,
        })?;
        pos += LEAD_IN + len;
        number += 1;
    }
    Ok(builder.finish(source))
}

/// Walks the index file, and checks it against the data file. `None` when
/// the index file cannot be trusted for any reason.
fn walk_index(data: &dyn ReadAt, index: &dyn ReadAt, options: &OpenOptions) -> Option<Index> {
    // How many lead-ins of the data file are compared with the index
    // file's. Each is a read at a far-apart offset, which is what an index
    // file is there to avoid; a stale index file almost always differs in
    // total length or in its first or last segment already.
    const MAX_CHECKS: usize = 64;
    let size = data.size();
    let index_size = index.size();
    let mut window = Window::new(index, 1 << 20);
    let mut builder = Builder::new(options);
    let mut checks: Vec<(u64, [u8; 24])> = Vec::new();
    let mut every = 1u64;
    let mut last = None;
    let (mut pos, mut at, mut number) = (0u64, 0u64, 0u64);
    while at < index_size {
        if index_size - at < LEAD_IN || size.checked_sub(pos)? < LEAD_IN {
            return None;
        }
        let head = window.fetch(at, LEAD_IN_LEN).ok()?;
        if &head[..4] != b"TDSh" {
            return None;
        }
        let mut rest = [0u8; 24];
        rest.copy_from_slice(&head[4..]);
        let lead = parse_lead(head);
        if lead.next > size - pos - LEAD_IN || lead.raw > lead.next {
            return None;
        }
        if lead.raw > index_size - at - LEAD_IN {
            return None;
        }
        if number % every == 0 {
            checks.push((pos, rest));
            if checks.len() > MAX_CHECKS {
                let mut keep = false;
                checks.retain(|_| {
                    keep = !keep;
                    keep
                });
                every *= 2;
            }
        }
        last = Some((pos, rest));
        let meta = if lead.toc & TOC_META != 0 && lead.raw > 0 {
            Some(
                window
                    .fetch(at + LEAD_IN, usize::try_from(lead.raw).ok()?)
                    .ok()?,
            )
        } else {
            None
        };
        builder
            .segment(&Segment {
                number,
                pos,
                lead: &lead,
                len: lead.next,
                incomplete: false,
                meta,
                meta_base: at + LEAD_IN,
            })
            .ok()?;
        at += LEAD_IN + lead.raw;
        pos += LEAD_IN + lead.next;
        number += 1;
    }
    if pos != size {
        return None;
    }
    checks.extend(last);
    let mut head = [0u8; LEAD_IN_LEN];
    for (pos, rest) in checks {
        data.read_exact_at(pos, &mut head).ok()?;
        if &head[..4] != b"TDSm" || head[4..] != rest {
            return None;
        }
    }
    Some(builder.finish(IndexSource::IndexFile))
}

/// Builds the index of a file, from its index file when one is given and
/// agrees with the data file.
pub(crate) fn build(
    data: &dyn ReadAt,
    index_file: Option<&dyn ReadAt>,
    options: &OpenOptions,
) -> Result<Index> {
    let mut source = IndexSource::DataFile;
    if let Some(index_file) = index_file {
        if let Some(index) = walk_index(data, index_file, options) {
            return check(index, options);
        }
        source = IndexSource::DataFileIndexRejected;
    }
    check(walk_data(data, options, source)?, options)
}

fn check(index: Index, options: &OpenOptions) -> Result<Index> {
    if options.strict {
        if let Some(anomaly) = index.anomalies.first() {
            return Err(Error::Anomaly(anomaly.clone()));
        }
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_parse() {
        assert_eq!(parse_path("/"), Some(ObjKind::Root));
        assert_eq!(parse_path("/'g'"), Some(ObjKind::Group("g".into())));
        assert_eq!(
            parse_path("/'Dr. T''s Events'/'Ti/me'"),
            Some(ObjKind::Channel("Dr. T's Events".into(), "Ti/me".into()))
        );
        assert_eq!(parse_path("/''"), Some(ObjKind::Group(String::new())));
        for bad in ["", "g", "/g", "/'g", "/'g'x", "/'a'/'b'/'c'", "/'a'/"] {
            assert_eq!(parse_path(bad), None, "{bad}");
        }
        assert_eq!(object_path("a'b", Some("c")), "/'a''b'/'c'");
    }
}
