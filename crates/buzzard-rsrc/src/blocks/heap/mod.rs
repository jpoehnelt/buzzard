//! Object heaps (`BDHb` for the block diagram, `FPHb` for the front panel).
//!
//! After zlib decompression a heap is a `u32` content length followed by a
//! stream of tag entries that open, close or stand alone, forming a tree
//! much like XML. Each entry starts with two bytes:
//!
//! ```text
//! byte 0: sss a cc ii    s = size spec, a = has attributes, c = scope,
//! byte 1: iiiiiiii       i = raw tag id (10 bits)
//! ```
//!
//! followed by an optional attribute list and the content bytes.

mod primitives;
mod tables;

use crate::error::{malformed, Result};
use crate::reader::Reader;

/// Attribute id of the object class.
pub const ATTR_CLASS: i32 = -2;
/// Attribute id of the object's unique id within the heap.
pub const ATTR_UID: i32 = -3;

/// Tag ids of the structural system tags.
pub mod tag {
    pub const OBJECT: i32 = -3;
    pub const ARRAY: i32 = -4;
    pub const REFERENCE: i32 = -5;
    pub const ARRAY_ELEMENT: i32 = -6;
    pub const ROOT_OBJECT: i32 = -7;
}

/// Deepest nesting accepted, so recursive consumers cannot overflow the stack.
pub const MAX_DEPTH: usize = 256;

/// The attribute ids that the nodes of real heaps carry: class, unique
/// id, element count of a list, and one more of unknown meaning.
const KNOWN_ATTRS: [i32; 4] = [ATTR_CLASS, ATTR_UID, -5, -6];

const SCOPE_OPEN: u8 = 0;
const SCOPE_LEAF: u8 = 1;
const SCOPE_CLOSE: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content<'a> {
    None,
    Bool(bool),
    Bytes(&'a [u8]),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeapNode<'a> {
    pub tag: i32,
    /// `(attribute id, value)` pairs in stream order.
    pub attrs: Vec<(i32, i32)>,
    pub content: Content<'a>,
    pub children: Vec<HeapNode<'a>>,
}

impl<'a> HeapNode<'a> {
    pub fn attr(&self, id: i32) -> Option<i32> {
        self.attrs.iter().find(|(k, _)| *k == id).map(|(_, v)| *v)
    }

    /// Class id from the `class` attribute, if the node is an object.
    pub fn class(&self) -> Option<i32> {
        self.attr(ATTR_CLASS)
    }

    pub fn uid(&self) -> Option<i32> {
        self.attr(ATTR_UID)
    }

    pub fn class_name(&self) -> Option<&'static str> {
        class_name(self.class()?)
    }

    pub fn tag_name(&self) -> Option<&'static str> {
        tag_name(self.tag)
    }

    /// First direct child with the given tag.
    pub fn child(&self, tag: i32) -> Option<&HeapNode<'a>> {
        self.children.iter().find(|c| c.tag == tag)
    }

    /// First direct child whose tag has the given name.
    pub fn child_named(&self, name: &str) -> Option<&HeapNode<'a>> {
        self.children.iter().find(|c| c.tag_name() == Some(name))
    }

    pub fn bytes(&self) -> Option<&'a [u8]> {
        match self.content {
            Content::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// Content as a big-endian signed integer of 1 to 4 bytes.
    pub fn int(&self) -> Option<i32> {
        let b = self.bytes()?;
        if b.is_empty() || b.len() > 4 {
            return None;
        }
        let mut value: i32 = if b[0] & 0x80 != 0 { -1 } else { 0 };
        for &byte in b {
            value = (value << 8) | i32::from(byte);
        }
        Some(value)
    }

    /// Content as a big-endian unsigned integer of 1 to 4 bytes.
    pub fn uint(&self) -> Option<u32> {
        let b = self.bytes()?;
        if b.is_empty() || b.len() > 4 {
            return None;
        }
        Some(
            b.iter()
                .fold(0u32, |acc, &byte| (acc << 8) | u32::from(byte)),
        )
    }

    /// Total number of nodes in this subtree, including this one.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(HeapNode::count).sum::<usize>()
    }
}

