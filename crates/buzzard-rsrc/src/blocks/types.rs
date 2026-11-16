//! Data types: the consolidated type table (`VCTP`) and the slice of it
//! that heap objects index into (`DTHP`).
//!
//! `VCTP` is zlib-compressed. Inflated, it is a `u32` count of flat type
//! descriptors followed by a list of "top-level" indices into that flat
//! list. Each descriptor is `len, flags, code, body`; containers such as
//! clusters refer to their elements by flat index. A heap object's
//! `typeDesc` value is 1-based within the `DTHP` slice of the top-level list.

use serde::Serialize;

use crate::error::{malformed, Result};
use crate::reader::{decode_text, Reader};

const CODE_INT32: u8 = 0x03;
const CODE_ENUM_U8: u8 = 0x15;
const CODE_ENUM_U32: u8 = 0x17;
const CODE_BOOLEAN: u8 = 0x21;
const CODE_STRING: u8 = 0x30;
const CODE_PATH: u8 = 0x32;
const CODE_ARRAY: u8 = 0x40;
const CODE_CLUSTER: u8 = 0x50;
const CODE_VARIANT: u8 = 0x53;
const CODE_REFNUM: u8 = 0x70;
const CODE_FUNCTION: u8 = 0xF0;
const CODE_TYPEDEF: u8 = 0xF1;

/// Nested type definitions deeper than this are treated as opaque.
const MAX_TYPEDEF_DEPTH: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Body {
    Cluster(Vec<u32>),
    Refnum(u16),
    /// An enumeration: the item names, in value order.
    Enum(Vec<String>),
    /// A named type definition wrapping another descriptor. The names are
    /// the qualified name, outermost first; the last is the `.ctl` file.
    TypeDef(Vec<String>, Box<TypeDesc>),
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TypeDesc {
    code: u8,
    body: Body,
}

/// What a reference refers to. Only kinds that hold an operating-system or
/// runtime resource are named; the rest are grouped as `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    DataLog,
    File,
    Device,
    Occurrence,
    Tcp,
    Automation,
    ViServer,
    DataSocket,
    Visa,
    Ivi,
    Udp,
    Notifier,
    Queue,
    Irda,
    UserDefined,
    EventRegistration,
    DotNet,
    UserEvent,
    Bluetooth,
    DataValue,
    Fifo,
    Tdms,
    Other,
}

impl RefKind {
    fn from_code(code: u16) -> Self {
        match code {
            1 => RefKind::DataLog,
            2 => RefKind::File,
            3 => RefKind::Device,
            4 => RefKind::Occurrence,
            5 => RefKind::Tcp,
            7 => RefKind::Automation,
            8 => RefKind::ViServer,
            13 => RefKind::DataSocket,
            14 => RefKind::Visa,
            15 => RefKind::Ivi,
            16 => RefKind::Udp,
            17 => RefKind::Notifier,
            18 => RefKind::Queue,
            19 => RefKind::Irda,
            20 | 21 | 29 => RefKind::UserDefined,
            23 => RefKind::EventRegistration,
            24 => RefKind::DotNet,
            25 => RefKind::UserEvent,
            31 => RefKind::Bluetooth,
            32 => RefKind::DataValue,
            33 => RefKind::Fifo,
            34 => RefKind::Tdms,
            _ => RefKind::Other,
        }
    }
}

/// Coarse classification of a terminal's data type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataType {
    /// The standard error cluster: Boolean status, 32-bit code, string source.
    ErrorCluster,
    Reference(RefKind),
    Boolean,
    Numeric,
    String,
    Path,
    Array,
    Cluster,
    Variant,
    /// A VI's connector pane signature.
    Function,
    Void,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TypeTable {
    flat: Vec<TypeDesc>,
    top: Vec<u32>,
    /// `(count, shift)` from `DTHP`: heap type ids `1..=count` map to
    /// top-level indices `shift..shift + count`.
    heap_slice: (u32, u32),
}

