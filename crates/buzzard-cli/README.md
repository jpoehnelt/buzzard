# buzzard-cli

This crate is the `buzzard` binary. It is a static analyzer, a language
server, and an MCP server for LabVIEW projects, and a reader for the TDMS
measurement files they produce. It reads the files from disk and needs no
LabVIEW installation.

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
manual. It has the usage, the rules, the exit statuses, the output formats,
and the limitations.

The `parquet` feature is on by default. `cargo install buzzard-cli
--no-default-features` builds a binary whose `tdms export` writes CSV only
and that does not carry the Arrow and Parquet crates.

This crate has no library. It is built from `buzzard-rsrc` (the file parser)
and `buzzard-tdms` (the TDMS reader), which are libraries for other
programs, and from `buzzard-lint`, `buzzard-ls`, and `buzzard-mcp`, which
are internal.

## License

Apache-2.0. See `LICENSE` and `NOTICE` in this package.

LabVIEW, NI, and National Instruments are trademarks of National Instruments
Corporation, used only to identify the file formats that buzzard reads.
buzzard is not affiliated with, endorsed by, or sponsored by that company.
