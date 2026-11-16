# buzzard-mcp

The Model Context Protocol server of
[buzzard](https://github.com/jpoehnelt/buzzard), a static analyzer for
LabVIEW projects. It offers buzzard's analysis to AI agents as read-only
tools over MCP's stdio transport.

**This crate is an internal part of the buzzard tool. Its API may change in
any 0.x release, including patch releases.** It is published so that
`cargo install buzzard-cli` works. To use the server, run `buzzard mcp`; the
tool names and their argument schemas are the supported interface.

```rust
use buzzard_mcp::Server;

let mut server = Server::new(std::env::temp_dir());
let reply = server
    .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"example","version":"0"}}}"#)
    .expect("a request gets a response");
assert!(reply.contains("serverInfo"));
```

## Licence

Apache-2.0. See `LICENSE` and `NOTICE` in this package.

LabVIEW and National Instruments are trademarks of National Instruments
Corporation. buzzard is an independent project and is not affiliated with,
endorsed by, or sponsored by it; the names are used only to identify the
file formats that buzzard reads.