impl TypeTable {
    /// Parse an inflated `VCTP` block and the raw `DTHP` block, if present.
    pub fn parse(vctp: &[u8], dthp: Option<&[u8]>) -> Result<Self> {
        let mut r = Reader::new(vctp, "VCTP");
        let count = r.u32()? as usize;
        if count > r.remaining() / 4 {
            return Err(malformed("VCTP", format!("implausible type count {count}")));
        }
        let mut flat = Vec::with_capacity(count);
        for _ in 0..count {
            flat.push(parse_desc(&mut r, 0)?);
        }
        let top_count = var_u2p2(&mut r)? as usize;
        if top_count > r.remaining() / 2 {
            return Err(malformed(
                "VCTP",
                format!("implausible top-level count {top_count}"),
            ));
        }
        let top = (0..top_count)
            .map(|_| var_u2p2(&mut r))
            .collect::<Result<Vec<u32>>>()?;

        let mut heap_slice = (0, 0);
        if let Some(dthp) = dthp {
            let mut r = Reader::new(dthp, "DTHP");
            let count = var_u2p2(&mut r)?;
            if count > 0 {
                heap_slice = (count, var_u2p2(&mut r)?);
            }
        }
        Ok(Self {
            flat,
            top,
            heap_slice,
        })
    }

    pub fn len(&self) -> usize {
        self.flat.len()
    }

    pub fn is_empty(&self) -> bool {
        self.flat.is_empty()
    }

    /// Flat index for a heap object's `typeDesc` value.
    fn heap_index(&self, heap_type_id: u32) -> Option<usize> {
        let (count, shift) = self.heap_slice;
        if heap_type_id < 1 || heap_type_id > count {
            return None;
        }
        // Top-level indices are 1-based.
        let top_index = shift.checked_add(heap_type_id)?.checked_sub(2)?;
        Some(*self.top.get(top_index as usize)? as usize)
    }

    /// Classify the type a heap object's `typeDesc` value refers to.
    pub fn heap_type(&self, heap_type_id: u32) -> Option<DataType> {
        self.flat
            .get(self.heap_index(heap_type_id)?)
            .map(|desc| self.classify(desc))
    }

    fn classify(&self, desc: &TypeDesc) -> DataType {
        let mut desc = desc;
        for _ in 0..MAX_TYPEDEF_DEPTH {
            match &desc.body {
                Body::TypeDef(_, inner) => desc = inner,
                _ => break,
            }
        }
        match (&desc.body, desc.code) {
            (Body::Cluster(elements), _) => {
                let codes: Vec<Option<u8>> = elements
                    .iter()
                    .map(|&i| self.flat.get(i as usize).map(|d| self.unwrap_code(d)))
                    .collect();
                if codes == [Some(CODE_BOOLEAN), Some(CODE_INT32), Some(CODE_STRING)] {
                    DataType::ErrorCluster
                } else {
                    DataType::Cluster
                }
            }
            (Body::Refnum(kind), _) => DataType::Reference(RefKind::from_code(*kind)),
            (_, 0x00) => DataType::Void,
            (_, 0x01..=0x1E) | (_, 0x5E) | (_, 0x5F) => DataType::Numeric,
            (_, 0x20) | (_, CODE_BOOLEAN) => DataType::Boolean,
            (_, CODE_STRING) | (_, 0x31) => DataType::String,
            (_, CODE_PATH) => DataType::Path,
            (_, CODE_ARRAY) => DataType::Array,
            (_, CODE_VARIANT) => DataType::Variant,
            (_, CODE_FUNCTION) => DataType::Function,
            _ => DataType::Other,
        }
    }

    /// Type code after looking through type definitions.
    fn unwrap_code(&self, desc: &TypeDesc) -> u8 {
        unwrap(desc).code
    }

    /// Everything known about the type a heap object's `typeDesc` value
    /// refers to.
    pub fn heap_type_info(&self, heap_type_id: u32) -> Option<TypeInfo> {
        let desc = self.flat.get(self.heap_index(heap_type_id)?)?;
        let base = unwrap(desc);
        Some(TypeInfo {
            data_type: self.classify(desc),
            numeric: NumericType::from_code(base.code),
            enum_items: match &base.body {
                Body::Enum(items) => Some(items.clone()),
                _ => None,
            },
            type_definition: match &desc.body {
                Body::TypeDef(names, _) => names.last().cloned(),
                _ => None,
            },
        })
    }

