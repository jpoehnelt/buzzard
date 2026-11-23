# buzzard-ls

The language server of [buzzard](https://github.com/jpoehnelt/buzzard), a
static analyzer for LabVIEW projects. It serves the Language Server Protocol
over stdio, watches the file system, and publishes diagnostics in order
from one worker.

This crate is an internal part of the buzzard tool. Its API may change in
any 0.x release, including patch releases. It is published so that
`cargo install buzzard-cli` works. To use the server, run `buzzard lsp`. The
protocol is the supported interface.

```rust,no_run
let runtime = tokio::runtime::Runtime::new().expect("the runtime starts");
let status = runtime.block_on(buzzard_ls::run_stdio());
std::process::exit(status);
```

## License

Apache-2.0. See `LICENSE` and `NOTICE` in this package.

LabVIEW, NI, and National Instruments are trademarks of National Instruments
Corporation, used only to identify the file formats that buzzard reads.
buzzard is not affiliated with, endorsed by, or sponsored by that company.
