# buzzard-ls

`buzzard-ls` is the language server of
[buzzard](https://github.com/jpoehnelt/buzzard), a static analyzer for
LabVIEW files. The server speaks the Language Server Protocol over stdio
and watches the file system. One worker task handles every change, so the
server publishes diagnostics in order.

## Stability

This crate is an internal part of the buzzard tool, published so that
`cargo install buzzard-cli` works. Its API may change in any 0.x release,
patch releases included. The supported interface is the protocol that
`buzzard lsp` serves.

## Example

This example runs the server, as `buzzard lsp` does.

```rust,no_run
let runtime = tokio::runtime::Runtime::new().expect("the runtime starts");
let status = runtime.block_on(buzzard_ls::run_stdio());
std::process::exit(status);
```

## License

The license is Apache-2.0. The `LICENSE` and `NOTICE` files of this package
hold the texts.

LabVIEW, NI, and National Instruments are trademarks of National Instruments
Corporation, used only to identify the file formats that buzzard reads.
buzzard is not affiliated with, endorsed by, or sponsored by that company.
