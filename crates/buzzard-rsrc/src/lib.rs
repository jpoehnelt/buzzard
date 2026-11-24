//! Parser for the binary RSRC resource container used by `.vi` and `.ctl`
//! files. The crate is pure Rust and read-only, and it needs no LabVIEW editor
//! or runtime.
//!
//! Start with [`ViMetadata::from_path`] for a summary of one file, or
//! [`RsrcFile::parse`] for raw access to the resource directory.
//! [`Llb::parse`] lists the members of an `.llb` archive and
//! [`Llb::read`] gives the bytes of one, which are an RSRC file again.

pub mod blocks;
pub mod builder;
pub mod container;
pub mod diagram;
pub mod error;
pub mod fourcc;
pub mod header;
pub mod llb;
pub mod metadata;
mod reader;
pub mod version;

pub use blocks::heap::{primitive_name, primitive_names, PrimitiveName};
pub use blocks::linkinfo::LinkRef;
pub use blocks::lvsr::{Reentrancy, ViFlags};
pub use blocks::path::{LvPath, PathKind};
pub use blocks::types::{DataType, NumericType, RefKind, TypeInfo, Value};
pub use container::{Resource, RsrcFile};
pub use diagram::{
    BlockDiagram, CaseSelector, Diagram, Direction, LoopInfo, Loss, Node, Rect, SelectorKind,
    SelectorMatch, ShiftRegister, StopTerminal, Terminal, Tunnel, TunnelMode, Wire,
};
pub use error::{Result, RsrcError};
pub use fourcc::FourCc;
pub use header::RsrcHeader;
pub use llb::{Llb, LlbMember, Storage};
pub use metadata::{read_file, FileKind, ViMetadata};
pub use version::{Stage, Version};

/// File extensions (lowercase, without the dot) that hold RSRC containers
/// this crate can summarize.
///
/// The constant is a slice so that a new extension does not change its type.
pub const RSRC_EXTENSIONS: &[&str] = &["vi", "ctl", "vit", "ctt", "vim"];

/// The file extension (lowercase, without the dot) of an LLB archive,
/// which [`Llb`] reads.
pub const LLB_EXTENSION: &str = "llb";

/// Compiles the examples of the crate's `README.md` as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
