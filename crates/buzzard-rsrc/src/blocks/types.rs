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
    /// Datalog file (kind 1).
    DataLog,
    /// Byte stream file (kind 2).
    File,
    /// Device (kind 3).
    Device,
    /// Occurrence (kind 4).
    Occurrence,
    /// TCP connection (kind 5).
    Tcp,
    /// ActiveX automation object (kind 7).
    Automation,
    /// VI Server reference to an application, VI or control (kind 8).
    ViServer,
    /// DataSocket connection (kind 13).
    DataSocket,
    /// VISA resource (kind 14).
    Visa,
    /// IVI instrument driver session (kind 15).
    Ivi,
    /// UDP connection (kind 16).
    Udp,
    /// Notifier (kind 17).
    Notifier,
    /// Queue (kind 18).
    Queue,
    /// IrDA connection (kind 19).
    Irda,
    /// User-defined refnum (kinds 20, 21 and 29).
    UserDefined,
    /// Event registration (kind 23).
    EventRegistration,
    /// .NET object (kind 24).
    DotNet,
    /// User event (kind 25).
    UserEvent,
    /// Bluetooth connection (kind 31).
    Bluetooth,
    /// Data value reference (kind 32).
    DataValue,
    /// FIFO (kind 33).
    Fifo,
    /// TDMS file (kind 34).
    Tdms,
    /// Any other kind.
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
    /// A refnum, with the kind of thing it refers to.
    Reference(RefKind),
    /// Boolean (type codes `0x20` and `0x21`).
    Boolean,
    /// Number of any representation (type codes `0x01` to `0x1E`, `0x5E`
    /// and `0x5F`).
    Numeric,
    /// String (type codes `0x30` and `0x31`).
    String,
    /// Path (type code `0x32`).
    Path,
    /// Array of any element type (type code `0x40`).
    Array,
    /// Any cluster other than the error cluster.
    Cluster,
    /// Variant (type code `0x53`).
    Variant,
    /// A VI's connector pane signature.
    Function,
    /// No data (type code `0x00`).
    Void,
    /// Any other type.
    Other,
}

/// A VI's type table, used to classify the types of heap objects.
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
    ///
    /// # Errors
    ///
    /// If either block is truncated, a count is too large for the data that
    /// follows it, or a descriptor is shorter than its own header. A
    /// descriptor whose body does not parse is kept as an opaque type.
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
        let top = std::iter::repeat_with(|| var_u2p2(&mut r))
            .take(top_count)
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

    /// Number of flat type descriptors.
    pub fn len(&self) -> usize {
        self.flat.len()
    }

    /// Whether the table has no descriptors.
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
                    .map(|&i| self.flat.get(i as usize).map(unwrap_code))
                    .collect();
                if codes == [Some(CODE_BOOLEAN), Some(CODE_INT32), Some(CODE_STRING)] {
                    DataType::ErrorCluster
                } else {
                    DataType::Cluster
                }
            }
            (Body::Refnum(kind), _) => DataType::Reference(RefKind::from_code(*kind)),
            (_, 0x00) => DataType::Void,
            (_, 0x01..=0x1E | 0x5E | 0x5F) => DataType::Numeric,
            (_, 0x20 | CODE_BOOLEAN) => DataType::Boolean,
            (_, CODE_STRING | 0x31) => DataType::String,
            (_, CODE_PATH) => DataType::Path,
            (_, CODE_ARRAY) => DataType::Array,
            (_, CODE_VARIANT) => DataType::Variant,
            (_, CODE_FUNCTION) => DataType::Function,
            _ => DataType::Other,
        }
    }
}

/// Type code after looking through type definitions.
fn unwrap_code(desc: &TypeDesc) -> u8 {
    let mut desc = desc;
    for _ in 0..MAX_TYPEDEF_DEPTH {
        match &desc.body {
            Body::TypeDef(inner) => desc = inner,
            _ => break,
        }
    }
    desc.code
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
    let _flags = body.u8()?;
    let code = body.u8()?;
    // A descriptor whose body does not parse is kept as opaque; its length
    // is known, so the table stays in step.
    let parsed = match code {
        CODE_CLUSTER => parse_cluster(&mut body).unwrap_or(Body::Other),
        CODE_REFNUM => body.u16().map_or(Body::Other, Body::Refnum),
        CODE_TYPEDEF if depth < MAX_TYPEDEF_DEPTH => {
            parse_typedef(&mut body, depth).unwrap_or(Body::Other)
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
        std::iter::repeat_with(|| var_u2p2(body))
            .take(count)
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
    Ok(Body::TypeDef(Box::new(parse_desc(body, depth + 1)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(code: u8, body: &[u8]) -> Vec<u8> {
        let len = u16::try_from(body.len() + 4).expect("test descriptor length fits in a u16");
        let mut out = len.to_be_bytes().to_vec();
        out.extend([0, code]);
        out.extend_from_slice(body);
        out
    }

    fn table(descs: &[Vec<u8>], top: &[u16]) -> Vec<u8> {
        let count = u32::try_from(descs.len()).expect("test type count fits in a u32");
        let mut out = count.to_be_bytes().to_vec();
        for d in descs {
            out.extend(d);
        }
        let top_count = u16::try_from(top.len()).expect("test top-level count fits in a u16");
        out.extend(top_count.to_be_bytes());
        for t in top {
            out.extend(t.to_be_bytes());
        }
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
