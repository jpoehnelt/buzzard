# buzzard-mcp

`buzzard-mcp` is the Model Context Protocol (MCP) server of
[buzzard](https://github.com/jpoehnelt/buzzard), a static analyzer for
LabVIEW files. The server offers buzzard's analysis to AI agents as
read-only tools over MCP's stdio transport. An agent's client starts it as
`buzzard mcp`, which the `buzzard-cli` crate installs.

## Stability

This crate is an internal part of the buzzard tool, published so that
`cargo install buzzard-cli` works. Its API may change in any 0.x release,
patch releases included. The supported interface is what `buzzard mcp`
serves: the tool names, their argument and output schemas, the error kinds of
a failed call, and the prompt `buzzard`.

## What the server promises an agent

- The eleven tools are read-only. `tools/list` names and describes them.
- A result is at most `max_bytes` bytes, 65536 by default. A call that
  passes less than 1024 fails with `bad_argument`. The size is the text plus
  the `structuredContent` as compact JSON.
  A larger answer comes as a smaller one with a `budget` entry. That entry
  holds `max_bytes`, `full_bytes` for the size of the whole answer, and
  `more`, which says how to get the rest. The one result without a bound is
  `buzzard_diagram` with `detail` `full` and no `max_bytes`. A failed call is
  inside the budget too.
- `buzzard_diagram` returns a whole diagram in format `json` as the diagram
  itself, with the one key `root`. A diagram or a frame that is over the
  budget comes as its outline.
- A failed call has `isError` set and carries
  `{"error": {"kind", "message", "path"}}` as `structuredContent`, without
  `path` when the error is about no path. The kinds are those of
  `buzzard_lint::ErrorKind`, and `buzzard --help` lists them.
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
