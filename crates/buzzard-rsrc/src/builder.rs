//! Writer for synthetic RSRC files, used to build test inputs without the
//! editor. It produces the same layout the parser reads.
//!
//! This is test support, and it is public on purpose: the other crates of
//! buzzard build their test files with it, and a program built on this
//! crate can do the same. It writes the resources buzzard's tests need
//! (any raw resource, a save record, link lists, and with [`LlbBuilder`]
//! an archive of such files); it is not a general writer and cannot
//! produce a file the editor would open.

use std::io::Write as _;

use flate2::{write::ZlibEncoder, Compression};

use crate::blocks::path::{LvPath, PathKind};
use crate::fourcc::FourCc;
use crate::header::{file_type, RsrcHeader, FORMAT_VERSION, HEADER_SIZE};

const INFO_HEADER_SIZE: usize = 20;

#[derive(Debug)]
struct Entry {
    ty: FourCc,
    id: i32,
    name: Option<String>,
    data: Vec<u8>,
}

/// A link given as raw bytes: `(name, path kind, path components)`.
pub type ByteLink<'a> = (&'a [u8], PathKind, &'a [&'a [u8]]);

/// Collects resources and lays them out as an RSRC file with
/// [`build`](Self::build).
#[derive(Debug)]
pub struct RsrcBuilder {
    file_type: FourCc,
    entries: Vec<Entry>,
}

impl Default for RsrcBuilder {
    fn default() -> Self {
        Self::new(file_type::VI)
    }
}

impl RsrcBuilder {
    /// Start an empty file of the given type, e.g. [`file_type::CONTROL`].
    pub fn new(file_type: FourCc) -> Self {
        Self {
            file_type,
            entries: Vec::new(),
        }
    }

    /// Add a resource. Resources of one type are stored together, in the
    /// order they were added.
    #[must_use]
    pub fn resource(
        mut self,
        ty: &[u8; 4],
        id: i32,
        name: Option<&str>,
        data: impl Into<Vec<u8>>,
    ) -> Self {
        self.entries.push(Entry {
            ty: FourCc(*ty),
            id,
            name: name.map(str::to_owned),
            data: data.into(),
        });
        self
    }

    /// Add `vers` and `LVSR` blocks for a packed version and flag words.
    #[must_use]
    pub fn save_record(self, version: u32, words: &[u32]) -> Self {
        let mut lvsr = version.to_be_bytes().to_vec();
        for w in words {
            lvsr.extend_from_slice(&w.to_be_bytes());
        }
        let mut vers = version.to_be_bytes().to_vec();
        vers.extend_from_slice(&[0, 0, 0, 0]);
        self.resource(b"LVSR", 0, None, lvsr)
            .resource(b"vers", 4, None, vers)
    }

    /// Add an `LIvi` block linking to the given `(name, path)` pairs as subVIs.
    ///
    /// # Panics
    ///
    /// If a name or path component is longer than 255 bytes, or a path has
    /// more than 65,535 components.
    #[must_use]
    pub fn links(self, links: &[(&str, LvPath)]) -> Self {
        let encoded: Vec<(&[u8], Vec<u8>)> = links
            .iter()
            .map(|(name, path)| (name.as_bytes(), encode_pth0(path)))
            .collect();
        self.encoded_links(&encoded)
    }

