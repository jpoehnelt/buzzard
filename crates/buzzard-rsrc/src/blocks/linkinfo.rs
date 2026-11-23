//! Link info blocks (`LIvi`, `LIfp`, `LIbd`): references to other files.
//!
//! The full link-object grammar has well over a hundred variants whose layout
//! depends on the saving version. But every variant that references a file
//! starts with the same prefix:
//!
//! ```text
//! fourcc   link type, such as `VIVI` (subVI) or `TDCC` (typedef)
//! pad      to a 4-byte boundary
//! u32      qualified-name part count, then that many Pascal strings
//! pad      to a 2-byte boundary
//! PTHx     flattened path
//! ```
//!
//! This module locates each path and reads that prefix backwards from it,
//! so it needs no model of each version's variant of the record.

use serde::Serialize;

use crate::blocks::path::LvPath;
use crate::fourcc::FourCc;
use crate::reader::{decode_text, Reader};

/// Resource types that hold link info for the VI, front panel, and diagram.
pub const LINK_BLOCKS: [&[u8; 4]; 3] = [b"LIvi", b"LIfp", b"LIbd"];

const MAX_NAME_PARTS: u32 = 32;
const MAX_LOOKBEHIND: usize = 4096;
/// Bytes of parsing work allowed per byte of block. [`scan`] spends it.
const WORK_PER_BYTE: usize = 16;
/// Most heap objects one link is expected to serve.
const MAX_HEAP_USES: usize = 4096;
/// Heap object ids are small sequential numbers.
const MAX_HEAP_UID: u32 = 1 << 24;

/// One reference from a file to another file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct LinkRef {
    /// Link object type, such as `VIVI`, when it could be identified.
    pub link_type: Option<FourCc>,
    /// Qualified name: owning libraries followed by the item name.
    pub qualified_name: Vec<String>,
    /// Where the referenced file was when the link was saved.
    pub path: LvPath,
    /// Unique ids of the heap objects (for example subVI call nodes) that
    /// use this link. Empty for links that are not tied to heap objects.
    pub heap_uids: Vec<u32>,
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
    let mut links: Vec<LinkRef> = Vec::new();
    let mut offset = 0;
    // Index into `links` and end offset of the most recent named link.
    let mut pending: Option<(usize, usize)> = None;
    // End of the last accepted path. No prefix reaches back past it.
    let mut floor = 0;
    // The budget bounds the total work on input that is full of path
    // look-alikes. Real blocks use a small fraction of it.
    let mut budget = data.len().saturating_mul(WORK_PER_BYTE) + 64 * 1024;
    while let Some(found) = find(&data[offset..], b"PTH") {
        let at = offset + found;
        let declared = data
            .get(at + 4..at + 8)
            .map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
        let cost = 16 + declared.min(data.len() - at);
        if cost > budget {
            break;
        }
        budget -= cost;
        match LvPath::parse(&data[at..]) {
            Ok((path, len)) => {
                // A heap link's record ends with the list of heap objects
                // that use it, directly followed by a second, usually empty,
                // path. That list sits right before this path.
                if let Some((index, end)) = pending.take() {
                    links[index].heap_uids = uid_list_before(&data[end..at]);
                }
                if !path.is_empty() {
                    let (link_type, qualified_name) =
                        prefix_before(data, at, floor).unwrap_or_default();
                    pending = Some((links.len(), at + len));
                    links.push(LinkRef {
                        link_type,
                        qualified_name,
                        path,
                        heap_uids: Vec::new(),
                    });
                }
                offset = at + len;
                floor = offset;
            }
            Err(_) => offset = at + 3,
        }
    }
    links
}

/// Read a `u32` count followed by that many `u32` ids from the end of
/// `tail`, choosing the shortest list whose count matches.
fn uid_list_before(tail: &[u8]) -> Vec<u32> {
    // `word(1)` is the last 32-bit word of `tail`, `word(2)` the one before.
    let word = |index_from_end: usize| -> Option<u32> {
        let end = tail.len().checked_sub((index_from_end - 1) * 4)?;
        let bytes = tail.get(end.checked_sub(4)?..end)?;
        Some(u32::from_be_bytes(bytes.try_into().ok()?))
    };
    for count in 1..=MAX_HEAP_USES {
        match word(count + 1) {
            Some(found) if found as usize == count => {
                let mut ids: Vec<u32> = (1..=count).rev().filter_map(word).collect();
                if ids.iter().all(|&id| id != 0 && id < MAX_HEAP_UID) {
                    ids.sort_unstable();
                    return ids;
                }
            }
            Some(_) => {}
            None => break,
        }
    }
    Vec::new()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Read the link type and qualified name that end right before `path_at`,
/// looking no further back than `floor` (the end of the previous path).
fn prefix_before(
    data: &[u8],
    path_at: usize,
    floor: usize,
) -> Option<(Option<FourCc>, Vec<String>)> {
    for pad in 0..=1 {
        let end = path_at.checked_sub(pad)?;
        if data[end..path_at].iter().any(|&b| b != 0) {
            continue;
        }
        let lowest = end.saturating_sub(MAX_LOOKBEHIND).max(floor);
        // A name list is at least a count plus one empty string.
        let mut start = end.checked_sub(5)?;
        while start >= lowest {
            // A cheap filter runs first, because a name list starts with a
            // small `u32`.
            let plausible = data[start..start + 3] == [0, 0, 0]
                && (1..=MAX_NAME_PARTS).contains(&u32::from(data[start + 3]));
            if plausible {
                if let Some(names) = names_between(data, start, end) {
                    return Some((link_type_before(data, start), names));
                }
            }
            if start == 0 {
                break;
            }
            start -= 1;
        }
    }
    None
}

fn names_between(data: &[u8], start: usize, end: usize) -> Option<Vec<String>> {
    let count = Reader::new(&data[start..end], "link name").u32().ok()?;
    if count == 0 || count > MAX_NAME_PARTS {
        return None;
    }
    // Validate the layout before allocating anything.
    let parts = |r: &mut Reader<'_>| -> Option<()> {
        for _ in 0..count {
            if r.pstr().ok()?.iter().any(|&b| b < 0x20) {
                return None;
            }
        }
        (r.remaining() == 0).then_some(())
    };
    parts(&mut Reader::new(&data[start + 4..end], "link name"))?;

    let mut r = Reader::new(&data[start + 4..end], "link name");
    std::iter::repeat_with(|| r.pstr().ok().map(decode_text))
        .take(count as usize)
        .collect()
}

