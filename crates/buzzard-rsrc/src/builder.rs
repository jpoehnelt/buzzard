//! Writer for synthetic RSRC files, used to build test inputs without the
//! editor. It produces the same layout the parser reads.

use crate::blocks::path::{LvPath, PathKind};
use crate::fourcc::FourCc;
use crate::header::{file_type, RsrcHeader, FORMAT_VERSION, HEADER_SIZE};

const INFO_HEADER_SIZE: usize = 20;

struct Entry {
    ty: FourCc,
    id: i32,
    name: Option<String>,
    data: Vec<u8>,
}

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
    pub fn new(file_type: FourCc) -> Self {
        Self { file_type, entries: Vec::new() }
    }

    pub fn resource(mut self, ty: &[u8; 4], id: i32, name: Option<&str>, data: impl Into<Vec<u8>>) -> Self {
        self.entries.push(Entry {
            ty: FourCc(*ty),
            id,
            name: name.map(str::to_owned),
            data: data.into(),
        });
        self
    }

    /// Add `vers` and `LVSR` blocks for a packed version and flag words.
    pub fn save_record(self, version: u32, words: &[u32]) -> Self {
        let mut lvsr = version.to_be_bytes().to_vec();
        for w in words {
            lvsr.extend_from_slice(&w.to_be_bytes());
        }
        let mut vers = version.to_be_bytes().to_vec();
        vers.extend_from_slice(&[0, 0, 0, 0]);
        self.resource(b"LVSR", 0, None, lvsr).resource(b"vers", 4, None, vers)
    }

    /// Add an `LIvi` block linking to the given `(name, path)` pairs as subVIs.
    pub fn links(self, links: &[(&str, LvPath)]) -> Self {
        let mut data = vec![0, 1];
        data.extend_from_slice(b"LVIN");
        data.extend_from_slice(&(links.len() as u32).to_be_bytes());
        data.extend_from_slice(&[0, 2]);
        for (name, path) in links {
            while data.len() % 4 != 0 {
                data.push(0);
            }
            data.extend_from_slice(b"VIVI");
            data.extend_from_slice(&1u32.to_be_bytes());
            data.push(name.len() as u8);
            data.extend_from_slice(name.as_bytes());
            if data.len() % 2 != 0 {
                data.push(0);
            }
            data.extend(encode_pth0(path));
            data.extend_from_slice(&[0; 4]);
        }
        self.resource(b"LIvi", 0, None, data)
    }

    pub fn build(self) -> Vec<u8> {
        // Group entries by type, preserving first-seen order.
        let mut types: Vec<(FourCc, Vec<&Entry>)> = Vec::new();
        for entry in &self.entries {
            match types.iter_mut().find(|(ty, _)| *ty == entry.ty) {
                Some((_, list)) => list.push(entry),
                None => types.push((entry.ty, vec![entry])),
            }
        }
        assert!(!types.is_empty(), "an RSRC file needs at least one resource");

        let mut data = Vec::new();
        let mut names = Vec::new();
        let mut lists = Vec::new();
        let mut type_table = ((types.len() - 1) as u32).to_be_bytes().to_vec();
        let lists_start = 4 + types.len() * 12;
        for (ty, list) in &types {
            type_table.extend_from_slice(ty.as_bytes());
            type_table.extend_from_slice(&((list.len() - 1) as u32).to_be_bytes());
            type_table.extend_from_slice(&((lists_start + lists.len()) as u32).to_be_bytes());
            for entry in list {
                let name_offset = match &entry.name {
                    Some(name) => {
                        let at = names.len() as u32;
                        names.push(name.len() as u8);
                        names.extend_from_slice(name.as_bytes());
                        at
                    }
                    None => u32::MAX,
                };
                lists.extend_from_slice(&entry.id.to_be_bytes());
                lists.extend_from_slice(&name_offset.to_be_bytes());
                lists.extend_from_slice(&[0; 4]);
                lists.extend_from_slice(&(data.len() as u32).to_be_bytes());
                lists.extend_from_slice(&[0; 4]);

                data.extend_from_slice(&(entry.data.len() as u32).to_be_bytes());
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
            info_offset: (HEADER_SIZE + data.len()) as u32,
            info_size: (names_offset + names.len()) as u32,
            data_offset: HEADER_SIZE as u32,
            data_size: data.len() as u32,
        };

        let mut out = header.to_bytes().to_vec();
        out.extend(data);
        out.extend_from_slice(&header.to_bytes());
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&(HEADER_SIZE as u32).to_be_bytes());
        out.extend_from_slice(&(types_offset as u32).to_be_bytes());
        out.extend_from_slice(&(names_offset as u32).to_be_bytes());
        out.extend(type_table);
        out.extend(lists);
        out.extend(names);
        out
    }
}

/// Encode a path in the `PTH0` form.
pub fn encode_pth0(path: &LvPath) -> Vec<u8> {
    let kind: u16 = match path.kind {
        PathKind::Absolute => 0,
        PathKind::Relative => 1,
        PathKind::NotAPath => 2,
        PathKind::Unc => 3,
    };
    let mut body = kind.to_be_bytes().to_vec();
    body.extend_from_slice(&(path.components.len() as u16).to_be_bytes());
    for component in &path.components {
        body.push(component.len() as u8);
        body.extend_from_slice(component.as_bytes());
    }
    let mut out = b"PTH0".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend(body);
    out
}

impl LvPath {
    /// Relative path to a sibling of the referencing file, e.g. `../a.vi`.
    pub fn sibling(name: &str) -> Self {
        Self { kind: PathKind::Relative, components: vec![String::new(), name.to_owned()] }
    }

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
            .resource(b"vers", 4, None, b"\x21\x00\x80\x00\x00\x00\x00\x00".to_vec())
            .resource(b"vers", 7, None, b"\x23\x00\x80\x00\x00\x00\x00\x00".to_vec())
            .resource(b"ODD!", -3, Some("odd"), vec![1, 2, 3])
            .build();
        let file = RsrcFile::parse(&bytes).unwrap();
        assert_eq!(file.resources().len(), 4);
        assert_eq!(file.types().len(), 3);
        assert_eq!(file.of_type(b"vers").count(), 2);
        let odd = file.of_type(b"ODD!").next().unwrap();
        assert_eq!((odd.id, odd.name.as_deref(), odd.data), (-3, Some("odd"), &[1u8, 2, 3][..]));
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
        assert!(meta.links.iter().all(|l| l.link_type == Some(FourCc::new(b"VIVI"))));
    }
}
