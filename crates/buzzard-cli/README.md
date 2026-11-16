# buzzard-cli

The `buzzard` binary: a static analyzer, language server and MCP server for
LabVIEW projects, and a reader for the TDMS measurement files they produce.
It reads the binary files directly; it needs neither the editor nor its
runtime.

```sh
cargo install buzzard-cli
```

The binary is named `buzzard`.

```sh
buzzard check .                     # lint a project; --format json or sarif
buzzard inspect Main.vi             # what one file holds
buzzard deps .                      # what the project needs from outside
buzzard diff Old.vi New.vi          # semantic diff of two files
buzzard verify .                    # state machine findings
buzzard tdms info run.tdms          # describe a TDMS file
buzzard tdms export run.tdms --out run.parquet
buzzard lsp                         # language server over stdio
buzzard mcp                         # MCP server over stdio
```

The [repository's README](https://github.com/jpoehnelt/buzzard#readme) is the
manual: usage, the rules, exit statuses, output formats and limitations.

The `parquet` feature is on by default. `cargo install buzzard-cli
--no-default-features` builds a binary whose `tdms export` writes CSV only
and that does not carry the Arrow and Parquet crates.

This crate has no library. Its building blocks are `buzzard-rsrc` (the file
parser) and `buzzard-tdms` (the TDMS reader), which are libraries for other
programs, and `buzzard-lint`, `buzzard-ls` and `buzzard-mcp`, which are
internal.

## Licence

Apache-2.0. See `LICENSE` and `NOTICE` in this package.

LabVIEW and National Instruments are trademarks of National Instruments
Corporation. buzzard is an independent project and is not affiliated with,
endorsed by, or sponsored by it; the names are used only to identify the
file formats that buzzard reads.
