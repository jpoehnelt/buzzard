//! The trait and the default backend that inspect one file.

use std::path::Path;

use async_trait::async_trait;
use buzzard_rsrc::{BlockDiagram, RsrcFile, ViMetadata};

/// Everything a backend reports about one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspection {
    /// Version, flags, links, and other facts decoded from the file.
    pub metadata: ViMetadata,
    /// `None` when the file has no block diagram, or when the diagram
    /// could not be decoded (the metadata is still usable then).
    pub diagram: Option<BlockDiagram>,
}

impl Inspection {
    /// Parse a binary file from disk, reading and decoding it once. `path`
    /// may name a member of an `.llb` archive, as in `util.llb/Helper.vi`
    /// (see [`crate::archive`]).
    ///
    /// # Errors
    ///
    /// Returns the reason as text when the file cannot be read, is too
    /// large, or does not parse (see [`Inspection::from_bytes`]), or when
    /// the archive of a member cannot be read or has no such member.
    pub fn from_path(path: &Path) -> Result<Self, String> {
        Self::from_bytes(&crate::archive::read(path)?)
    }

    /// Parse a binary file that is already in memory.
    ///
    /// # Errors
    ///
    /// Returns the reason as text when the bytes are not a valid RSRC
    /// container or its metadata cannot be decoded. A block diagram that
    /// cannot be decoded is not an error. [`Inspection::diagram`] is `None`
    /// then.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let file = RsrcFile::parse(bytes).map_err(|e| e.to_string())?;
        let metadata = ViMetadata::from_rsrc(&file).map_err(|e| e.to_string())?;
        let diagram = BlockDiagram::from_rsrc(&file).ok().flatten();
        Ok(Self { metadata, diagram })
    }
}

/// A source of [`Inspection`]s. The default backend parses files from disk.
/// Another backend, for example one that queries a running editor, can
/// replace it without a change to the lint or language server code.
#[async_trait]
pub trait ViInspector: Send + Sync {
    /// Inspect the binary file at `path`.
    ///
    /// # Errors
    ///
    /// Returns a reason, meant for the user, when the file cannot be read
    /// or parsed.
    async fn inspect(&self, path: &Path) -> Result<Inspection, String>;
}

/// The default backend. It parses the binary file and needs no running
/// editor.
#[derive(Debug, Default, Clone, Copy)]
pub struct RsrcInspector;

#[async_trait]
impl ViInspector for RsrcInspector {
    async fn inspect(&self, path: &Path) -> Result<Inspection, String> {
        Inspection::from_path(path)
    }
}
