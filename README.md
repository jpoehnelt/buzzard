# buzzard

`buzzard` is a static analyzer, a language server, and a Model Context Protocol
(MCP) server for LabVIEW files: `.vi`, `.ctl`, `.llb`, `.lvproj`, `.lvlib`, and
`.lvclass`. It also reads TDMS measurement files. `buzzard` reads the files
from disk and needs no LabVIEW installation, so it runs on Linux, macOS, and
Windows CI runners.

## Install

```bash
git clone https://github.com/jpoehnelt/buzzard
cd buzzard
cargo install --path crates/buzzard-cli
```

To install without a checkout, run
`cargo install --git https://github.com/jpoehnelt/buzzard buzzard-cli`.
Both ways need Rust 1.91 or later and install one binary, `buzzard`. The crates
are not on crates.io yet.

## Quick start

From the checkout, check a fixture VI that has breakpoints. `check` prints two
problems and exits with status 1.

```console
$ buzzard check crates/buzzard-rsrc/tests/fixtures/pylavi/add_breakpoints.vi
crates/buzzard-rsrc/tests/fixtures/pylavi/add_breakpoints.vi: warning[BZ001]: compiled code is not separated from the source file; enable "Separate compiled code from source file"
crates/buzzard-rsrc/tests/fixtures/pylavi/add_breakpoints.vi: warning[BZ004]: 3 breakpoints left in the block diagram
Found 2 problems (0 errors, 2 warnings) in 1 file.
```

## Use the commands

The examples run from a checkout, on the test fixtures. `check`, `deps`, and
`verify` scan the current directory when you give no path. For every option,
format, and exit status of a command, run `buzzard <command> --help`.

### Check files and directories

```bash
buzzard check crates/buzzard-rsrc/tests/fixtures
```

- To write JSON or SARIF, pass `--format json` or `--format sarif`.
- To skip a rule, pass `--ignore <CODE>`. Repeat the flag for each rule. To
  change the path length limit of `BZ003`, pass `--max-path-length <N>`.
- To fail a CI job on problems, use the exit status. `check` exits 1 when a
  problem is an error or a warning, and 0 otherwise. `check` exits 2 for an
  unknown `--ignore` code, a path that does not exist, or a report that it
  cannot write.
- To exit 0 despite errors and warnings, pass `--exit-zero`.

### Print what one file holds

```bash
buzzard inspect crates/buzzard-rsrc/tests/fixtures/pylavi/add.vi --format json
```

- To print the settings, links, and block diagram as Markdown, omit `--format`.
- To draw the block diagram, pass `--format svg`, `dot`, or `mermaid`.
- To list the members of an `.llb` archive, pass the archive.

### Compare two versions of a file

```console
$ cd crates/buzzard-lint/tests/fixtures/diff/jki-state-machine
$ buzzard diff vi-tree.old.vi vi-tree.new.vi
1 node added, 1 removed, 2 moved; 2 wires added, 2 removed, 2 changed
added: subVI `Conditional State String.vi` #42
removed: subVI `Conditional State String.vi` #224
...
```

- To look up an object id such as `#42`, run `inspect --format json`.
- To leave out nodes that only changed position or size, pass `--ignore-moves`.
  For Markdown or JSON, pass `--format markdown` or `json`.
- To compare a file with nothing, pass `/dev/null` as the other file.

To see the changes of a VI in git, add `*.vi diff=buzzard` to `.gitattributes`.
Then set one of these two drivers:

```bash
git config diff.buzzard.textconv "buzzard inspect --format markdown"
git config diff.buzzard.command "buzzard diff --git"
```

The first driver gives a line diff of the Markdown of both versions. The second
gives the report above. With `--git`, `diff` always exits 0, as git requires.
`git log -p` and `git show` use the second driver only with `--ext-diff`.

### List what the files need from outside the project

```console
$ buzzard deps crates/buzzard-rsrc/tests/fixtures/pylavi
<vilib> (not checked: no directory was given for root vilib)
  AdvancedString (1 file)
    unchecked <vilib>/AdvancedString/Path To Command Line String.vi
      used by crates/buzzard-rsrc/tests/fixtures/pylavi/absolute_link_PTH.vi
...
```

- To make `deps` look up the files of a symbolic root on disk, pass
  `--root NAME=DIR` once per root, as in `--root vilib=/opt/labview/vi.lib`.
  Write the name with or without angle brackets, in any letter case.
- To print JSON or Markdown, pass `--format json` or `--format markdown`.

### Check the state machines on block diagrams

```console
$ buzzard verify crates/buzzard-lint/tests/fixtures/verify/jki-smo/process.vi
crates/buzzard-lint/tests/fixtures/verify/jki-smo/process.vi
  state machine at while loop #3379, case structure #3023
    states (2): Init, Idle
    initial state: Init
...
```

