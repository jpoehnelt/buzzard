//! A Model Context Protocol server that offers buzzard's analysis as tools
//! for AI agents.
//!
//! The transport is MCP's stdio transport, which is newline-delimited
//! JSON-RPC 2.0 with one message per line. Standard output carries protocol
//! messages only. The protocol layer is written by hand on `serde_json` so
//! that any reader and writer can drive it, which is how the tests run it.
//!
//! The server is read-only. No tool writes, creates, or modifies a file.

//!
//! The reader of every result is a language model, so each result has a
//! byte budget (`bound`), each failure has a kind
//! ([`buzzard_lint::ErrorKind`]), and each tool declares the shape of its
//! `structuredContent` (`schema`). The guide to the tools is served as an
//! MCP prompt (`prompts`).

mod bound;
mod prompts;
mod protocol;
mod schema;
mod tools;

use std::path::PathBuf;

use buzzard_lint::{ErrorKind, Failure};

pub use prompts::GUIDE;
pub use protocol::{Server, MAX_LINE_BYTES, SUPPORTED_PROTOCOL_VERSIONS};

/// Serve on standard input and output until the client closes the pipe.
/// Paths in tool arguments are resolved against the current directory.
///
/// With `roots`, the tools read only inside those directories
/// ([`Server::confined`]). Without any, a path argument may name any file
/// that the process can read.
///
/// # Errors
///
/// Returns a [`Failure`] of the kind `bad_argument` when a root is not an
/// existing directory, and one of the kind `internal` when the current
/// directory cannot be determined, or when reading standard input or
/// writing standard output fails.
pub fn run_stdio(roots: &[PathBuf]) -> Result<(), Failure> {
    let transport = |err: std::io::Error| {
        Failure::new(ErrorKind::Internal, format!("mcp transport failed: {err}"))
    };
    let base = std::env::current_dir().map_err(transport)?;
    let mut server = if roots.is_empty() {
        Server::new(base)
    } else {
        Server::confined(&base, roots)?
    };
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    server.serve(stdin, stdout).map_err(transport)
}

/// Compiles the examples of the crate's `README.md` as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