    /// Decode the stored value of a constant of the given heap type.
    ///
    /// The value is the flattened form: big-endian and exactly as long as
    /// the type, except that a value whose bytes are all zero carries one
    /// extra zero byte (checked on 5,629 constants saved by versions 13 to
    /// 26). A string is a `u32` length and that many bytes. Returns `None`
    /// for any other type, for a length that fits neither form, and for an
    /// unsigned 64-bit value above `i64::MAX`.
    pub fn constant(&self, heap_type_id: u32, bytes: &[u8]) -> Option<Value> {
        let base = unwrap(self.flat.get(self.heap_index(heap_type_id)?)?);
        if base.code == CODE_STRING {
            return string_constant(bytes);
        }
        let code = base.code;
        let size = match code {
            CODE_BOOLEAN => 1,
            0x09 => 4,
            0x0A => 8,
            _ => NumericType::from_code(code)?.size()?,
        };
        let exact = bytes.len() == size;
        let zero_padded = bytes.len() == size + 1 && bytes.iter().all(|&b| b == 0);
        if !exact && !zero_padded {
            return None;
        }
        let unsigned = bytes[..size]
            .iter()
            .fold(0u64, |acc, &b| (acc << 8) | u64::from(b));
        Some(match code {
            CODE_BOOLEAN => Value::Boolean(unsigned != 0),
            0x09 => Value::Float(f32::from_bits(unsigned as u32).to_string()),
            0x0A => Value::Float(f64::from_bits(unsigned).to_string()),
            _ => match NumericType::from_code(code)? {
                NumericType::Enum8 | NumericType::Enum16 | NumericType::Enum32 => {
                    let value = u32::try_from(unsigned).ok()?;
                    let name = match &base.body {
                        Body::Enum(items) => items.get(value as usize).cloned(),
                        _ => None,
                    };
                    Value::Enum { value, name }
                }
                NumericType::I8 => Value::Integer(i64::from(unsigned as u8 as i8)),
                NumericType::I16 => Value::Integer(i64::from(unsigned as u16 as i16)),
                NumericType::I32 => Value::Integer(i64::from(unsigned as u32 as i32)),
                NumericType::I64 => Value::Integer(unsigned as i64),
                NumericType::U8 | NumericType::U16 | NumericType::U32 | NumericType::U64 => {
                    Value::Integer(i64::try_from(unsigned).ok()?)
                }
                NumericType::Float | NumericType::Other => return None,
            },
        })
    }
}

/// The descriptor behind any type definitions.
fn unwrap(desc: &TypeDesc) -> &TypeDesc {
    let mut desc = desc;
    for _ in 0..MAX_TYPEDEF_DEPTH {
        match &desc.body {
            Body::TypeDef(_, inner) => desc = inner,
            _ => break,
        }
    }
    desc
}

fn string_constant(bytes: &[u8]) -> Option<Value> {
    let mut r = Reader::new(bytes, "constant");
    let len = r.u32().ok()? as usize;
    let text = r.take(len).ok()?;
    // The empty string is all zeros, so it carries the extra byte.
    let rest = r.remaining();
    let surplus_ok = rest == 0 || (rest == 1 && bytes.iter().all(|&b| b == 0));
    surplus_ok.then(|| Value::String(decode_text(text)))
}

/// How a number is stored. Looks through type definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NumericType {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    /// An enumeration stored in an unsigned 8-bit integer.
    Enum8,
    /// An enumeration stored in an unsigned 16-bit integer.
    Enum16,
    /// An enumeration stored in an unsigned 32-bit integer.
    Enum32,
    /// A floating-point or complex number of any width.
    Float,
    /// Any other numeric type, such as one with a unit or a fixed-point
    /// number.
    Other,
}

impl NumericType {
    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0x01 => Self::I8,
            0x02 => Self::I16,
            0x03 => Self::I32,
            0x04 => Self::I64,
            0x05 => Self::U8,
            0x06 => Self::U16,
            0x07 => Self::U32,
            0x08 => Self::U64,
            0x09..=0x0E => Self::Float,
            0x15 => Self::Enum8,
            0x16 => Self::Enum16,
            0x17 => Self::Enum32,
            0x0F..=0x14 | 0x18..=0x1E | 0x5E | 0x5F => Self::Other,
            _ => return None,
        })
    }

    /// Size in bytes of an integer or enumeration value.
    fn size(self) -> Option<usize> {
        Some(match self {
            Self::I8 | Self::U8 | Self::Enum8 => 1,
            Self::I16 | Self::U16 | Self::Enum16 => 2,
            Self::I32 | Self::U32 | Self::Enum32 => 4,
            Self::I64 | Self::U64 => 8,
            Self::Float | Self::Other => return None,
        })
    }

    /// Whether the type is an enumeration.
    pub fn is_enum(self) -> bool {
        matches!(self, Self::Enum8 | Self::Enum16 | Self::Enum32)
    }

    /// Whether the type is a plain signed or unsigned integer.
    pub fn is_integer(self) -> bool {
        self.size().is_some() && !self.is_enum()
    }

    /// Whether the type cannot hold a negative value.
    pub fn is_unsigned(self) -> bool {
        self.is_enum() || matches!(self, Self::U8 | Self::U16 | Self::U32 | Self::U64)
    }
}

