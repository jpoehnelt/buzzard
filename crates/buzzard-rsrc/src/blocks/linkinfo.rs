//! Link info blocks (`LIvi`, `LIfp`, `LIbd`): references to other files.
//!
//! The full link-object grammar has well over a hundred variants whose layout
//! depends on the saving version. Every variant that references a file shares
//! a common prefix, though:
//!
//! ```text
//! fourcc   link type, e.g. `VIVI` (subVI) or `TDCC` (typedef)
//! pad      to a 4-byte boundary
//! u32      qualified-name part count, then that many Pascal strings
//! pad      to a 2-byte boundary
//! PTHx     flattened path
//! ```
//!
//! This module locates each path and reads that prefix backwards from it,
//! which is robust across versions without modelling every variant.

use serde::Serialize;

use crate::blocks::path::LvPath;
use crate::fourcc::FourCc;
use crate::reader::{decode_text, Reader};

/// Resource types that hold link info for the VI, front panel and diagram.
pub const LINK_BLOCKS: [&[u8; 4]; 3] = [b"LIvi", b"LIfp", b"LIbd"];

const MAX_NAME_PARTS: u32 = 32;
const MAX_LOOKBEHIND: usize = 4096;

/// One reference from a file to another file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct LinkRef {
    /// Link object type, e.g. `VIVI`, when it could be identified.
    pub link_type: Option<FourCc>,
    /// Qualified name: owning libraries followed by the item name.
    pub qualified_name: Vec<String>,
    pub path: LvPath,
}

impl LinkRef {
    /// Item name: last part of the qualified name, else the path's file name.
    pub fn name(&self) -> Option<&str> {
        self.qualified_name
            .last()
            .map(String::as_str)
            .or_else(|| self.path.file_name())
    }
}

/// Extract every link with a non-empty path from one link info block.
pub fn scan(data: &[u8]) -> Vec<LinkRef> {
    let mut links = Vec::new();
    let mut offset = 0;
    while let Some(found) = find(&data[offset..], b"PTH") {
        let at = offset + found;
        match LvPath::parse(&data[at..]) {
            Ok((path, len)) => {
                if !path.is_empty() {
                    let (link_type, qualified_name) = prefix_before(data, at).unwrap_or_default();
                    links.push(LinkRef { link_type, qualified_name, path });
                }
                offset = at + len;
            }
            Err(_) => offset = at + 3,
        }
    }
    links
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Read the link type and qualified name that end right before `path_at`.
fn prefix_before(data: &[u8], path_at: usize) -> Option<(Option<FourCc>, Vec<String>)> {
    for pad in 0..=1 {
        let end = path_at.checked_sub(pad)?;
        if data[end..path_at].iter().any(|&b| b != 0) {
            continue;
        }
        let lowest = end.saturating_sub(MAX_LOOKBEHIND);
        // A name list is at least a count plus one empty string.
        let mut start = end.checked_sub(5)?;
        loop {
            if let Some(names) = names_between(data, start, end) {
                return Some((link_type_before(data, start), names));
            }
            if start <= lowest {
                break;
            }
            start -= 1;
        }
    }
    None
}

fn names_between(data: &[u8], start: usize, end: usize) -> Option<Vec<String>> {
    let mut r = Reader::new(&data[start..end], "link name");
    let count = r.u32().ok()?;
    if count == 0 || count > MAX_NAME_PARTS {
        return None;
    }
    let mut names = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let raw = r.pstr().ok()?;
        if raw.iter().any(|&b| b < 0x20) {
            return None;
        }
        names.push(decode_text(raw));
    }
    (r.remaining() == 0).then_some(names)
}

fn link_type_before(data: &[u8], names_at: usize) -> Option<FourCc> {
    (0..4).find_map(|pad| {
        let end = names_at.checked_sub(pad)?;
        if data[end..names_at].iter().any(|&b| b != 0) {
            return None;
        }
        let code: [u8; 4] = data.get(end.checked_sub(4)?..end)?.try_into().ok()?;
        code.iter().all(u8::is_ascii_alphanumeric).then_some(FourCc(code))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::path::PathKind;

    // `LIvi` block of a real VI calling `empty.vi` through a relative path.
    const RELATIVE: &[u8] = b"\x00\x01LVIN\x00\x00\x00\x01\x00\x02VIVI\x00\x00\x00\x01\x08empty.vi\x00\
PTH0\x00\x00\x00\x0e\x00\x01\x00\x02\x00\x08empty.vi\x00\x00\x00\x00\x01\x42";

    #[test]
    fn extracts_link_with_prefix() {
        let links = scan(RELATIVE);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, Some(FourCc::new(b"VIVI")));
        assert_eq!(links[0].qualified_name, ["empty.vi"]);
        assert_eq!(links[0].path.kind, PathKind::Relative);
        assert_eq!(links[0].path.to_string(), "../empty.vi");
        assert_eq!(links[0].name(), Some("empty.vi"));
    }

    #[test]
    fn skips_empty_paths_and_lookalikes() {
        let mut data = b"\x00\x01BDHP\x00\x00\x00\x00\x00\x03".to_vec();
        data.extend_from_slice(b"BSVR\x00\x00\x00\x00PTH0\x00\x00\x00\x04\x00\x01\x00\x00");
        data.extend_from_slice(b"\x08PTH test");
        assert!(scan(&data).is_empty());
    }

    #[test]
    fn path_without_recognizable_prefix_is_still_reported() {
        let data = b"\xff\xffPTH0\x00\x00\x00\x09\x00\x01\x00\x01\x04a.vi";
        let links = scan(data);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, None);
        assert!(links[0].qualified_name.is_empty());
        assert_eq!(links[0].name(), Some("a.vi"));
    }
}
