# buzzard

A fast, cross-platform static analyzer and language server for LabVIEW™
projects, written in Rust.

`buzzard` reads binary `.vi` and `.ctl` files directly from disk. It does not
need the LabVIEW IDE or runtime, so it runs on Linux, macOS and Windows CI
runners alike.

> **Status:** early development. See the crate list below for what exists.

## Workspace

| Crate | Purpose |
| --- | --- |
| `crates/buzzard-rsrc` | Parser for the binary RSRC resource container |
| `crates/buzzard-lint` | Lint rules (`BZxxx`) and the workspace dependency graph |
| `crates/buzzard-ls` | Language Server Protocol implementation |
| `crates/buzzard-cli` | The `buzzard` binary (`check`, `inspect`, `lsp`) |

## Development

```bash
cargo test --workspace
```

## Acknowledgements

The RSRC container layout was learned from the open-source work of
[mefistotelis/pylabview](https://github.com/mefistotelis/pylabview) (MIT) and
[marcpage/pylavi](https://github.com/marcpage/pylavi) (public domain). The
conformance fixtures under `crates/buzzard-rsrc/tests/fixtures` come from
pylavi.

## Trademark disclaimer

LabVIEW and National Instruments are trademarks of National Instruments
Corporation (NI), now part of Emerson. `buzzard` is an independent open-source
project. It is not affiliated with, endorsed by, or sponsored by NI or
Emerson. Those names are used here only to describe the file formats this
tool reads.

## License

MIT. See [LICENSE](LICENSE).
