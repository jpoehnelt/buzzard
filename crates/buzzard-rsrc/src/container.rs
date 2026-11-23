//! The RSRC container: header plus the directory of typed resources.
//!
//! Layout (all integers big-endian):
//!
//! ```text
//! 0                 header (32 bytes)
//! data_offset       data section: for each resource, u32 size + payload
//! info_offset       header again (32 bytes)
//!   +32             info header: 3 x u32, u32 type-list offset, u32 name-table offset
//!   +type-list      u32 (type count - 1), then per type:
//!                     fourcc, u32 (resource count - 1), u32 offset of its entry list
//!                   per resource entry (20 bytes):
//!                     i32 id, u32 name offset (or 0xFFFFFFFF), u32, u32 data offset, u32
//!   +name-table     Pascal strings
//! ```

use crate::error::{Result, RsrcError};
use crate::fourcc::FourCc;
use crate::header::{RsrcHeader, HEADER_SIZE};
use crate::reader::{decode_text, Reader};

const NO_NAME: u32 = 0xFFFF_FFFF;
const TYPE_ENTRY_SIZE: usize = 12;
const RESOURCE_ENTRY_SIZE: usize = 20;

/// One resource (a "section" of a block type) borrowed from the file bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource<'a> {
    /// Block type, such as `LVSR` or `BDHb`.
    pub ty: FourCc,
    /// Resource id, which tells apart several resources of one type.
    pub id: i32,
    /// Name from the name table, if the entry has one.
    pub name: Option<String>,
    /// Payload, without its `u32` size prefix.
    pub data: &'a [u8],
}

/// A parsed RSRC container. Resource payloads borrow from the input buffer.
#[derive(Debug, Clone)]
pub struct RsrcFile<'a> {
    /// The header at offset 0, which parsing checks against its copy in the
    /// info section.
    pub header: RsrcHeader,
    resources: Vec<Resource<'a>>,
}

impl<'a> RsrcFile<'a> {
    /// Parse the container directory. Payloads are not copied or interpreted.
    ///
    /// # Errors
    ///
    /// Returns any error of [`RsrcHeader::parse`] or [`RsrcHeader::validate`].
    /// Returns [`RsrcError::HeaderMismatch`] if the copy of the header in the
    /// info section differs, and [`RsrcError::OutOfBounds`] if a directory
    /// list, name, or payload lies outside `bytes` or there are more directory
    /// entries than the info section has room for.
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        let header = RsrcHeader::parse(bytes)?;
        header.validate(bytes.len())?;

        let info = header.info_offset as usize;
        let second = bytes.get(info..).ok_or(RsrcError::HeaderMismatch)?;
        if RsrcHeader::parse(second).ok().as_ref() != Some(&header) {
            return Err(RsrcError::HeaderMismatch);
        }

        let mut r = Reader::at(bytes, info + HEADER_SIZE, "info header");
        r.take(12)?;
        let types_base = offset("type list", info, r.u32()?, bytes)?;
        let names_base = offset("name table", info, r.u32()?, bytes)?;

        let mut r = Reader::at(bytes, types_base, "type list");
        let type_count = count(r.u32()?);
        if type_count > (r.remaining() / TYPE_ENTRY_SIZE) as u64 {
            return Err(out_of_bounds("type list", types_base, type_count, bytes));
        }
        // Every resource needs its own directory entry inside the info
        // section. Without this cap, type entries that share one entry list
        // would turn a small file into billions of resources.
        let max_resources = (header.info_size as usize / RESOURCE_ENTRY_SIZE) as u64;

        let mut resources = Vec::new();
        for _ in 0..type_count {
            let ty = FourCc(r.fourcc()?);
            let entry_count = count(r.u32()?);
            let list_offset = offset("resource list", types_base, r.u32()?, bytes)?;

            let mut e = Reader::at(bytes, list_offset, "resource list");
            if entry_count > (e.remaining() / RESOURCE_ENTRY_SIZE) as u64
                || resources.len() as u64 + entry_count > max_resources
            {
                return Err(out_of_bounds(
                    "resource list",
                    list_offset,
                    entry_count,
                    bytes,
                ));
            }
            for _ in 0..entry_count {
                let id = e.u32()?.cast_signed();
                let name_offset = e.u32()?;
                e.u32()?;
                let data_offset = e.u32()?;
                e.u32()?;

                let name = if name_offset == NO_NAME {
                    None
                } else {
                    let at = offset("resource name", names_base, name_offset, bytes)?;
                    Some(decode_text(Reader::at(bytes, at, "resource name").pstr()?))
                };
                let at = offset(
                    "resource data",
                    header.data_offset as usize,
                    data_offset,
                    bytes,
                )?;
                let mut d = Reader::at(bytes, at, "resource data");
                let size = d.u32()? as usize;
                let data = d.take(size)?;
                resources.push(Resource { ty, id, name, data });
            }
        }