    /// Like [`links`](Self::links), with names and path components given as
    /// raw bytes: `(name, kind, components)`. Old link forms store text in
    /// the code page of the machine that saved the file, which a `&str`
    /// cannot express.
    ///
    /// # Panics
    ///
    /// If a name or path component is longer than 255 bytes, or a path has
    /// more than 65,535 components.
    #[must_use]
    pub fn links_bytes(self, links: &[ByteLink<'_>]) -> Self {
        let encoded: Vec<(&[u8], Vec<u8>)> = links
            .iter()
            .map(|(name, kind, components)| (*name, encode_pth0_bytes(*kind, components)))
            .collect();
        self.encoded_links(&encoded)
    }

    fn encoded_links(self, links: &[(&[u8], Vec<u8>)]) -> Self {
        let mut data = vec![0, 1];
        data.extend_from_slice(b"LVIN");
        data.extend_from_slice(&u32_field(links.len()).to_be_bytes());
        data.extend_from_slice(&[0, 2]);
        for (name, path) in links {
            while data.len() % 4 != 0 {
                data.push(0);
            }
            data.extend_from_slice(b"VIVI");
            data.extend_from_slice(&1u32.to_be_bytes());
            push_pstr(&mut data, name);
            if data.len() % 2 != 0 {
                data.push(0);
            }
            data.extend_from_slice(path);
            data.extend_from_slice(&[0; 4]);
        }
        self.resource(b"LIvi", 0, None, data)
    }

    /// Lay out the header, data section and directory.
    ///
    /// # Panics
    ///
    /// If no resource was added, a resource name is longer than 255 bytes,
    /// or the file would not fit the 32-bit offsets of the format.
    pub fn build(self) -> Vec<u8> {
        // Group entries by type, preserving first-seen order.
        let mut types: Vec<(FourCc, Vec<&Entry>)> = Vec::new();
        for entry in &self.entries {
            match types.iter_mut().find(|(ty, _)| *ty == entry.ty) {
                Some((_, list)) => list.push(entry),
                None => types.push((entry.ty, vec![entry])),
            }
        }
        assert!(
            !types.is_empty(),
            "an RSRC file needs at least one resource"
        );

        let mut data = Vec::new();
        let mut names = Vec::new();
        let mut lists = Vec::new();
        let mut type_table = u32_field(types.len() - 1).to_be_bytes().to_vec();
        let lists_start = 4 + types.len() * 12;
        for (ty, list) in &types {
            type_table.extend_from_slice(ty.as_bytes());
            type_table.extend_from_slice(&u32_field(list.len() - 1).to_be_bytes());
            type_table.extend_from_slice(&u32_field(lists_start + lists.len()).to_be_bytes());
            for entry in list {
                let name_offset = match &entry.name {
                    Some(name) => {
                        let at = u32_field(names.len());
                        push_pstr(&mut names, name.as_bytes());
                        at
                    }
                    None => u32::MAX,
                };
                lists.extend_from_slice(&entry.id.to_be_bytes());
                lists.extend_from_slice(&name_offset.to_be_bytes());
                lists.extend_from_slice(&[0; 4]);
                lists.extend_from_slice(&u32_field(data.len()).to_be_bytes());
                lists.extend_from_slice(&[0; 4]);

                data.extend_from_slice(&u32_field(entry.data.len()).to_be_bytes());
                data.extend_from_slice(&entry.data);
                while data.len() % 4 != 0 {
                    data.push(0);
                }
            }
        }

        let types_offset = HEADER_SIZE + INFO_HEADER_SIZE;
        let names_offset = types_offset + type_table.len() + lists.len();
        let header = RsrcHeader {
            format_version: FORMAT_VERSION,
            file_type: self.file_type,
            creator: FourCc::new(b"LBVW"),
            info_offset: u32_field(HEADER_SIZE + data.len()),
            info_size: u32_field(names_offset + names.len()),
            data_offset: u32_field(HEADER_SIZE),
            data_size: u32_field(data.len()),
        };

        let mut out = header.to_bytes().to_vec();
        out.extend(data);
        out.extend_from_slice(&header.to_bytes());
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&u32_field(HEADER_SIZE).to_be_bytes());
        out.extend_from_slice(&u32_field(types_offset).to_be_bytes());
        out.extend_from_slice(&u32_field(names_offset).to_be_bytes());
        out.extend(type_table);
        out.extend(lists);
        out.extend(names);
        out
    }
}

/// One entry of an archive directory for [`encode_adir`]: the id, the
/// file type, the packed version and the name as raw bytes.
pub type AdirEntry<'a> = (i32, FourCc, u32, &'a [u8]);

/// Encode the `ADir` directory of an LLB archive (see [`crate::llb`]).
///
/// # Panics
///
/// If a name is longer than 255 bytes.
pub fn encode_adir(entries: &[AdirEntry<'_>]) -> Vec<u8> {
    let mut out = vec![0; 4];
    out.extend_from_slice(b"adir");
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&u32_field(entries.len()).to_be_bytes());
    for (id, file_type, version, name) in entries {
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(file_type.as_bytes());
        out.extend_from_slice(&version.to_be_bytes());
        out.extend_from_slice(&[0; 20]);
        push_pstr(&mut out, name);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
    }
    out
}

#[derive(Debug)]
struct LlbEntry {
    name: Vec<u8>,
    data: Vec<u8>,
    compressed: bool,
}

