# buzzard

`buzzard` is a static analyzer, a language server and a Model Context
Protocol (MCP) server for LabVIEW files: `.vi`, `.ctl`, `.llb`, `.lvproj`,
`.lvlib` and `.lvclass`. It also reads TDMS measurement files. It reads
the files from disk and needs no LabVIEW installation, so it runs on Linux,
macOS and Windows CI runners.

## Install

```bash
cargo install --git https://github.com/jpoehnelt/buzzard buzzard-cli
```

From a checkout, run `cargo install --path crates/buzzard-cli`. Both need
Rust 1.91 or later and install one binary, `buzzard`. The crates are not on
crates.io yet.

## Commands

The examples run from a checkout of this repository, on its test fixtures.
`check`, `deps` and `verify` scan the current directory when given no path.

### check

```console
$ buzzard check crates/buzzard-rsrc/tests/fixtures/pylavi/add_breakpoints.vi
crates/buzzard-rsrc/tests/fixtures/pylavi/add_breakpoints.vi: warning[BZ001]: compiled code is not separated from the source file; enable "Separate compiled code from source file"
crates/buzzard-rsrc/tests/fixtures/pylavi/add_breakpoints.vi: warning[BZ004]: 3 breakpoints left in the block diagram
Found 2 problems (0 errors, 2 warnings) in 1 file.
```

- `--format` is `human` (the default), `json` or `sarif`.
- `--ignore <CODE>` skips a rule and may be repeated. `--max-path-length
  <N>` sets the limit of `BZ003`. `--exit-zero` exits 0 despite problems.
- The exit status is 0 for no problems, 1 for problems, and 2 for an
  unknown `--ignore` code, a path that does not exist, or a report that
  cannot be written. A file that cannot be parsed is a `BZ000` problem,
  not an error. Neither is a reader that closes the pipe early.
- In SARIF, a relative path gives a relative URI reference and an absolute
  path gives a `file:///` URI. Both are percent-encoded.

### inspect

```bash
buzzard inspect crates/buzzard-rsrc/tests/fixtures/pylavi/add.vi --format json
```

`inspect` prints the settings, links and block diagram of one file.
`--format` is `markdown` (the default), `json`, or a diagram as `svg`, `dot`
or `mermaid`. On an `.llb` file it lists the members. It exits 2 when the
file cannot be read, or when a diagram is asked of a file that has none.

### diff

```console
$ cd crates/buzzard-lint/tests/fixtures/diff/jki-state-machine
$ buzzard diff vi-tree.old.vi vi-tree.new.vi
1 node added, 1 removed, 2 moved; 2 wires added, 2 removed, 2 changed
added: subVI `Conditional State String.vi` #42
removed: subVI `Conditional State String.vi` #224
...
```

- `#42` is the object id that `inspect --format json` prints.
- `--ignore-moves` leaves out nodes that only changed position or size.
- `--format` is `text` (the default), `markdown` or `json`.
- The exit status is 0 when the files are the same, 1 when they differ, and
  2 for an error such as a file that is not a LabVIEW binary.
- A file that does not exist, or `/dev/null`, is an empty side.

For git, add `*.vi diff=buzzard` to `.gitattributes` and set one of these:

```bash
git config diff.buzzard.textconv "buzzard inspect --format markdown"
git config diff.buzzard.command "buzzard diff --git"
```

The first gives a line diff of the Markdown of both versions. The second
gives the report above. With `--git`, `diff` always exits 0, as git
requires. `git log -p` and `git show` use it only with `--ext-diff`.

### deps

```console
$ buzzard deps crates/buzzard-rsrc/tests/fixtures/pylavi
<vilib> (not checked: no directory was given for root vilib)
  AdvancedString (1 file)
    unchecked <vilib>/AdvancedString/Path To Command Line String.vi
      used by crates/buzzard-rsrc/tests/fixtures/pylavi/absolute_link_PTH.vi
...
```

The report lists files under a symbolic root such as `<vilib>` by package,
links by a machine-absolute path, relative links whose file does not exist,
and files that cannot be parsed.

- `--root NAME=DIR` gives the directory of a root, as in `--root
  vilib=/opt/labview/vi.lib`, and may be repeated. The name matches with
  or without angle brackets and in any letter case. A `--root` that no
  link is under is listed in `unused_roots` and warned about on stderr.
- A file is `found` when its root was given a directory that holds it,
  `missing` when that directory does not, and `unchecked` otherwise.
- A package is the first two directories below the root, or the first
  three below `addons`. A library such as `error.llb` ends the name early.
  The rustdoc of `buzzard_lint::deps::package` has the full rule.
- `--format` is `text` (the default), `json` or `markdown`. The exit status
  is 0 whatever the report holds, and 2 for a path that does not exist or
  a `--root` that cannot be used.

