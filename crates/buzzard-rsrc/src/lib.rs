//! Parser for the binary RSRC resource container used by `.vi` and `.ctl`
//! files. Pure Rust, read-only, no IDE or runtime required.

pub mod container;
pub mod error;
pub mod fourcc;
pub mod header;
mod reader;

pub use container::{Resource, RsrcFile};
pub use error::{Result, RsrcError};
pub use fourcc::FourCc;
pub use header::RsrcHeader;