        Ok(Self { header, resources })
    }

    /// File type from the header, such as `LVIN` or `LVCC`.
    pub fn file_type(&self) -> FourCc {
        self.header.file_type
    }

    /// All resources in directory order.
    pub fn resources(&self) -> &[Resource<'a>] {
        &self.resources
    }

    /// All resources of one type.
    pub fn of_type<'s>(&'s self, ty: &[u8; 4]) -> impl Iterator<Item = &'s Resource<'a>> + 's {
        let ty = FourCc(*ty);
        self.resources.iter().filter(move |r| r.ty == ty)
    }

    /// Payload of the first resource of the given type.
    pub fn first(&self, ty: &[u8; 4]) -> Option<&'a [u8]> {
        self.of_type(ty).next().map(|r| r.data)
    }

    /// Distinct resource types in directory order.
    pub fn types(&self) -> Vec<FourCc> {
        let mut out: Vec<FourCc> = Vec::new();
        for r in &self.resources {
            if out.last() != Some(&r.ty) && !out.contains(&r.ty) {
                out.push(r.ty);
            }
        }
        out
    }
}

/// Counts are stored minus one.
fn count(raw: u32) -> u64 {
    u64::from(raw) + 1
}

/// `base + relative`, checked against the buffer so that later arithmetic
/// cannot overflow even where `usize` is 32 bits wide.
fn offset(what: &'static str, base: usize, relative: u32, bytes: &[u8]) -> Result<usize> {
    usize::try_from(relative)
        .ok()
        .and_then(|relative| base.checked_add(relative))
        .filter(|&at| at <= bytes.len())
        .ok_or(RsrcError::OutOfBounds {
            what,
            offset: base as u64 + u64::from(relative),
            len: 0,
            size: bytes.len(),
        })
}

fn out_of_bounds(what: &'static str, offset: usize, count: u64, bytes: &[u8]) -> RsrcError {
    RsrcError::OutOfBounds {
        what,
        offset: offset as u64,
        len: count,
        size: bytes.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::RsrcBuilder;

    fn u32_of(n: usize) -> u32 {
        u32::try_from(n).expect("test offsets and counts fit in a u32")
    }

    /// Many type entries that all point at one entry list must not multiply
    /// into more resources than the directory has room for.
    #[test]
    fn rejects_type_entries_sharing_one_resource_list() {
        const TYPES: usize = 2_000;
        const ENTRIES: usize = 2_000;
        let header_len = HEADER_SIZE;
        let lists_at = 4 + TYPES * TYPE_ENTRY_SIZE;
        let mut info = vec![0u8; header_len];
        info.extend_from_slice(&[0; 12]);
        info.extend_from_slice(&u32_of(header_len + 20).to_be_bytes());
        info.extend_from_slice(&0u32.to_be_bytes());
        info.extend_from_slice(&u32_of(TYPES - 1).to_be_bytes());
        for _ in 0..TYPES {
            info.extend_from_slice(b"AAAA");
            info.extend_from_slice(&u32_of(ENTRIES - 1).to_be_bytes());
            info.extend_from_slice(&u32_of(lists_at).to_be_bytes());
        }
        for _ in 0..ENTRIES {
            info.extend_from_slice(&0u32.to_be_bytes());
            info.extend_from_slice(&NO_NAME.to_be_bytes());
            info.extend_from_slice(&[0; 12]);
        }
        let data = [0u8; 4];
        let header = RsrcHeader {
            format_version: crate::header::FORMAT_VERSION,
            file_type: crate::header::file_type::VI,
            creator: FourCc::new(b"LBVW"),
            info_offset: u32_of(header_len + data.len()),
            info_size: u32_of(info.len()),
            data_offset: u32_of(header_len),
            data_size: u32_of(data.len()),
        };
        info[..header_len].copy_from_slice(&header.to_bytes());
        let mut bytes = header.to_bytes().to_vec();
        bytes.extend_from_slice(&data);
        bytes.extend(info);

        // 2,000 x 2,000 would be four million resources from a 64 KB file.
        assert!(bytes.len() < 70_000);
        assert!(matches!(
            RsrcFile::parse(&bytes),
            Err(RsrcError::OutOfBounds {
                what: "resource list",
                ..
            })
        ));
    }

    #[test]
    fn accepts_a_full_directory() {
        let mut builder = RsrcBuilder::default();
        for i in 0..300 {
            builder = builder.resource(b"DATA", i, None, vec![i.to_le_bytes()[0]]);
        }
        let bytes = builder.build();
        assert_eq!(RsrcFile::parse(&bytes).unwrap().resources().len(), 300);
    }
}
