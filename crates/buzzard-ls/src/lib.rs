//! Language Server Protocol implementation.
//!
//! The analyzed files are binary and are never edited through the editor, so
//! the server does not use text synchronization. It watches the file system
//! instead and re-parses files when the IDE that owns them saves.

pub mod engine;
pub mod fuzzy;
mod server;

pub use server::run_stdio;