/// Name of a tag: a system tag or a common object field.
///
/// A few object classes (mostly front-panel ones) reuse field ids with their
/// own meanings; those class-specific tables are not modelled here.
pub fn tag_name(id: i32) -> Option<&'static str> {
    match id {
        tag::OBJECT => Some("SL__object"),
        tag::ARRAY => Some("SL__array"),
        tag::REFERENCE => Some("SL__reference"),
        tag::ARRAY_ELEMENT => Some("SL__arrayElement"),
        tag::ROOT_OBJECT => Some("SL__rootObject"),
        _ => lookup(tables::FIELD_TAGS, id),
    }
}

pub fn class_name(id: i32) -> Option<&'static str> {
    lookup(tables::CLASS_TAGS, id)
}

/// Id of the field tag with the given name.
pub fn tag_id(name: &str) -> Option<i32> {
    tables::FIELD_TAGS
        .iter()
        .find(|(_, n)| *n == name)
        .map(|(id, _)| *id)
}

/// Id of the object class with the given name.
pub fn class_id(name: &str) -> Option<i32> {
    tables::CLASS_TAGS
        .iter()
        .find(|(_, n)| *n == name)
        .map(|(id, _)| *id)
}

/// Name of a built-in function, from the vendored table in `primitives.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PrimitiveName {
    /// The function's name as the editor's palette shows it, e.g. `Add`.
    /// Printable ASCII without quotes, backticks, backslashes or brackets.
    /// Not unique: a few functions share a name under different ids.
    pub name: &'static str,
    /// Whether the source of the table records that it confirmed which
    /// function the id is. An unverified name is the best one known and may
    /// be wrong.
    pub verified: bool,
}

/// Name of the built-in function with the given resource id (the
/// `primResID` of a `prim` object), if the vendored table has it.
///
/// The table comes from LVKit (Apache-2.0); see `NOTICE` at the repository
/// root for its origin and limits.
pub fn primitive_name(id: u32) -> Option<PrimitiveName> {
    let table = primitives::PRIMITIVES;
    let index = table.binary_search_by_key(&id, |(k, _, _)| *k).ok()?;
    let (_, name, verified) = table[index];
    Some(PrimitiveName { name, verified })
}

/// Every built-in function the vendored table names, in ascending id order.
pub fn primitive_names() -> impl ExactSizeIterator<Item = (u32, PrimitiveName)> {
    primitives::PRIMITIVES
        .iter()
        .map(|&(id, name, verified)| (id, PrimitiveName { name, verified }))
}

fn lookup(table: &'static [(i32, &'static str)], id: i32) -> Option<&'static str> {
    table
        .binary_search_by_key(&id, |(k, _)| *k)
        .ok()
        .map(|i| table[i].1)
}

/// A decoded heap: usually a single root object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heap<'a> {
    pub roots: Vec<HeapNode<'a>>,
    /// The stream breaks a rule that every heap of 1,773 real files
    /// keeps: an entry that closes a node has the tag of the entry that
    /// opened it and carries no attributes and no content, a node has
    /// each of its attributes once and only of the four kinds seen
    /// (ids -2, -3, -5 and -6), and no node is left open at the end. The parser reads on regardless, so that a
    /// damaged file stays usable, but nodes may then be missing: a
    /// damaged attribute count swallows the entries after it.
    pub irregular: bool,
}

impl<'a> Heap<'a> {
    /// Parse a decompressed heap block.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(data, "heap");
        let length = r.u32()? as usize;
        let mut r = Reader::new(r.take(length)?, "heap");

