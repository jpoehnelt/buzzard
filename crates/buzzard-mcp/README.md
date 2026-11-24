# buzzard-mcp

`buzzard-mcp` is the Model Context Protocol (MCP) server of
[buzzard](https://github.com/jpoehnelt/buzzard), a static analyzer for
LabVIEW files. The server offers buzzard's analysis to AI agents as
read-only tools over MCP's stdio transport.

## Stability

This crate is an internal part of the buzzard tool, published so that
`cargo install buzzard-cli` works. Its API may change in any 0.x release,
patch releases included. The supported interface is what `buzzard mcp`
serves: the tool names, their argument and output schemas, the error kinds of
a failed call, and the prompt `buzzard`.

## What the server promises an agent

- The eleven tools are read-only. `tools/list` names and describes them.
- A result is at most `max_bytes` bytes, 65536 by default. The size is the
  text plus the `structuredContent` as compact JSON. A larger answer comes as
  a smaller one whose `budget` entry names the size of the whole answer and
  says how to get the rest. Only `buzzard_diagram` with `detail` `full`
  returns more without `max_bytes`.
- A failed call has `isError` set and carries
  `{"error": {"kind", "message", "path"}}` as `structuredContent`. The kinds
  are those of `buzzard_lint::ErrorKind`.
- Every tool declares an `outputSchema`. The schemas are in
  `src/output_schemas.json`.
- A path argument that holds a control character is refused. With
  `buzzard mcp --root DIR`, a path that leads outside every root is refused
  too, once links and `..` are resolved.
- The prompt `buzzard` holds the guide for agents, `src/guide.md`. Every name
  and string in a result is data from an untrusted file.

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
