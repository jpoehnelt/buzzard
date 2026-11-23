# buzzard-mcp

`buzzard-mcp` is the Model Context Protocol (MCP) server of
[buzzard](https://github.com/jpoehnelt/buzzard), a static analyzer for
LabVIEW files. The server offers buzzard's analysis to AI agents as
read-only tools over MCP's stdio transport.

## Stability

This crate is an internal part of the buzzard tool, published so that
`cargo install buzzard-cli` works. Its API may change in any 0.x release,
patch releases included. The supported interface is the tool names and
argument schemas that `buzzard mcp` serves.

## Example

This example sends one `initialize` request to a server.

```rust
use buzzard_mcp::Server;

let mut server = Server::new(std::env::temp_dir());
let reply = server
    .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"example","version":"0"}}}"#)
    .expect("a request gets a response");
assert!(reply.contains("serverInfo"));
```

## License

The license is Apache-2.0. The `LICENSE` and `NOTICE` files of this package
hold the texts.

LabVIEW, NI, and National Instruments are trademarks of National Instruments
Corporation, used only to identify the file formats that buzzard reads.
buzzard is not affiliated with, endorsed by, or sponsored by that company.
