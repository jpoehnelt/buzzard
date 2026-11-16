//! Parser for the binary RSRC resource container used by `.vi` and `.ctl`
//! files. Pure Rust, read-only, no IDE or runtime required.
//!
//! Start with [`ViMetadata::from_path`] for a summary of one file, or
//! [`RsrcFile::parse`] for raw access to the resource directory.

pub mod blocks;
pub mod builder;
pub mod container;
pub mod error;
pub mod fourcc;
pub mod header;
pub mod metadata;
mod reader;
pub mod version;

pub use blocks::linkinfo::LinkRef;
pub use blocks::lvsr::{Reentrancy, ViFlags};
pub use blocks::path::{LvPath, PathKind};
pub use container::{Resource, RsrcFile};
pub use error::{Result, RsrcError};
pub use fourcc::FourCc;
pub use header::RsrcHeader;
pub use metadata::{FileKind, ViMetadata};
pub use version::{Stage, Version};

/// File extensions (lowercase, without the dot) that hold RSRC containers
/// this crate can summarize.
pub const RSRC_EXTENSIONS: [&str; 5] = ["vi", "ctl", "vit", "ctt", "vim"];