        // Open nodes, innermost last. `stack[0]` collects the roots.
        let mut stack: Vec<Vec<HeapNode<'a>>> = vec![Vec::new()];
        let mut open: Vec<HeapNode<'a>> = Vec::new();
        let mut irregular = false;
        while r.remaining() > 0 {
            let b0 = r.u8()?;
            let b1 = r.u8()?;
            let size_spec = (b0 >> 5) & 7;
            let has_attrs = (b0 >> 4) & 1 != 0;
            let scope = (b0 >> 2) & 3;
            let raw_tag = i32::from(b1) | (i32::from(b0 & 3) << 8);
            let tag = if raw_tag == 1023 {
                r.u32()? as i32
            } else {
                raw_tag - 31
            };

            let mut attrs = Vec::new();
            if has_attrs {
                let count = var_u124(&mut r)? as usize;
                if count > r.remaining() / 3 {
                    return Err(malformed(
                        "heap",
                        format!("implausible attribute count {count}"),
                    ));
                }
                attrs.reserve(count);
                for _ in 0..count {
                    let id = var_s124(&mut r)?;
                    let value = var_s24(&mut r)?;
                    attrs.push((id, value));
                }
            }
            // Short enough to compare pairwise; a longer list is not one
            // a real node has.
            irregular |= attrs.len() > KNOWN_ATTRS.len()
                || attrs.iter().enumerate().any(|(at, (id, _))| {
                    !KNOWN_ATTRS.contains(id)
                        || attrs[..at].iter().any(|(earlier, _)| earlier == id)
                });
            let content = match size_spec {
                0 => Content::None,
                7 => Content::Bool(true),
                1..=4 => Content::Bytes(r.take(size_spec as usize)?),
                6 => {
                    let size = var_u124(&mut r)? as usize;
                    Content::Bytes(r.take(size)?)
                }
                other => return Err(malformed("heap", format!("unknown size spec {other}"))),
            };

            match scope {
                SCOPE_OPEN => {
                    if open.len() >= MAX_DEPTH {
                        return Err(malformed("heap", "nesting is too deep"));
                    }
                    open.push(HeapNode {
                        tag,
                        attrs,
                        content,
                        children: Vec::new(),
                    });
                    stack.push(Vec::new());
                }
                SCOPE_LEAF => {
                    let content = if size_spec == 0 {
                        Content::Bool(false)
                    } else {
                        content
                    };
                    let node = HeapNode {
                        tag,
                        attrs,
                        content,
                        children: Vec::new(),
                    };
                    stack.last_mut().expect("stack is never empty").push(node);
                }
                SCOPE_CLOSE => {
                    let Some(mut node) = open.pop() else {
                        return Err(malformed("heap", "close tag without a matching open tag"));
                    };
                    irregular |= node.tag != tag || has_attrs || size_spec != 0;
                    node.children = stack.pop().expect("one child list per open node");
                    stack.last_mut().expect("stack is never empty").push(node);
                }
                other => return Err(malformed("heap", format!("unknown scope {other}"))),
            }
        }
        // Tolerate a truncated tail: close whatever is still open.
        irregular |= !open.is_empty();
        while let Some(mut node) = open.pop() {
            node.children = stack.pop().expect("one child list per open node");
            stack.last_mut().expect("stack is never empty").push(node);
        }
        Ok(Self {
            roots: stack.pop().expect("root list"),
            irregular,
        })
    }

    pub fn node_count(&self) -> usize {
        self.roots.iter().map(HeapNode::count).sum()
    }
}

/// Unsigned 8-bit value, widened to 16 bits after `0xFF` or 32 after `0xFE`.
fn var_u124(r: &mut Reader<'_>) -> Result<u32> {
    Ok(match r.u8()? {
        255 => u32::from(r.u16()?),
        254 => r.u32()?,
        small => u32::from(small),
    })
}

/// Signed 8-bit value, widened to 16 bits after `-128` or 32 after `-127`.
fn var_s124(r: &mut Reader<'_>) -> Result<i32> {
    Ok(match r.u8()? as i8 {
        -128 => i32::from(r.u16()? as i16),
        -127 => r.u32()? as i32,
        small => i32::from(small),
    })
}