/// The value of a block-diagram constant of a simple type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Value {
    Boolean(bool),
    /// A signed or unsigned integer of any width.
    Integer(i64),
    /// An enumeration value: the 0-based index into the item list and the
    /// item's name. The name is `None` when the value is outside the list.
    ///
    /// The item list is the copy the file stored when it was saved; if the
    /// type definition changed afterwards and the file was not saved
    /// again, the name is the old one.
    Enum {
        value: u32,
        name: Option<String>,
    },
    /// A single or double precision number, printed in the shortest form
    /// that reads back to the same number.
    Float(String),
    /// A string. Bytes that are not UTF-8 are read as Latin-1.
    String(String),
}

impl std::fmt::Display for Value {
    /// The value as a case label or a constant on the diagram shows it:
    /// `True`, `42`, `"text"`, `"Item"`. An enumeration value without a
    /// name prints as its number.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Boolean(true) => f.write_str("True"),
            Value::Boolean(false) => f.write_str("False"),
            Value::Integer(value) => write!(f, "{value}"),
            Value::Enum {
                name: Some(name), ..
            } => write!(f, "\"{}\"", printable(name)),
            Value::Enum { value, name: None } => write!(f, "{value}"),
            Value::Float(text) => f.write_str(text),
            Value::String(text) => write!(f, "\"{}\"", printable(text)),
        }
    }
}

/// Text on one line: control characters, quotes and backslashes escaped.
pub(crate) fn printable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\x{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// What the type table says about one type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeInfo {
    pub data_type: DataType,
    /// Storage of a numeric type; `None` for anything else.
    pub numeric: Option<NumericType>,
    /// Item names of an enumeration, in value order.
    pub enum_items: Option<Vec<String>>,
    /// File name of the type definition the type is an instance of, e.g.
    /// `State.ctl`.
    pub type_definition: Option<String>,
}

/// Longest list of enumeration items accepted.
const MAX_ENUM_ITEMS: usize = 65_536;

/// `u16 count`, then that many Pascal strings.
fn parse_enum(body: &mut Reader<'_>) -> Result<Body> {
    let count = body.u16()? as usize;
    if count > body.remaining() || count > MAX_ENUM_ITEMS {
        return Err(malformed("VCTP", "implausible enumeration size"));
    }
    Ok(Body::Enum(
        (0..count)
            .map(|_| body.pstr().map(decode_text))
            .collect::<Result<Vec<String>>>()?,
    ))
}

/// 16-bit value, or 31 bits when the top bit of the first word is set.
fn var_u2p2(r: &mut Reader<'_>) -> Result<u32> {
    let first = u32::from(r.u16()?);
    Ok(if first & 0x8000 != 0 {
        ((first & 0x7FFF) << 16) | u32::from(r.u16()?)
    } else {
        first
    })
}

fn parse_desc(r: &mut Reader<'_>, depth: usize) -> Result<TypeDesc> {
    let start = r.pos();
    let len = var_u2p2(r)? as usize;
    let header = r.pos() - start;
    if len < header + 2 {
        return Err(malformed(
            "VCTP",
            format!("type descriptor length {len} is too small"),
        ));
    }
    let mut body = Reader::new(r.take(len - header)?, "VCTP");
    parse_body(&mut body, depth)
}

/// The descriptor a type definition wraps. It is stored inline and is the
/// last thing in the type definition's body.
///
/// Its declared length is 4 bytes larger than the bytes it occupies: in
/// 8,399 of 8,399 type definitions in 1,750 real files (versions 13 to 25)
/// the declared length is exactly the rest of the parent body plus 4. The
/// length is therefore capped at what the parent has left, which also
/// accepts a nested descriptor that declares its true length.
fn parse_nested_desc(parent: &mut Reader<'_>, depth: usize) -> Result<TypeDesc> {
    let start = parent.pos();
    let len = var_u2p2(parent)? as usize;
    let header = parent.pos() - start;
    if len < header + 2 {
        return Err(malformed(
            "VCTP",
            format!("nested type descriptor length {len} is too small"),
        ));
    }
    let take = (len - header).min(parent.remaining());
    let mut body = Reader::new(parent.take(take)?, "VCTP");
    parse_body(&mut body, depth)
}