/// Collects files and lays them out as an LLB archive with
/// [`build`](Self::build): an `ADir` directory and one content resource
/// per member, numbered from 1 in the order they were added.
///
/// Like [`RsrcBuilder`] this is test support. The archive has none of the
/// palette and icon resources the editor writes.
#[derive(Debug, Default)]
pub struct LlbBuilder {
    version: u32,
    entries: Vec<LlbEntry>,
}

impl LlbBuilder {
    /// The packed version the directory states for every member, e.g.
    /// `0x1700_8000`. Zero, the default, states none.
    #[must_use]
    pub fn version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    /// Add a member that is stored as it is (`UCRF`).
    #[must_use]
    pub fn member(self, name: &str, data: &[u8]) -> Self {
        self.member_bytes(name.as_bytes(), data)
    }

    /// Like [`member`](Self::member), with the name given as raw bytes:
    /// names are text of the saving machine's code page.
    #[must_use]
    pub fn member_bytes(mut self, name: &[u8], data: &[u8]) -> Self {
        self.entries.push(LlbEntry {
            name: name.to_vec(),
            data: data.to_vec(),
            compressed: false,
        });
        self
    }

    /// Add a member that is stored as a zlib stream (`ZCRF`).
    #[must_use]
    pub fn compressed(mut self, name: &str, data: &[u8]) -> Self {
        self.entries.push(LlbEntry {
            name: name.as_bytes().to_vec(),
            data: data.to_vec(),
            compressed: true,
        });
        self
    }

    /// Lay out the archive.
    ///
    /// # Panics
    ///
    /// If a name is longer than 255 bytes, there are more than `i32::MAX`
    /// members, or the file would not fit the 32-bit offsets of the
    /// format.
    pub fn build(self) -> Vec<u8> {
        let ids: Vec<i32> = (1..=self.entries.len())
            .map(|id| i32::try_from(id).expect("an archive holds at most i32::MAX members"))
            .collect();
        let directory: Vec<AdirEntry<'_>> = self
            .entries
            .iter()
            .zip(&ids)
            .map(|(entry, &id)| {
                // The directory states the type the member's own header has.
                let file_type = RsrcHeader::parse(&entry.data)
                    .map_or(FourCc::new(b"\0\0\0\0"), |header| header.file_type);
                (id, file_type, self.version, entry.name.as_slice())
            })
            .collect();
        let mut builder = RsrcBuilder::new(file_type::LIBRARY).resource(
            b"ADir",
            0,
            None,
            encode_adir(&directory),
        );
        for (entry, &id) in self.entries.iter().zip(&ids) {
            let name = String::from_utf8_lossy(&entry.name);
            // A resource name is a Pascal string; the directory is what
            // the reader takes names from.
            let name = (name.len() <= 255).then_some(&*name);
            builder = if entry.compressed {
                let size = u32_field(entry.data.len()).to_be_bytes().to_vec();
                let mut encoder = ZlibEncoder::new(size, Compression::default());
                encoder
                    .write_all(&entry.data)
                    .expect("writing to a vector cannot fail");
                let stream = encoder.finish().expect("writing to a vector cannot fail");
                builder.resource(b"ZCRF", id, name, stream)
            } else {
                builder.resource(b"UCRF", id, name, entry.data.clone())
            };
        }
        builder.build()
    }
}

/// A length or offset narrowed to a 32-bit field of the format.
fn u32_field(value: usize) -> u32 {
    u32::try_from(value)
        .expect("RSRC lengths and offsets are 32-bit, so a file must stay under 4 GiB")
}

/// Append `text` as a Pascal string with a one-byte length prefix.
fn push_pstr(out: &mut Vec<u8>, text: &[u8]) {
    out.push(u8::try_from(text.len()).expect("a Pascal string holds at most 255 bytes"));
    out.extend_from_slice(text);
}

/// Encode a path in the `PTH0` form.
///
/// # Panics
///
/// If the path has more than 65,535 components or a component is longer
/// than 255 bytes.
pub fn encode_pth0(path: &LvPath) -> Vec<u8> {
    let components: Vec<&[u8]> = path.components.iter().map(String::as_bytes).collect();
    encode_pth0_bytes(path.kind, &components)
}

