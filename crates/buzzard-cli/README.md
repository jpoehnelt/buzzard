# buzzard-cli

`buzzard-cli` is the `buzzard` binary: a static analyzer, a language server,
and a Model Context Protocol (MCP) server for LabVIEW files. The binary
also reads TDMS measurement files. `buzzard` reads the files from disk and
needs no LabVIEW installation.

## Install

To install the `buzzard` binary, run this command:

```sh
cargo install buzzard-cli
```

To build a binary without the Arrow and Parquet crates, add
`--no-default-features`. That flag turns off the `parquet` feature, and
`tdms export` then writes CSV only.

## Run the commands

```sh
buzzard check .                     # report the problems in a directory
buzzard inspect Main.vi             # what one file holds
buzzard deps .                      # what the files need from outside
buzzard diff Old.vi New.vi          # semantic diff of two files
buzzard verify .                    # state machine findings
buzzard tdms info run.tdms          # describe a TDMS file
buzzard tdms export run.tdms --out run.parquet
buzzard lsp                         # language server over stdio
buzzard mcp                         # MCP server over stdio
```

To see the options, output formats, and exit statuses of a command, run
`buzzard <command> --help`. The
[buzzard README](https://github.com/jpoehnelt/buzzard#readme) is the manual.
It has the usage, the rules, and the limitations.

## Which crates are libraries

This crate has no library. Cargo builds the binary from five other crates.
Two of them are libraries for other programs: `buzzard-rsrc`, the file
parser, and `buzzard-tdms`, the TDMS reader. The other three,
`buzzard-lint`, `buzzard-ls`, and `buzzard-mcp`, are internal.

## License

The license is Apache-2.0. The `LICENSE` and `NOTICE` files of this package
hold the texts.

LabVIEW, NI, and National Instruments are trademarks of National Instruments
Corporation, used only to identify the file formats that buzzard reads.
buzzard is not affiliated with, endorsed by, or sponsored by that company.
