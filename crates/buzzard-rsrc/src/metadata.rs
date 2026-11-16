//! Owned, serializable summary of everything the analyzer needs from a file.

use std::path::Path;

use serde::Serialize;

use crate::blocks::bdpw::Bdpw;
use crate::blocks::libn;
use crate::blocks::linkinfo::{self, LinkRef};
use crate::blocks::lvsr::{Lvsr, ViFlags};
use crate::blocks::vers::Vers;
use crate::container::RsrcFile;
use crate::error::Result;
use crate::fourcc::FourCc;
use crate::header::file_type;
use crate::version::Version;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Vi,
    Control,
    Library,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViMetadata {
    pub kind: FileKind,
    pub file_type: FourCc,
    /// Name the file had when it was last saved.
    pub saved_name: Option<String>,
    /// Version of the editor that saved the file.
    pub version: Option<Version>,
    pub flags: Option<ViFlags>,
    /// `None` when the file has no block diagram password record.
    pub password_protected: Option<bool>,
    /// Owning libraries, outermost first.
    pub libraries: Vec<String>,
    /// References to other files, sorted and deduplicated.
    pub links: Vec<LinkRef>,
    /// Resource types present, in directory order.
    pub resource_types: Vec<FourCc>,
}

impl ViMetadata {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::from_rsrc(&RsrcFile::parse(bytes)?)
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_bytes(&std::fs::read(path)?)
    }

    pub fn from_rsrc(file: &RsrcFile<'_>) -> Result<Self> {
        let save_record = file.of_type(b"LVSR").next();
        let lvsr = save_record.map(|r| Lvsr::parse(r.data)).transpose()?;

        // The lowest-numbered `vers` resource describes the file itself.
        let vers = file
            .of_type(b"vers")
            .min_by_key(|r| r.id)
            .map(|r| Vers::parse(r.data))
            .transpose()?;
        let version = vers
            .map(|v| v.version)
            .or_else(|| lvsr.as_ref().map(|l| l.version));

        let password_protected = file
            .first(b"BDPW")
            .map(|data| Bdpw::parse(data).map(|b| b.has_password()))
            .transpose()?;
        let libraries = file
            .first(b"LIBN")
            .map(libn::parse)
            .transpose()?
            .unwrap_or_default();

        let mut links: Vec<LinkRef> = linkinfo::LINK_BLOCKS
            .iter()
            .flat_map(|ty| file.of_type(ty))
            .flat_map(|r| linkinfo::scan(r.data))
            .collect();
        links.sort();
        links.dedup();

        let kind = match file.file_type() {
            t if t == file_type::VI => FileKind::Vi,
            t if t == file_type::CONTROL => FileKind::Control,
            t if t == file_type::LIBRARY => FileKind::Library,
            _ => FileKind::Other,
        };

        Ok(Self {
            kind,
            file_type: file.file_type(),
            saved_name: save_record.and_then(|r| r.name.clone()),
            version,
            flags: lvsr.as_ref().map(Lvsr::flags),
            password_protected,
            libraries,
            links,
            resource_types: file.types(),
        })
    }
}