/// Encode a path in the `PTH0` form from raw component bytes.
///
/// # Panics
///
/// If there are more than 65,535 components or a component is longer than
/// 255 bytes.
pub fn encode_pth0_bytes(kind: PathKind, components: &[&[u8]]) -> Vec<u8> {
    let kind: u16 = match kind {
        PathKind::Absolute => 0,
        PathKind::Relative => 1,
        PathKind::NotAPath => 2,
        PathKind::Unc => 3,
    };
    let mut body = kind.to_be_bytes().to_vec();
    let count =
        u16::try_from(components.len()).expect("a `PTH0` path holds at most 65,535 components");
    body.extend_from_slice(&count.to_be_bytes());
    for component in components {
        push_pstr(&mut body, component);
    }
    let mut out = b"PTH0".to_vec();
    out.extend_from_slice(&u32_field(body.len()).to_be_bytes());
    out.extend(body);
    out
}

impl LvPath {
    /// Relative path to a sibling of the referencing file, e.g. `../a.vi`.
    pub fn sibling(name: &str) -> Self {
        Self {
            kind: PathKind::Relative,
            components: vec![String::new(), name.to_owned()],
        }
    }

    /// Absolute path from its components, e.g. `["C", "x", "a.vi"]` for
    /// `C:/x/a.vi`.
    pub fn absolute(components: &[&str]) -> Self {
        Self {
            kind: PathKind::Absolute,
            components: components.iter().map(|c| (*c).to_owned()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RsrcFile, ViMetadata};

    #[test]
    fn built_file_round_trips_through_parser() {
        let bytes = RsrcBuilder::default()
            .resource(b"LVSR", 0, Some("a.vi"), vec![0x21, 0, 0x80, 0, 0, 0, 0, 0])
            .resource(
                b"vers",
                4,
                None,
                b"\x21\x00\x80\x00\x00\x00\x00\x00".to_vec(),
            )
            .resource(
                b"vers",
                7,
                None,
                b"\x23\x00\x80\x00\x00\x00\x00\x00".to_vec(),
            )
            .resource(b"ODD!", -3, Some("odd"), vec![1, 2, 3])
            .build();
        let file = RsrcFile::parse(&bytes).unwrap();
        assert_eq!(file.resources().len(), 4);
        assert_eq!(file.types().len(), 3);
        assert_eq!(file.of_type(b"vers").count(), 2);
        let odd = file.of_type(b"ODD!").next().unwrap();
        assert_eq!(
            (odd.id, odd.name.as_deref(), odd.data),
            (-3, Some("odd"), &[1u8, 2, 3][..])
        );
        assert_eq!(file.resources()[0].name.as_deref(), Some("a.vi"));
    }

    #[test]
    fn built_links_are_extracted() {
        let bytes = RsrcBuilder::default()
            .save_record(0x2300_8000, &[0, 0x400])
            .links(&[
                ("b.vi", LvPath::sibling("b.vi")),
                ("odd name.vi", LvPath::absolute(&["C", "x", "odd name.vi"])),
            ])
            .build();
        let meta = ViMetadata::from_bytes(&bytes).unwrap();
        assert_eq!(meta.version.unwrap().to_string(), "23.0.0");
        assert_eq!(meta.flags.unwrap().separate_compiled_code, Some(true));
        let names: Vec<_> = meta.links.iter().map(|l| l.name().unwrap()).collect();
        assert_eq!(names, ["b.vi", "odd name.vi"]);
        assert!(meta
            .links
            .iter()
            .all(|l| l.link_type == Some(FourCc::new(b"VIVI"))));
    }

    #[test]
    fn built_byte_links_keep_their_bytes() {
        // Shift-JIS bytes are not UTF-8, so the reader shows one character
        // per byte, which loses nothing.
        let name: &[u8] = b"\x83\x65\x83\x58\x83\x67.vi";
        let bytes = RsrcBuilder::default()
            .save_record(0x2300_8000, &[0, 0x400])
            .links_bytes(&[(name, PathKind::Relative, &[b"", name])])
            .build();
        let meta = ViMetadata::from_bytes(&bytes).unwrap();
        let expected: String = name.iter().map(|&b| char::from(b)).collect();
        assert_eq!(meta.links.len(), 1);
        assert_eq!(meta.links[0].name(), Some(expected.as_str()));
        assert_eq!(meta.links[0].path.kind, PathKind::Relative);
        assert_eq!(meta.links[0].path.components, ["", expected.as_str()]);
    }
}