### verify

```console
$ buzzard verify crates/buzzard-lint/tests/fixtures/verify/jki-smo/process.vi
crates/buzzard-lint/tests/fixtures/verify/jki-smo/process.vi
  state machine at while loop #3379, case structure #3023
    states (2): Init, Idle
    initial state: Init
    default state: Idle
    model: over-approximation (0 of 2 states with a next state that could not be followed, 2 with a stop condition that could not be read)
...
```

`verify` finds state machines: a while loop, a shift register, and a case
structure whose selector is wired from the register. Where it cannot
follow a next state, the model lets the case go to any state. `--format` is
`text` (the default), `json` or `dot`. Each finding has one of three labels:

- `PROVEN` holds for every execution of the VI as saved in the file.
- `STRUCTURAL` is a fact about the wiring. It says nothing about what runs.
- `HEURISTIC` is a match on names and can be wrong in both directions.

The decoder records everything it skips as a loss. A diagram with a loss is
not searched. `verify` does not model:

- a state held in a cluster or a feedback node
- for loops and timed loops
- a selector that is a Boolean, an error cluster, or not fed by the register
- a case structure with more than 1,024 cases
- which case of an event structure handles which event

The rustdoc of `buzzard_lint::verify::extract` lists every assumption. The
exit status is 0 for no findings, 1 for findings of any label, and 2 for a
path that does not exist. A file that cannot be parsed does not change it.

### tdms

```bash
buzzard tdms info crates/buzzard-tdms/tests/fixtures/zenodo-7758683/waveforms-point21.tdms
buzzard tdms export crates/buzzard-tdms/tests/fixtures/zenodo-7758683/waveforms-point21.tdms --out point21.parquet
```

`info` prints groups, channels, properties and what is wrong with the file,
as `--format text` or `json`. `export` writes one Parquet or CSV table. Both
exit 0, or 2 on an error. A file damaged at its end is read as far as it
goes. See the [buzzard-tdms README](crates/buzzard-tdms/README.md).

### lsp

`buzzard lsp` runs the language server over stdio. When a file fails to
parse, the server reads it again for about 0.4 s before it reports `BZ000`.
It exits 0 after `shutdown` and then `exit`, and 1 otherwise.
[`editors/vscode`](editors/vscode) is a VS Code extension for it.

| Method | Result |
| --- | --- |
| `textDocument/publishDiagnostics` | Every finding of `check`, within about 150 ms of a save |
| `workspace/symbol` | Fuzzy search over VIs, controls, classes, libraries and projects |
| `textDocument/hover` | Saved version, execution flags, counts of dependencies and callers |
| `textDocument/references` | Every file in the workspace that uses the file |
| `buzzard/inspect` | Custom request with `{ "uri": ... }`: metadata, callers, callees and owners as JSON |
| `buzzard/exportAst` | Custom request with `{ "uri": ..., "format": "json" or "markdown" }`: the block diagram |

### mcp

`buzzard mcp` serves the analysis as tools over stdio. To register it in
Claude Code, run `claude mcp add buzzard -- buzzard mcp`.

| Tool | Arguments | Returns |
| --- | --- | --- |
| `buzzard_check` | `paths[]`, `ignore?[]`, `limit?`, `refresh?` | Diagnostics and a summary |
| `buzzard_verify` | `paths[]`, `format?` | What `buzzard verify` prints |
| `buzzard_deps` | `paths[]`, `format?`, `detail?`, `roots?`, `refresh?` | Counts per package, or with `detail` `files` what `buzzard deps` prints |
| `buzzard_inspect` | `path` | Metadata and links of one file, or the members of an archive |
| `buzzard_diagram` | `path`, `format?` | The block diagram of one file |
| `buzzard_callers` | `path`, `root`, `refresh?` | The files under `root` that use the file |
| `buzzard_callees` | `path`, `root`, `refresh?` | The files under `root` that the file uses |
| `buzzard_symbols` | `root`, `query`, `limit?`, `refresh?` | Fuzzy file search |
| `buzzard_rules` | none | The rule table |

Every tool is read-only. The server resolves relative paths against the
directory it was started in. It caches the parsed workspace per root and
parses again when a file's size or modification time differs, or when a
call sets `refresh: true`. A failed call is a result with `isError` set.

## LLB archives

A member of an `.llb` archive is named by a member path: the path of the
archive, a separator and the member's name, as in `lib/util.llb/Init.vi`.
`check`, `verify`, `deps`, `inspect`, both sides of `diff` and the MCP
tools take one where they take a file. A scan of a directory opens the
VIs, the controls and the project files of every archive in it.

