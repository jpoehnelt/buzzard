//! A Model Context Protocol server that offers buzzard's analysis as tools
//! for AI agents.
//!
//! The transport is MCP's stdio transport, which is newline-delimited
//! JSON-RPC 2.0 with one message per line. Standard output carries protocol
//! messages only. The protocol layer is written by hand on `serde_json` so
//! that any reader and writer can drive it, which is how the tests run it.
//!
//! The server is read-only. No tool writes, creates, or modifies a file.

mod protocol;
mod tools;

pub use protocol::{Server, MAX_LINE_BYTES, SUPPORTED_PROTOCOL_VERSIONS};

/// Serve on standard input and output until the client closes the pipe.
/// Paths in tool arguments are resolved against the current directory.
///
/// # Errors
///
/// Returns an error if the current directory cannot be determined, or if
/// reading standard input or writing standard output fails.
pub fn run_stdio() -> std::io::Result<()> {
    let base = std::env::current_dir()?;
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    Server::new(base).serve(stdin, stdout)
}

/// Compiles the examples of the crate's `README.md` as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