fn link_type_before(data: &[u8], names_at: usize) -> Option<FourCc> {
    (0..4).find_map(|pad| {
        let end = names_at.checked_sub(pad)?;
        if data[end..names_at].iter().any(|&b| b != 0) {
            return None;
        }
        let code: [u8; 4] = data.get(end.checked_sub(4)?..end)?.try_into().ok()?;
        code.iter()
            .all(u8::is_ascii_alphanumeric)
            .then_some(FourCc(code))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::path::PathKind;

    // `LIvi` block of a real VI calling `empty.vi` through a relative path.
    const RELATIVE: &[u8] =
        b"\x00\x01LVIN\x00\x00\x00\x01\x00\x02VIVI\x00\x00\x00\x01\x08empty.vi\x00\
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
    fn reads_heap_object_ids_of_diagram_links() {
        // `LIbd` block of the same VI: the call node has heap uid 0x2b.
        let data: &[u8] =
            b"\x00\x01BDHP\x00\x00\x00\x01\x00\x02IUVI\x00\x00\x00\x01\x08empty.vi\x00\
PTH0\x00\x00\x00\x0e\x00\x01\x00\x02\x00\x08empty.vi\x00\x00\x00\x00\x00\x00\x42\x00\x00\x00\x00\
\x00\x00\x00\x01\x00\x00\x00\x2b\
PTH0\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x03";
        let links = scan(data);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, Some(FourCc::new(b"IUVI")));
        assert_eq!(links[0].heap_uids, [0x2b]);
        // The VI-level link in `LIvi` has no trailing path, so it has no ids.
        assert!(
            scan(RELATIVE)[0].heap_uids.is_empty(),
            "expected nothing, got {:?}",
            scan(RELATIVE)[0].heap_uids
        );
    }

    #[test]
    fn uid_list_requires_a_matching_count() {
        let words =
            |values: &[u32]| -> Vec<u8> { values.iter().flat_map(|v| v.to_be_bytes()).collect() };
        assert_eq!(
            uid_list_before(&words(&[9, 9, 2, 0x30, 0x10])),
            [0x10, 0x30]
        );
        assert_eq!(uid_list_before(&words(&[7, 7, 7])), Vec::<u32>::new());
        assert_eq!(uid_list_before(&words(&[1, 0])), Vec::<u32>::new());
        assert_eq!(uid_list_before(&[1, 2, 3]), Vec::<u32>::new());
        assert_eq!(uid_list_before(&[]), Vec::<u32>::new());
    }

    #[test]
    fn dense_lookalikes_are_scanned_in_bounded_time() {
        // The data is minimal valid paths back to back. The scan must not do
        // a full lookbehind for each one, so 200,000 of them finish in time.
        let record = b"PTH0\x00\x00\x00\x05\x00\x01\x00\x01\x00";
        let data = record.repeat(200_000);
        let started = std::time::Instant::now();
        assert_eq!(scan(&data).len(), 200_000);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));

        // Each signature declares a huge body made of tiny components. Every
        // parse fails late, and the work budget ends the scan early.
        let mut hostile = Vec::new();
        for _ in 0..20_000 {
            hostile.extend_from_slice(b"PTH0\x00\x00\xff\x00\x00\x01\xff\xff");
            hostile.extend_from_slice(&[0u8; 20]);
        }
        let started = std::time::Instant::now();
        let _ = scan(&hostile);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn skips_empty_paths_and_lookalikes() {
        let mut data = b"\x00\x01BDHP\x00\x00\x00\x00\x00\x03".to_vec();
        data.extend_from_slice(b"BSVR\x00\x00\x00\x00PTH0\x00\x00\x00\x04\x00\x01\x00\x00");
        data.extend_from_slice(b"\x08PTH test");
        assert!(
            scan(&data).is_empty(),
            "expected nothing, got {:?}",
            scan(&data)
        );
    }

    #[test]
    fn path_without_recognizable_prefix_is_still_reported() {
        let data = b"\xff\xffPTH0\x00\x00\x00\x09\x00\x01\x00\x01\x04a.vi";
        let links = scan(data);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, None);
        assert!(
            links[0].qualified_name.is_empty(),
            "expected nothing, got {:?}",
            links[0].qualified_name
        );
        assert_eq!(links[0].name(), Some("a.vi"));
    }
}
