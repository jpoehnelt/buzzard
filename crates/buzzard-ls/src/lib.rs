//! Language Server Protocol implementation.
//!
//! The analyzed files are binary and are never edited through the editor, so
//! the server does not use text synchronization. It watches the file system
//! instead and re-parses files when the IDE that owns them saves.
//!
//! Requests are answered from an immutable snapshot ([`engine::Snapshot`]);
//! all changes go through one worker task, which keeps the published
//! diagnostics in order.

pub mod engine;
pub mod fuzzy;
mod paths;
mod server;
mod worker;

pub use server::run_stdio;

/// Compiles the examples of the crate's `README.md` as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
