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
    pub ty: FourCc,
    pub id: i32,
    pub name: Option<String>,
    pub data: &'a [u8],
}

/// A parsed RSRC container. Resource payloads borrow from the input buffer.
#[derive(Debug, Clone)]
pub struct RsrcFile<'a> {
    pub header: RsrcHeader,
    resources: Vec<Resource<'a>>,
}

impl<'a> RsrcFile<'a> {
    /// Parse the container directory. Payloads are not copied or interpreted.
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
        let types_base = info + r.u32()? as usize;
        let names_base = info + r.u32()? as usize;

        let mut r = Reader::at(bytes, types_base, "type list");
        let type_count = r.u32()? as usize + 1;
        if type_count > r.remaining() / TYPE_ENTRY_SIZE {
            return Err(out_of_bounds("type list", types_base, type_count, bytes));
        }

        let mut resources = Vec::new();
        for _ in 0..type_count {
            let ty = FourCc(r.fourcc()?);
            let count = r.u32()? as usize + 1;
            let list_offset = types_base + r.u32()? as usize;

            let mut e = Reader::at(bytes, list_offset, "resource list");
            if count > e.remaining() / RESOURCE_ENTRY_SIZE {
                return Err(out_of_bounds("resource list", list_offset, count, bytes));
            }
            for _ in 0..count {
                let id = e.u32()? as i32;
                let name_offset = e.u32()?;
                e.u32()?;
                let data_offset = e.u32()?;
                e.u32()?;

                let name = if name_offset == NO_NAME {
                    None
                } else {
                    let at = names_base + name_offset as usize;
                    Some(decode_text(Reader::at(bytes, at, "resource name").pstr()?))
                };
                let at = header.data_offset as usize + data_offset as usize;
                let mut d = Reader::at(bytes, at, "resource data");
                let size = d.u32()? as usize;
                let data = d.take(size)?;
                resources.push(Resource { ty, id, name, data });
            }
        }

        Ok(Self { header, resources })
    }

    /// File type from the header, e.g. `LVIN` or `LVCC`.
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

fn out_of_bounds(what: &'static str, offset: usize, count: usize, bytes: &[u8]) -> RsrcError {
    RsrcError::OutOfBounds {
        what,
        offset: offset as u64,
        len: count as u64,
        size: bytes.len(),
    }
}
