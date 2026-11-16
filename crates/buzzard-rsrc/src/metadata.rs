//! Owned, serializable summary of everything the analyzer needs from a file.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde::Serialize;

use crate::blocks::bdpw::Bdpw;
use crate::blocks::libn;
use crate::blocks::linkinfo::{self, LinkRef};
use crate::blocks::lvsr::{Lvsr, ViFlags};
use crate::blocks::vers::Vers;
use crate::container::RsrcFile;
use crate::error::{Result, RsrcError};
use crate::fourcc::FourCc;
use crate::header::{file_type, RsrcHeader, HEADER_SIZE};
use crate::version::Version;

/// Largest file read into memory. Real files are kilobytes to a few
/// megabytes; the container format itself cannot exceed 4 GiB.
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

/// Read an RSRC file, checking its size and header before loading the rest,
/// so that a huge or unrelated file with a matching extension costs nothing.
///
/// # Errors
///
/// [`RsrcError::Io`] if the file cannot be read, [`RsrcError::TooLarge`] if
/// it is over [`MAX_FILE_BYTES`], and any error of [`RsrcHeader::parse`] if
/// it does not start with a valid header.
pub fn read_file(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len > MAX_FILE_BYTES {
        return Err(RsrcError::TooLarge(len, MAX_FILE_BYTES));
    }
    // Cannot fail: `MAX_FILE_BYTES` fits in a 32-bit `usize`.
    let size = usize::try_from(len).map_err(|_| RsrcError::TooLarge(len, MAX_FILE_BYTES))?;
    let mut bytes = vec![0; HEADER_SIZE.min(size)];
    file.read_exact(&mut bytes)?;
    RsrcHeader::parse(&bytes)?;
    bytes.reserve_exact(size - bytes.len());
    file.take(MAX_FILE_BYTES).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// What a file is, from the file type in its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    /// A VI (`LVIN`).
    Vi,
    /// A custom control or typedef (`LVCC`).
    Control,
    /// An LLB archive (`LVAR`).
    Library,
    /// Any other file type.
    Other,
}

/// Summary of one file: its kind, version, settings, owning libraries and
/// links to other files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViMetadata {
    /// What the file is, decided by [`file_type`](Self::file_type).
    pub kind: FileKind,
    /// File type code from the header, e.g. `LVIN`.
    pub file_type: FourCc,
    /// Name the file had when it was last saved.
    pub saved_name: Option<String>,
    /// Version of the editor that saved the file.
    pub version: Option<Version>,
    /// Settings from the `LVSR` save record; `None` when there is none.
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
    /// Summarize a file already read into memory.
    ///
    /// # Errors
    ///
    /// Any error of [`RsrcFile::parse`] or [`from_rsrc`](Self::from_rsrc).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::from_rsrc(&RsrcFile::parse(bytes)?)
    }

    /// Read and summarize a file.
    ///
    /// # Errors
    ///
    /// Any error of [`read_file`] or [`from_bytes`](Self::from_bytes).
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_bytes(&read_file(path)?)
    }

    /// Summarize an already parsed container.
    ///
    /// # Errors
    ///
    /// If its `LVSR`, `vers`, `BDPW` or `LIBN` block is malformed. Link info
    /// blocks never cause an error: links that do not parse are skipped.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_file_rejects_non_rsrc_content_from_the_header_alone() {
        let dir = std::env::temp_dir().join(format!("buzzard-rsrc-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let text = dir.join("notes.vi");
        std::fs::write(&text, vec![b'x'; 100_000]).unwrap();
        assert!(matches!(read_file(&text), Err(RsrcError::BadSignature)));

        let short = dir.join("short.vi");
        std::fs::write(&short, b"RSRC").unwrap();
        assert!(matches!(read_file(&short), Err(RsrcError::TooSmall(4))));

        assert!(matches!(
            read_file(dir.join("absent.vi")),
            Err(RsrcError::Io(_))
        ));

        let real = crate::builder::RsrcBuilder::default()
            .save_record(0x2300_8000, &[0, 0x400])
            .build();
        let good = dir.join("good.vi");
        std::fs::write(&good, &real).unwrap();
        assert_eq!(read_file(&good).unwrap(), real);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
