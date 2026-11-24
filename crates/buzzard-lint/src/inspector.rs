//! The trait and the default backend that inspect one file.

use std::path::Path;

use async_trait::async_trait;
use buzzard_rsrc::{BlockDiagram, RsrcFile, ViMetadata};
use serde::Serialize;

use crate::summary::DiagramOutline;

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

/// The JSON shape of `buzzard inspect --format json`: the fields of the
/// metadata, then `diagram` with the whole block diagram or `null`. See
/// [`Inspection::full`].
#[derive(Debug, Clone, Copy, Serialize)]
pub struct InspectionJson<'a> {
    #[serde(flatten)]
    metadata: &'a ViMetadata,
    diagram: Option<&'a BlockDiagram>,
}

/// An [`Inspection`] with the outline of the block diagram in place of
/// the diagram. See [`Inspection::summary`].
///
/// As JSON it has the fields of the metadata, as the whole JSON has
/// them, and then `detail`, `full_json_bytes`, and `diagram`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InspectionSummary {
    /// Version, flags, links, and other facts decoded from the file.
    #[serde(flatten)]
    pub metadata: ViMetadata,
    /// Always `summary`. It tells this shape from the whole JSON.
    pub detail: &'static str,
    /// The size in bytes of the whole JSON
    /// ([`Inspection::full_json_bytes`]), which this summary stands for.
    pub full_json_bytes: u64,
    /// The outline of the block diagram, or `None` where
    /// [`Inspection::diagram`] is `None`.
    pub diagram: Option<DiagramOutline>,
}

impl Inspection {
    /// The inspection as `buzzard inspect --format json` prints it, for
    /// a serializer.
    pub fn full(&self) -> InspectionJson<'_> {
        InspectionJson {
            metadata: &self.metadata,
            diagram: self.diagram.as_ref(),
        }
    }

    /// The text of `buzzard inspect --format json`: [`Inspection::full`]
    /// as indented JSON that ends in a line break.
    #[expect(
        clippy::missing_panics_doc,
        reason = "the metadata and the diagram hold only strings, numbers, and lists"
    )]
    pub fn full_json(&self) -> String {
        serde_json::to_string_pretty(&self.full()).expect("an inspection always serializes") + "\n"
    }

    /// The length of [`Inspection::full_json`] in bytes. The text is not
    /// built, so a caller can decide whether to ask for it.
    pub fn full_json_bytes(&self) -> u64 {
        // One more byte for the line break at the end.
        crate::budget::json_bytes(&self.full(), true).saturating_add(1)
    }

    /// The inspection without the nodes, terminals, and wires of the
    /// block diagram: the metadata, the size of the whole JSON, and the
    /// outline of the diagram ([`crate::summary::outline`]).
    pub fn summary(&self) -> InspectionSummary {
        InspectionSummary {
            metadata: self.metadata.clone(),
            detail: "summary",
            full_json_bytes: self.full_json_bytes(),
            diagram: self.diagram.as_ref().map(crate::summary::outline),
        }
    }
}

impl InspectionSummary {
    /// The summary as indented JSON that ends in a line break.
    #[expect(
        clippy::missing_panics_doc,
        reason = "the metadata and the outline hold only strings, numbers, and lists"
    )]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a summary always serializes") + "\n"
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