/// Parse `flags, code, body` of a descriptor whose length is consumed.
fn parse_body(body: &mut Reader<'_>, depth: usize) -> Result<TypeDesc> {
    let _flags = body.u8()?;
    let code = body.u8()?;
    // A descriptor whose body does not parse is kept as opaque; its length
    // is known, so the table stays in step.
    let parsed = match code {
        CODE_CLUSTER => parse_cluster(body).unwrap_or(Body::Other),
        CODE_REFNUM => body.u16().map(Body::Refnum).unwrap_or(Body::Other),
        CODE_ENUM_U8..=CODE_ENUM_U32 => parse_enum(body).unwrap_or(Body::Other),
        CODE_TYPEDEF if depth < MAX_TYPEDEF_DEPTH => {
            parse_typedef(body, depth).unwrap_or(Body::Other)
        }
        _ => Body::Other,
    };
    Ok(TypeDesc { code, body: parsed })
}

fn parse_cluster(body: &mut Reader<'_>) -> Result<Body> {
    let count = var_u2p2(body)? as usize;
    if count > body.remaining() / 2 {
        return Err(malformed("VCTP", "implausible cluster size"));
    }
    Ok(Body::Cluster(
        (0..count)
            .map(|_| var_u2p2(body))
            .collect::<Result<Vec<u32>>>()?,
    ))
}

