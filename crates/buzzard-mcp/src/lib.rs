//! Model Context Protocol server: buzzard's analysis as tools for AI agents.
//!
//! The transport is MCP's stdio transport: newline-delimited JSON-RPC 2.0,
//! one message per line, with standard output carrying protocol messages
//! only. The protocol layer is hand-written on `serde_json` so that it can
//! be driven through any reader and writer, which is how the tests run it.
//!
//! The server is strictly read-only: no tool writes, creates or modifies a
//! file.

mod protocol;
mod tools;

pub use protocol::{Server, MAX_LINE_BYTES, SUPPORTED_PROTOCOL_VERSIONS};

/// Serve on standard input and output until the client closes the pipe.
/// Paths in tool arguments are resolved against the current directory.
pub fn run_stdio() -> std::io::Result<()> {
    let base = std::env::current_dir()?;
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    Server::new(base).serve(stdin, stdout)
}