/// Signed 16-bit value, or 32 bits after `-0x8000`.
fn var_s24(r: &mut Reader<'_>) -> Result<i32> {
    Ok(match r.u16()? as i16 {
        i16::MIN => r.u32()? as i32,
        small => i32::from(small),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heap(body: &[u8]) -> Vec<u8> {
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(body);
        out
    }

    /// Entry header for a tag id, scope and size spec, optionally with attributes.
    fn entry(tag: i32, scope: u8, size_spec: u8, attrs: bool) -> [u8; 2] {
        let raw = (tag + 31) as u16;
        [
            (size_spec << 5) | (u8::from(attrs) << 4) | (scope << 2) | (raw >> 8) as u8,
            raw as u8,
        ]
    }

    #[test]
    fn tables_are_sorted_for_binary_search() {
        for table in [tables::FIELD_TAGS, tables::CLASS_TAGS] {
            assert!(table.windows(2).all(|w| w[0].0 < w[1].0));
        }
        assert_eq!(tag_name(268), Some("termList"));
        assert_eq!(class_name(49), Some("iUse"));
        assert_eq!(tag_id("termList"), Some(268));
        assert_eq!(class_id("prim"), Some(47));
        assert_eq!(tag_name(tag::REFERENCE), Some("SL__reference"));
        assert_eq!(tag_name(99_999), None);
    }

    #[test]
    fn primitive_table_is_sorted_and_unique() {
        let table = primitives::PRIMITIVES;
        assert!(table.len() > 600);
        // Strictly ascending ids: sorted for binary search, and no duplicates.
        assert!(table.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(primitive_names().len(), table.len());
        for (id, entry) in primitive_names() {
            assert_eq!(primitive_name(id), Some(entry));
        }
    }

    #[test]
    fn primitive_names_are_looked_up_by_id() {
        assert_eq!(
            primitive_name(1050),
            Some(PrimitiveName {
                name: "Add",
                verified: true
            })
        );
        let unverified = primitive_name(8011).expect("in the table");
        assert_eq!(unverified.name, "Close Reference");
        assert!(!unverified.verified);
        for unknown in [0, 1, 1043, 1_000_000, u32::MAX] {
            assert_eq!(primitive_name(unknown), None);
        }
    }

    #[test]
    fn primitive_names_are_safe_to_print() {
        for (id, entry) in primitive_names() {
            let name = entry.name;
            assert!(!name.is_empty() && name.len() <= 80, "{id}: {name:?}");
            assert_eq!(name, name.trim(), "{id}");
            assert!(!name.contains("  "), "{id}: {name:?}");
            // No control characters, nothing outside ASCII, and none of the
            // characters that delimit Rust, Markdown, SVG, DOT or Mermaid
            // text. `&` is allowed: the SVG writer escapes it.
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || " &'()+,-/?.:".contains(c)),
                "{id}: {name:?}"
            );
        }
    }

    #[test]
    fn parses_nested_tree_with_attributes_and_content() {
        let mut body = Vec::new();
        // <object class=47 uid=5>
        body.extend(entry(tag::OBJECT, SCOPE_OPEN, 0, true));
        body.extend([2, (-2i8) as u8, 0, 47, (-3i8) as u8, 0, 5]);
        //   <primIndex>0x0102</primIndex> (leaf, two bytes)
        body.extend(entry(202, SCOPE_LEAF, 2, false));
        body.extend([1, 2]);
        //   <flag/> (leaf true) and a length-prefixed leaf
        body.extend(entry(100, SCOPE_LEAF, 7, false));
        body.extend(entry(101, SCOPE_LEAF, 6, false));
        body.extend([3, b'a', b'b', b'c']);
        // </object>
        body.extend(entry(tag::OBJECT, SCOPE_CLOSE, 0, false));

        let bytes = heap(&body);
        let parsed = Heap::parse(&bytes).unwrap();
        assert!(!parsed.irregular);
        assert_eq!(parsed.roots.len(), 1);
        assert_eq!(parsed.node_count(), 4);
        let object = &parsed.roots[0];
        assert_eq!((object.class(), object.uid()), (Some(47), Some(5)));
        assert_eq!(object.class_name(), Some("prim"));
        assert_eq!(object.child(202).unwrap().int(), Some(0x0102));
        assert_eq!(
            object.child_named("primIndex").unwrap().uint(),
            Some(0x0102)
        );
        assert_eq!(object.child(100).unwrap().content, Content::Bool(true));
        assert_eq!(object.child(101).unwrap().bytes(), Some(&b"abc"[..]));
    }

    #[test]
    fn an_irregular_stream_is_read_and_flagged() {
        // <object class=47 uid=5> <leaf 100/> <leaf 101/> </object>
        let object = |attrs: &[u8], close: Vec<u8>| {
            let mut body = entry(tag::OBJECT, SCOPE_OPEN, 0, true).to_vec();
            body.extend_from_slice(attrs);
            body.extend(entry(100, SCOPE_LEAF, 7, false));
            body.extend(entry(101, SCOPE_LEAF, 7, false));
            body.extend(close);
            heap(&body)
        };
        let class_and_uid = [2, (-2i8) as u8, 0, 47, (-3i8) as u8, 0, 5];
        let close = || entry(tag::OBJECT, SCOPE_CLOSE, 0, false).to_vec();
        let bytes = object(&class_and_uid, close());
        let regular = Heap::parse(&bytes).unwrap();
        assert!(!regular.irregular);
        assert_eq!(regular.node_count(), 3);

        // The entry that closes the object has another tag, carries
        // content, or is missing.
        let other_tag = object(&class_and_uid, entry(202, SCOPE_CLOSE, 0, false).to_vec());
        let mut with_content = entry(tag::OBJECT, SCOPE_CLOSE, 1, false).to_vec();
        with_content.push(9);
        let with_content = object(&class_and_uid, with_content);
        let left_open = object(&class_and_uid, Vec::new());
        // An attribute of a kind no node has, and the same one twice.
        let unknown = object(&[2, (-2i8) as u8, 0, 47, 7, 0, 5], close());
        let twice = object(&[2, (-3i8) as u8, 0, 47, (-3i8) as u8, 0, 5], close());
        for bytes in [other_tag, with_content, left_open, unknown, twice] {
            let parsed = Heap::parse(&bytes).unwrap();
            assert!(parsed.irregular);
            assert_eq!(parsed.node_count(), 3);
        }

        // What one changed byte does: the attribute count 2 becomes 4,
        // and the two leaves that follow are read as attributes. The
        // tree is whole but two nodes short.
        let swallowed = [4, (-2i8) as u8, 0, 47, (-3i8) as u8, 0, 5];
        let bytes = object(&swallowed, close());
        let parsed = Heap::parse(&bytes).unwrap();
        assert!(parsed.irregular);
        assert_eq!(parsed.node_count(), 1);
    }

    #[test]
    fn decodes_wide_fields() {
        let mut body = Vec::new();
        // Tag id escaped to 32 bits, attribute id and value both widened.
        body.extend([(1 << 4) | (SCOPE_LEAF << 2) | 0x03, 0xFF]);
        body.extend(70_000i32.to_be_bytes());
        body.push(1);
        body.extend([0x80, 0x01, 0x00]); // id = 256 via 16-bit form
        body.extend([0x80, 0x00]); // value escape
        body.extend((-70_000i32).to_be_bytes());
        let bytes = heap(&body);
        let parsed = Heap::parse(&bytes).unwrap();
        assert_eq!(parsed.roots[0].tag, 70_000);
        assert_eq!(parsed.roots[0].attrs, [(256, -70_000)]);
    }

    #[test]
    fn signed_content_sign_extends() {
        let node = HeapNode {
            tag: 0,
            attrs: vec![],
            content: Content::Bytes(&[0xFF, 0xFE]),
            children: vec![],
        };
        assert_eq!(node.int(), Some(-2));
        assert_eq!(node.uint(), Some(0xFFFE));
    }

    #[test]
    fn rejects_malformed_streams() {
        // Close without open.
        assert!(Heap::parse(&heap(&entry(tag::OBJECT, SCOPE_CLOSE, 0, false))).is_err());
        // Content runs past the end.
        let mut body = entry(5, SCOPE_LEAF, 4, false).to_vec();
        body.push(1);
        assert!(Heap::parse(&heap(&body)).is_err());
        // Declared length longer than the buffer.
        assert!(Heap::parse(&[0, 0, 0, 9, 1]).is_err());
        // Absurd attribute count.
        let mut body = entry(5, SCOPE_LEAF, 0, true).to_vec();
        body.extend([254, 0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(Heap::parse(&heap(&body)).is_err());
    }

    #[test]
    fn bounds_nesting_depth_and_closes_truncated_tail() {
        let mut body = Vec::new();
        for _ in 0..3 {
            body.extend(entry(tag::OBJECT, SCOPE_OPEN, 0, false));
        }
        let parsed_bytes = heap(&body);
        let parsed = Heap::parse(&parsed_bytes).unwrap();
        assert_eq!(parsed.node_count(), 3);
        assert_eq!(parsed.roots[0].children[0].children.len(), 1);

        let mut deep = Vec::new();
        for _ in 0..=MAX_DEPTH {
            deep.extend(entry(tag::OBJECT, SCOPE_OPEN, 0, false));
        }
        assert!(Heap::parse(&heap(&deep)).is_err());
    }
}
