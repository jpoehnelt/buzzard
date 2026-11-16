//! Backend abstraction: how information is obtained for one file.

use std::path::Path;

use async_trait::async_trait;
use buzzard_rsrc::{BlockDiagram, RsrcFile, ViMetadata};

/// Everything a backend reports about one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspection {
    pub metadata: ViMetadata,
    /// `None` when the file has no block diagram, or when the diagram
    /// could not be decoded (the metadata is still usable then).
    pub diagram: Option<BlockDiagram>,
}

impl Inspection {
    /// Parse a binary file from disk, reading and decoding it once.
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let bytes = buzzard_rsrc::read_file(path).map_err(|e| e.to_string())?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let file = RsrcFile::parse(bytes).map_err(|e| e.to_string())?;
        let metadata = ViMetadata::from_rsrc(&file).map_err(|e| e.to_string())?;
        let diagram = BlockDiagram::from_rsrc(&file).ok().flatten();
        Ok(Self { metadata, diagram })
    }
}

/// A source of [`Inspection`]s. The default backend parses files from disk;
/// other backends (for example one that queries a running editor) can be
/// swapped in without touching the lint or language-server code.
#[async_trait]
pub trait ViInspector: Send + Sync {
    async fn inspect(&self, path: &Path) -> Result<Inspection, String>;
}

/// Headless backend that parses the binary file directly.
#[derive(Debug, Default, Clone, Copy)]
pub struct RsrcInspector;

#[async_trait]
impl ViInspector for RsrcInspector {
    async fn inspect(&self, path: &Path) -> Result<Inspection, String> {
        Inspection::from_path(path)
    }
}
