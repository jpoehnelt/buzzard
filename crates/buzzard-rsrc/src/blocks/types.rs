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
use crate::reader::Reader;

const CODE_INT32: u8 = 0x03;
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
    /// A named type definition wrapping another descriptor.
    TypeDef(Box<TypeDesc>),
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
                Body::TypeDef(inner) => desc = inner,
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
        let mut desc = desc;
        for _ in 0..MAX_TYPEDEF_DEPTH {
            match &desc.body {
                Body::TypeDef(inner) => desc = inner,
                _ => break,
            }
        }
        desc.code
    }
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
    for _ in 0..names {
        body.pstr()?;
    }
    Ok(Body::TypeDef(Box::new(parse_nested_desc(body, depth + 1)?)))
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
