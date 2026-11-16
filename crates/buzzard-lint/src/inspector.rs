//! Backend abstraction: how metadata is obtained for one file.

use std::path::Path;

use async_trait::async_trait;
use buzzard_rsrc::ViMetadata;

/// A source of [`ViMetadata`]. The default backend parses files from disk;
/// other backends (for example one that queries a running editor) can be
/// swapped in without touching the lint or language-server code.
#[async_trait]
pub trait ViInspector: Send + Sync {
    async fn inspect(&self, path: &Path) -> Result<ViMetadata, String>;
}

/// Headless backend that parses the binary file directly.
#[derive(Debug, Default, Clone, Copy)]
pub struct RsrcInspector;

#[async_trait]
impl ViInspector for RsrcInspector {
    async fn inspect(&self, path: &Path) -> Result<ViMetadata, String> {
        ViMetadata::from_path(path).map_err(|e| e.to_string())
    }
}