- To print JSON, pass `--format json`. To print the transition graphs for
  Graphviz, pass `--format dot`.
- To fail a CI job on findings, use the exit status. `verify` exits 1 for
  findings of any [label](#labels-of-a-verify-finding).

### Read a TDMS file

```bash
buzzard tdms info crates/buzzard-tdms/tests/fixtures/zenodo-7758683/waveforms-point21.tdms
buzzard tdms export crates/buzzard-tdms/tests/fixtures/zenodo-7758683/waveforms-point21.tdms --out point21.parquet
```

`info` prints the groups, channels, properties, and anomalies of a file.
`export` writes the channels as one Parquet table, or as CSV with
`--format csv`. `buzzard` reads a file with a damaged end as far as the file
goes. The [buzzard-tdms README](crates/buzzard-tdms/README.md) has the details.

### Run the language server or the MCP server

- To start the language server over stdio, run `buzzard lsp`. For VS Code, use
  the extension in [`editors/vscode`](editors/vscode).
- To start the MCP server over stdio, run `buzzard mcp`. To register the server
  in Claude Code, run `claude mcp add buzzard -- buzzard mcp`.

## Reference

### Member paths and `.llb` archives

A member is one file inside an `.llb` archive. A member path names a member by
the path of the archive, a separator, and the member's name, as in
`lib/util.llb/Init.vi`. `check`, `verify`, `deps`, `inspect`, both sides of
`diff`, and the MCP tools take a member path where they take a file. A scan
opens the VIs, the controls, and the project files of every archive it finds.

| In the archive | Result |
| --- | --- |
| A member that `buzzard` cannot parse or inflate | `BZ000` on the member path |
| An archive whose directory (its list of members) `buzzard` cannot read | `BZ000` on the `.llb` file |
| A member name that is not a file name (`..`, a name with `/`, `\`, or a control character, and on Windows a name with `:`), or a name used twice | `BZ000` on the `.llb` file. No command opens the member |
| A member that declares more than 128 MiB, or compressed members past 1 GiB inflated in one scan | `BZ000` on the member path |
| An archive inside an archive | `inspect` lists the archive, and no command opens it |
| A member that is not a VI, a control, or a project file | `inspect` lists the member, and no command opens it |

`diff` of two `.llb` files exits 2 and asks for members, unless their bytes are
the same. SARIF and the language server locate a problem in a member at the
`.llb` file, with the member's name first in the message.

### Rules that `check` reports

| Code | Severity | Meaning |
| --- | --- | --- |
| `BZ000` | error | A file cannot be read or parsed, or a member of an archive cannot be opened |
| `BZ001` | warning | Compiled code is stored inside the source file |
| `BZ002` | warning | Saved with a different version than most of the workspace |
| `BZ003` | warning | A link by a machine-specific absolute path, or a path over the length limit |
| `BZ004` | warning | Breakpoints or suspend-when-called left on, or a password-protected block diagram |
| `BZ005` | error | Part of a circular dependency |
| `BZ006` | warning | A project, library, or class lists an item whose file does not exist on disk or in its archive |
| `BZ101` | warning or info | An error output on the block diagram is not wired |
| `BZ102` | warning | A reference is created and never reaches anything that could close it |
| `BZ201` | warning | A state of a state machine can never be reached (proven) |
| `BZ202` | warning or info | The loop's condition terminal can never stop a state machine from one state, or at all (proven or structural) |
| `BZ203` | warning | A constant that no case handles can reach the state register (structural or heuristic) |

- A file that `check` cannot parse is a `BZ000` problem in the report and does not make `check` exit 2. `check` also does not fail when the program that reads its output closes the pipe early.
- `BZ002` reports nothing when the two most common versions have the same number of files.
- `BZ003` measures a file's path from the scan root.
- `BZ006` skips an item under `Dependencies`, an item with a `<vilib>` path, and an item that the project file embeds.
- `BZ101` is a warning when automatic error handling is off, and info when it is on.
- `BZ102` reports only when every path ends at an unwired output.
- In SARIF, a relative path gives a relative URI reference and an absolute path gives a `file:///` URI. Both are percent-encoded.

### Statuses and packages in a `deps` report

An external file is a file under a symbolic root such as `<vilib>`. The report
lists the external files by package, the machine-absolute links, the relative
links whose file does not exist, and the files that `deps` cannot parse.

- An external file is `found` when `--root` names a directory for the file's
  root and that directory holds the file. The file is `missing` when the
  directory does not hold it, and `unchecked` otherwise.
- A package is the first two directories below the root, or the first three
  below `addons`. An archive, a library, or a class ends the name early. The
  rustdoc of `buzzard_lint::deps::package` has the full rule.
- `deps` lists a `--root` that no link is under in `unused_roots` and warns on
  stderr.

### Labels of a `verify` finding

`verify` finds state machines: a while loop, a shift register, and a case
structure whose selector is wired from the register. Where `verify` cannot
follow a next state, the model lets the case go to any state. Each finding has
one of these labels:

- A `PROVEN` finding holds for every execution of the VI as saved in the file.
- A `STRUCTURAL` finding is a fact about the wiring, not about what runs.
- A `HEURISTIC` finding is a match on names and can be wrong in both directions.

### Language server methods

| Method | Result |
| --- | --- |
| `textDocument/publishDiagnostics` | Every problem that `check` reports, within about 150 ms of a save. When a file fails to parse, the server reads the file again for about 0.4 s before it reports `BZ000` |
| `workspace/symbol` | Fuzzy search over VIs, controls, classes, libraries, and projects |
| `textDocument/hover` | Saved version, execution flags, and counts of dependencies and callers |
| `textDocument/references` | Every file in the workspace that uses the file |
| `buzzard/inspect` | Custom request with `{ "uri": ... }`: metadata, callers, callees, and owners as JSON |
| `buzzard/exportAst` | Custom request with `{ "uri": ..., "format": "json" or "markdown" }`: the block diagram |

### MCP tools

The tools are `buzzard_check`, `buzzard_verify`, `buzzard_deps`,
`buzzard_inspect`, `buzzard_diagram`, `buzzard_callers`, `buzzard_callees`,
`buzzard_symbols`, and `buzzard_rules`. Every tool is read-only, and the
server's `tools/list` response describes each tool and its arguments.
`buzzard_deps` returns counts per package, or with `detail` `files` what
`buzzard deps` prints. A failed call is a result with `isError` set. The server
resolves relative paths against the directory it was started in. It caches the
most recent parsed workspace and parses again when a file's size or
modification time differs, or when a call sets `refresh: true`.

## Limitations

- `verify` models the classic state machine only. Where it cannot follow a next
  state or a stop condition, it reports less, never a false proof. On a
  JKI-style string/queue machine it makes one heuristic check.
- `verify` does not model a for loop, a timed loop, a state that a cluster or a
  feedback node holds, or a case structure with more than 1,024 cases. It does
  not model a selector that is a Boolean or an error cluster, or a selector
  that the register does not feed. It does not know which case of an event
  structure handles which event.
- The decoder records everything it skips as a loss, and `verify` does not
  search a block diagram that has a loss. The rustdoc of
  `buzzard_lint::verify::extract` lists every assumption.
- A proof is about the file as saved. `verify` takes the items of a type
  definition from the copy in the VI and does not open the `.ctl` file.
- The decoding was checked against real files, not against a running LabVIEW.
- `buzzard` names built-in functions from a vendored table and shows an id
  outside the table as a number. No second source confirms most of those names.
- The decoder reads only Boolean, integer, enumeration, floating-point, and
  string constants. It does not read free labels or wire routes.
- `buzzard` does not decode the block diagram of a file saved before version 8.
  It does not open packed libraries (`.lvlibp`) or installer packages.
- No real archive with zlib members was found. Generated archives test them.
- `buzzard` counts a file twice when a symbolic link also leads to the file.

## Contributing

Before you open a pull request, run the local commands in the "Checks" table of
[AGENTS.md](AGENTS.md). The same file has the rules for tests, fixtures, and
commit subjects. Do not copy npTDMS source, tests, or fixtures into this
repository. To report a vulnerability, follow [SECURITY.md](SECURITY.md).

## Acknowledgements

`buzzard` builds on these projects:

- [pylabview](https://github.com/mefistotelis/pylabview) (MIT) and
  [pylavi](https://github.com/marcpage/pylavi) for the file format. The table
  of heap tag and class names is generated from pylabview.
- [LVKit](https://github.com/pragmatest-dev/lvkit) (Apache-2.0) for the table
  of built-in function names.
- [npTDMS](https://github.com/adamreeve/npTDMS) (LGPL-3.0), which the tests run
  as a separate program. None of it is in this repository.
- The fixtures come from pylavi, pylabview, ni/grpc-labview,
  JKISoftware/JKI-State-Machine, JKISoftware/JKI-State-Machine-Objects,
  LabVIEW-Open-Source/LV-MQTT-Broker, and a
  [recording](https://doi.org/10.5281/zenodo.7758683) by Saullo G. P. Castro
  (CC BY 4.0). NOTICE gives each license.

## Trademark

LabVIEW, NI, and National Instruments are trademarks of National Instruments
Corporation, used only to identify the file formats that buzzard reads. buzzard
is not affiliated with, endorsed by, or sponsored by that company.

## License

The license is Apache-2.0, and [LICENSE](LICENSE) holds the text. One generated
table in `buzzard-rsrc` comes from pylabview under the MIT license, and
[NOTICE](NOTICE) holds that text.