fn parse_typedef(body: &mut Reader<'_>, depth: usize) -> Result<Body> {
    body.u32()?;
    let names = body.u32()? as usize;
    if names > body.remaining() {
        return Err(malformed("VCTP", "implausible type definition name"));
    }
    let names = (0..names)
        .map(|_| body.pstr().map(decode_text))
        .collect::<Result<Vec<String>>>()?;
    Ok(Body::TypeDef(
        names,
        Box::new(parse_nested_desc(body, depth + 1)?),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(code: u8, body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 4) as u16).to_be_bytes().to_vec();
        out.extend([0, code]);
        out.extend_from_slice(body);
        out
    }

    fn table(descs: &[Vec<u8>], top: &[u16]) -> Vec<u8> {
        let mut out = (descs.len() as u32).to_be_bytes().to_vec();
        descs.iter().for_each(|d| out.extend(d));
        out.extend((top.len() as u16).to_be_bytes());
        top.iter().for_each(|t| out.extend(t.to_be_bytes()));
        out
    }

    fn sample() -> TypeTable {
        let typedef_body = {
            let mut b = 0u32.to_be_bytes().to_vec();
            b.extend(1u32.to_be_bytes());
            b.extend(b"\x07err.ctl");
            b.extend(desc(CODE_CLUSTER, &[0, 3, 0, 0, 0, 1, 0, 2]));
            b
        };
        let vctp = table(
            &[
                desc(CODE_BOOLEAN, &[]),                       // 0
                desc(CODE_INT32, &[]),                         // 1
                desc(CODE_STRING, &[0xff, 0xff, 0xff, 0xff]),  // 2
                desc(CODE_CLUSTER, &[0, 3, 0, 0, 0, 1, 0, 2]), // 3 error cluster
                desc(CODE_CLUSTER, &[0, 2, 0, 0, 0, 1]),       // 4 other cluster
                desc(CODE_REFNUM, &[0, 18, 0, 0]),             // 5 queue
                desc(CODE_TYPEDEF, &typedef_body),             // 6 typedef'd error cluster
                desc(0x0A, &[]),                               // 7 double
            ],
            // Top-level list; the heap uses the last five entries.
            &[0, 1, 3, 4, 5, 6, 7],
        );
        // Five heap types starting at top-level index 3 (1-based).
        TypeTable::parse(&vctp, Some(&[0, 5, 0, 3])).unwrap()
    }

    #[test]
    fn resolves_heap_type_ids_through_the_slice() {
        let t = sample();
        assert_eq!(t.len(), 8);
        assert_eq!(t.heap_type(1), Some(DataType::ErrorCluster));
        assert_eq!(t.heap_type(2), Some(DataType::Cluster));
        assert_eq!(t.heap_type(3), Some(DataType::Reference(RefKind::Queue)));
        assert_eq!(t.heap_type(4), Some(DataType::ErrorCluster));
        assert_eq!(t.heap_type(5), Some(DataType::Numeric));
        assert_eq!(t.heap_type(0), None);
        assert_eq!(t.heap_type(6), None);
    }

    fn hex(text: &str) -> Vec<u8> {
        let digits: Vec<u8> = text.bytes().filter(u8::is_ascii_hexdigit).collect();
        digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    /// A table of raw descriptors, each used once by the heap, in order.
    fn raw_table(descs: &[Vec<u8>]) -> TypeTable {
        let top: Vec<u16> = (0..descs.len() as u16).collect();
        let vctp = table(descs, &top);
        let dthp = [(descs.len() as u16).to_be_bytes(), 1u16.to_be_bytes()].concat();
        TypeTable::parse(&vctp, Some(&dthp)).unwrap()
    }

    /// Descriptors copied byte for byte from files saved by the editor. A
    /// descriptor nested in a type definition declares a length 4 bytes
    /// larger than it occupies; before that was handled every type
    /// definition classified as `Other`.
    #[test]
    fn real_type_definitions_are_looked_through() {
        // `State.ctl`, an enum of three items, as saved by version 13: the
        // nested descriptor declares 0x29 = 41 bytes and occupies 37.
        let state_enum = hex("003b00f1 00000000 00000001 0953746174652e63746c \
             0029 4016 0003 0741637175697265 07416e616c797a65 05416c61726d 00 06537461746573 00");
        // The same type under another label, declaring 0x2d and occupying 41.
        let next_state = hex(
            "003f00f1 00000000 00000001 0953746174652e63746c \
             002d 4016 0003 0741637175697265 07416e616c797a65 05416c61726d 00 0a4e657874205374617465 00",
        );
        // `Stored Packet -- cluster.ctl` in a class in a library, as saved
        // by version 13: a cluster of flat types 5, 6 and 7, declaring
        // 0x1e = 30 bytes and occupying 26.
        let stored_packet = hex("006b00f1 00000000 00000003 \
             114d515454205365727665722e6c766c6962 \
             155f537562736372697074696f6e2e6c76636c617373 \
             1c53746f726564205061636b6574202d2d20636c75737465722e63746c \
             001e 4050 0003 0005 0006 0007 0d53746f726564205061636b6574");
        let t = raw_table(&[state_enum, next_state, stored_packet]);
        assert_eq!(t.heap_type(1), Some(DataType::Numeric));
        assert_eq!(t.heap_type(2), Some(DataType::Numeric));
        assert_eq!(t.heap_type(3), Some(DataType::Cluster));
    }

    #[test]
    fn real_typedef_of_an_error_cluster_is_an_error_cluster() {
        // The three element types and the plain cluster are copied from a
        // version 13 file; the type definition wraps the same cluster body
        // with the editor's length quirk (declared 0x1a, occupying 22).
        let status = hex("000c 4021 06737461747573 00");
        let code = hex("000b 4003 00 04636f6465 00");
        let source = hex("0010 4030 ffffffff 06736f75726365 00");
        let plain = hex("0016 4050 0003 0000 0001 0002 096572726f72206f7574");
        let wrapped = hex("002a 00f1 00000000 00000001 076572722e63746c \
             001a 4050 0003 0000 0001 0002 096572726f72206f7574");
        let t = raw_table(&[status, code, source, plain, wrapped]);
        assert_eq!(t.heap_type(1), Some(DataType::Boolean));
        assert_eq!(t.heap_type(4), Some(DataType::ErrorCluster));
        assert_eq!(t.heap_type(5), Some(DataType::ErrorCluster));
    }

    /// Enumerations copied from files saved by versions 13, 25 and 17: a
    /// plain one, one whose item list needs a pad byte, and one with a
    /// label after the items.
    #[test]
    fn real_enumerations_list_their_items() {
        let plain = hex("001d 0016 0003 0741637175697265 07416e616c797a65 05416c61726d 00");
        let padded = hex("002f 0016 0005 055374617274 0950726f636573732031 \
             0950726f636573732032 03456e64 0853687574646f776e 00 00");
        let labelled = hex("002b 4017 0003 09686973746f6772616d 047065616b \
             0b6175746f2073656c656374 00 00 066d6574686f64 00");
        let typedef = hex("003b00f1 00000000 00000001 0953746174652e63746c \
             0029 4016 0003 0741637175697265 07416e616c797a65 05416c61726d 00 06537461746573 00");
        let t = raw_table(&[plain, padded, labelled, typedef, desc(CODE_INT32, &[])]);

        let info = t.heap_type_info(1).unwrap();
        assert_eq!(info.data_type, DataType::Numeric);
        assert_eq!(info.numeric, Some(NumericType::Enum16));
        assert_eq!(
            info.enum_items.as_deref(),
            Some(
                &[
                    "Acquire".to_owned(),
                    "Analyze".to_owned(),
                    "Alarm".to_owned()
                ][..]
            )
        );
        assert_eq!(info.type_definition, None);

        let items = |id| t.heap_type_info(id).unwrap().enum_items.unwrap();
        assert_eq!(
            items(2),
            ["Start", "Process 1", "Process 2", "End", "Shutdown"]
        );
        assert_eq!(items(3), ["histogram", "peak", "auto select"]);
        assert_eq!(
            t.heap_type_info(3).unwrap().numeric,
            Some(NumericType::Enum32)
        );

        let wrapped = t.heap_type_info(4).unwrap();
        assert_eq!(wrapped.enum_items, info.enum_items);
        assert_eq!(wrapped.numeric, Some(NumericType::Enum16));
        assert_eq!(wrapped.type_definition.as_deref(), Some("State.ctl"));

        let plain_number = t.heap_type_info(5).unwrap();
        assert_eq!(plain_number.numeric, Some(NumericType::I32));
        assert_eq!(plain_number.enum_items, None);
        assert_eq!(t.heap_type_info(0), None);
        assert_eq!(t.heap_type_info(6), None);
    }

    #[test]
    fn enumeration_item_lists_are_bounded() {
        // A count far beyond the body, an item running past the end, and
        // no count at all: the type stays numeric without items.
        for body in ["ffff 0141", "0002 0141 09", ""] {
            let t = raw_table(&[desc(0x16, &hex(body))]);
            let info = t.heap_type_info(1).unwrap();
            assert_eq!(info.numeric, Some(NumericType::Enum16));
            assert_eq!(info.enum_items, None);
            assert_eq!(
                t.constant(1, &[0, 1]),
                Some(Value::Enum {
                    value: 1,
                    name: None
                })
            );
        }
    }

    /// The stored forms are the ones found in real files: exact size, or
    /// one extra zero byte when the value is all zeros.
    #[test]
    fn constants_decode_by_type() {
        let state = hex("001d 0016 0003 0741637175697265 07416e616c797a65 05416c61726d 00");
        let t = raw_table(&[
            desc(CODE_BOOLEAN, &[]),                      // 1
            desc(CODE_INT32, &[]),                        // 2
            state,                                        // 3
            desc(0x0A, &[]),                              // 4 double
            desc(CODE_STRING, &[0xff, 0xff, 0xff, 0xff]), // 5
            desc(0x08, &[]),                              // 6 unsigned 64-bit
            desc(0x01, &[]),                              // 7 signed 8-bit
            desc(CODE_CLUSTER, &[0, 0]),                  // 8
            desc(0x09, &[]),                              // 9 single
            desc(0x07, &[]),                              // 10 unsigned 32-bit
        ]);
        let value = |id, bytes: &str| t.constant(id, &hex(bytes));

        assert_eq!(value(1, "01"), Some(Value::Boolean(true)));
        assert_eq!(value(1, "0000"), Some(Value::Boolean(false)));
        assert_eq!(value(1, "00"), Some(Value::Boolean(false)));
        assert_eq!(value(1, "0100"), None);
        assert_eq!(value(1, ""), None);

        assert_eq!(value(2, "0000002a"), Some(Value::Integer(42)));
        assert_eq!(value(2, "ffffffff"), Some(Value::Integer(-1)));
        assert_eq!(value(2, "0000000000"), Some(Value::Integer(0)));
        assert_eq!(value(2, "0000002a00"), None);
        assert_eq!(value(2, "002a"), None);
        assert_eq!(value(10, "ffffffff"), Some(Value::Integer(0xFFFF_FFFF)));
        assert_eq!(value(7, "80"), Some(Value::Integer(-128)));

        let item = |value, name: &str| {
            Some(Value::Enum {
                value,
                name: Some(name.to_owned()),
            })
        };
        assert_eq!(value(3, "0002"), item(2, "Alarm"));
        assert_eq!(value(3, "000000"), item(0, "Acquire"));
        assert_eq!(
            value(3, "0003"),
            Some(Value::Enum {
                value: 3,
                name: None
            })
        );

        assert_eq!(
            value(4, "402e000000000000"),
            Some(Value::Float("15".to_owned()))
        );
        assert_eq!(value(9, "3fc00000"), Some(Value::Float("1.5".to_owned())));
        assert_eq!(
            value(5, "0000000473706563"),
            Some(Value::String("spec".to_owned()))
        );
        assert_eq!(value(5, "0000000000"), Some(Value::String(String::new())));
        assert_eq!(value(5, "00000000"), Some(Value::String(String::new())));
        // A length beyond the bytes, and bytes beyond the length.
        assert_eq!(value(5, "00000009737065"), None);
        assert_eq!(value(5, "000000017300"), None);

        assert_eq!(value(6, "7fffffffffffffff"), Some(Value::Integer(i64::MAX)));
        assert_eq!(value(6, "8000000000000000"), None);
        // Types that are not decoded, and ids outside the table.
        assert_eq!(value(8, "00"), None);
        assert_eq!(value(0, "00"), None);
        assert_eq!(value(11, "00"), None);
    }

    #[test]
    fn values_print_as_the_editor_shows_them() {
        let show = |value: Value| value.to_string();
        assert_eq!(show(Value::Boolean(true)), "True");
        assert_eq!(show(Value::Integer(-3)), "-3");
        assert_eq!(show(Value::String("a\"b\n".to_owned())), "\"a\\\"b\\n\"");
        assert_eq!(
            show(Value::Enum {
                value: 1,
                name: Some("Idle".to_owned())
            }),
            "\"Idle\""
        );
        assert_eq!(
            show(Value::Enum {
                value: 7,
                name: None
            }),
            "7"
        );
    }

    #[test]
    fn nested_descriptor_lengths_are_bounded() {
        // A nested descriptor that claims far more than its parent holds,
        // one that claims less than a header, and one cut off mid-header:
        // the type definition stays opaque or is read from what is there.
        for nested in ["ffff 4016 0001 0141 00", "0003 4016", "00", ""] {
            let mut body = hex("00000000 00000001 0161");
            body.extend(hex(nested));
            let t = raw_table(&[desc(CODE_TYPEDEF, &body)]);
            assert!(matches!(
                t.heap_type(1),
                Some(DataType::Numeric | DataType::Other)
            ));
        }
        // A name count far larger than the body.
        let t = raw_table(&[desc(CODE_TYPEDEF, &hex("00000000 ffffffff 0161"))]);
        assert_eq!(t.heap_type(1), Some(DataType::Other));
    }

    #[test]
    fn missing_dthp_means_no_heap_types() {
        let vctp = table(&[desc(CODE_BOOLEAN, &[])], &[0]);
        let t = TypeTable::parse(&vctp, None).unwrap();
        assert_eq!(t.heap_type(1), None);
        let t = TypeTable::parse(&vctp, Some(&[0, 0])).unwrap();
        assert_eq!(t.heap_type(1), None);
    }

    #[test]
    fn rejects_malformed_tables() {
        assert!(TypeTable::parse(&[0xff, 0xff, 0xff, 0xff], None).is_err());
        // Descriptor length shorter than its own header.
        assert!(TypeTable::parse(&[0, 0, 0, 1, 0, 2, 0, 0], None).is_err());
        // Descriptor runs past the end.
        assert!(TypeTable::parse(&[0, 0, 0, 1, 0, 40, 0, 0], None).is_err());
        // Out-of-range indices resolve to nothing rather than panicking.
        let vctp = table(&[desc(CODE_CLUSTER, &[0, 1, 0, 99])], &[0, 77]);
        let t = TypeTable::parse(&vctp, Some(&[0, 2, 0, 1])).unwrap();
        assert_eq!(t.heap_type(1), Some(DataType::Cluster));
        assert_eq!(t.heap_type(2), None);
    }
}