| In the archive | Result |
| --- | --- |
| A member that does not parse or cannot be inflated | `BZ000` on the member path |
| A directory that cannot be read | `BZ000` on the `.llb` file |
| A member name that is not a file name (`..`, a name with `/`, `\` or a control character, and on Windows a name with `:`), or a name used twice | `BZ000` on the `.llb` file. The member is not opened |
| A member that declares more than 128 MiB, or compressed members past 1 GiB inflated in one scan | `BZ000` on the member path |
| An archive inside an archive | Listed by `inspect`, not opened |
| A member that is not a VI, a control or a project file | Listed by `inspect` only |

`diff` of two `.llb` files exits 2 and asks for members, unless their bytes
are the same. SARIF and the language server locate a finding about a member
at the `.llb` file, with the member's name first in the message.

## Rules

| Code | Severity | Meaning |
| --- | --- | --- |
| `BZ000` | error | A file cannot be read or parsed, or a member of an archive cannot be opened |
| `BZ001` | warning | Compiled code is stored inside the source file |
| `BZ002` | warning | Saved with a different version than most of the workspace |
| `BZ003` | warning | A link by a machine-specific absolute path, or a path over the length limit |
| `BZ004` | warning | Breakpoints or suspend-when-called left on, or a password-protected block diagram |
| `BZ005` | error | Part of a circular dependency |
| `BZ006` | warning | A project, library or class lists a member whose file does not exist on disk or in its archive |
| `BZ101` | warning or info | An error output on the block diagram is not wired |
| `BZ102` | warning | A reference is created and never reaches anything that could close it |
| `BZ201` | warning | A state of a state machine can never be reached (proven) |
| `BZ202` | warning or info | The loop's condition terminal can never stop a state machine from one state, or at all (proven or structural) |
| `BZ203` | warning | A constant that no case handles can reach the state register (structural or heuristic) |

- `BZ101` is a warning when automatic error handling is off, info when on.
- `BZ102` reports only when every path ends at an unwired output.
- `BZ002` reports nothing when two versions are tied for the lead.
- `BZ003` measures a file's path from the scan root.
- `BZ006` skips `<vilib>` paths, items in the XML file and `Dependencies`.

## Limitations

- `verify` models the classic state machine only. Where it cannot follow a
  next state or a stop condition, it reports less, never a false proof. On a
  JKI-style string/queue machine it makes one heuristic check.
- A proof is about the file as saved. `verify` takes the items of a type
  definition from the copy in the VI and does not open the `.ctl` file.
- The decoding was checked against real files, not against a running editor.
- Built-in functions are named from a vendored table. Most of its names are
  not independently confirmed. An id outside the table is shown as a number.
- Only Boolean, integer, enumeration, floating-point and string constants
  are decoded. Free labels and wire routes are not.
- A file saved before version 8 has no decoded block diagram. Packed
  libraries (`.lvlibp`) and installer packages are not opened.
- No real archive with zlib members was found. Generated archives test it.
- A file reached through a symbolic link is counted a second time.

## Contributing

```bash
cargo fmt --all --check
cargo +1.100.0 clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny --all-features --locked check
```

Run these before a pull request. [AGENTS.md](AGENTS.md) has every check,
including rustdoc and the VS Code extension.

- `BUZZARD_BLESS=1 cargo test -p buzzard-rsrc --test conformance` regenerates
  the parser snapshots.
- A new fixture records its source and license in its README and in NOTICE.
- Do not copy npTDMS source, tests or fixtures into this repository.
- Commit subjects are short imperative sentences.
- To report a vulnerability, see [SECURITY.md](SECURITY.md).

## Acknowledgements

- [pylabview](https://github.com/mefistotelis/pylabview) (MIT) and
  [pylavi](https://github.com/marcpage/pylavi) for the file format. The
  table of heap tag and class names is generated from pylabview.
- [LVKit](https://github.com/pragmatest-dev/lvkit) (Apache-2.0) for the
  table of built-in function names.
- [npTDMS](https://github.com/adamreeve/npTDMS) (LGPL-3.0), which the tests
  run as a separate program. None of it is in this repository.
- The fixtures come from pylavi, pylabview, ni/grpc-labview,
  JKISoftware/JKI-State-Machine, JKISoftware/JKI-State-Machine-Objects,
  LabVIEW-Open-Source/LV-MQTT-Broker, and a
  [recording](https://doi.org/10.5281/zenodo.7758683) by Saullo G. P. Castro
  (CC BY 4.0). NOTICE gives each license.

## Trademark

LabVIEW, NI, and National Instruments are trademarks of National Instruments
Corporation, used only to identify the file formats that buzzard reads.
buzzard is not affiliated with, endorsed by, or sponsored by that company.

## License

Apache-2.0. See [LICENSE](LICENSE). One generated table in `buzzard-rsrc`
comes from pylabview under the MIT license. [NOTICE](NOTICE) has its text.
