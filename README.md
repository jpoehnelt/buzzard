# buzzard

`buzzard` is a static analyzer, a language server, and a Model Context Protocol
(MCP) server for LabVIEW files: `.vi`, `.ctl`, `.llb`, `.lvproj`, `.lvlib`, and
`.lvclass`. It also reads TDMS measurement files. `buzzard` reads the files
from disk and needs no LabVIEW installation, so it runs on Linux, macOS, and
Windows CI runners. `buzzard check` finds problems such as breakpoints left
on, unwired error outputs, references that are never closed, links by absolute
path, and circular dependencies. `buzzard verify` finds states of a state
machine that can never be reached and loops that can never stop.

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

A GitHub Release may attach prebuilt binaries. Download the
`buzzard-<version>-<target>` archive for your system and `SHA256SUMS`. To
verify the archive, run `sha256sum -c --ignore-missing SHA256SUMS` on Linux
or `shasum -a 256 -c --ignore-missing SHA256SUMS` on macOS. On Windows, run
`Get-FileHash -Algorithm SHA256 <archive>` in PowerShell and compare the hash
with the archive's line in `SHA256SUMS`. Then unpack the archive and put
`buzzard` on your `PATH`. The binaries are not signed. If macOS blocks the
first start, run `xattr -c buzzard`. On Windows, SmartScreen may warn about an
unknown publisher.

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
- To print the first problems only, pass `--limit <N>`. The summary still counts every problem, and the exit status does not change. With `--limit`, the JSON is an object with `diagnostics` and `summary`, not the bare list.
- To skip a rule, pass `--ignore <CODE>` once per rule. `buzzard rules` lists the codes. To change the path length limit of `BZ003`, pass `--max-path-length <N>`.
- To fail a CI job on problems, use the exit status. `check` exits 1 when a problem is an error or a warning, and 0 otherwise or with `--exit-zero`. `check` exits 2 on an [error](#errors). When the program that reads the output, such as `head`, closes the pipe early, the status does not change.

### Print what one file holds

```bash
buzzard inspect crates/buzzard-rsrc/tests/fixtures/pylavi/add.vi --format json
```

- To print the settings, links, and block diagram as Markdown, omit `--format`.
- To print an outline in place of every node and wire, pass `--detail summary`. The full JSON of one VI can be over 1 MB.
- To print one frame or a range of nodes as JSON, take the ids from the outline. Pass `--structure <ID> --frame <INDEX>` or `--first-node <ID> --last-node <ID>` with `--format json`.
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
- To leave out nodes that only moved or changed size, pass `--ignore-moves`.
- To print Markdown or JSON, pass `--format markdown` or `--format json`.
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

- To make `deps` look up the files of a symbolic root on disk, pass `--root NAME=DIR` once per root, as in `--root vilib=/opt/labview/vi.lib`. Write the name with or without angle brackets, in any letter case.
- To print JSON or Markdown, pass `--format json` or `--format markdown`.
- To print the counts of each package only, pass `--detail packages`.

### Check the state machines on block diagrams

```console
$ buzzard verify crates/buzzard-lint/tests/fixtures/verify/jki-smo/process.vi
crates/buzzard-lint/tests/fixtures/verify/jki-smo/process.vi
  state machine at while loop #3379, case structure #3023
    states (2): Init, Idle
    initial state: Init
...
```

- To print JSON or the graphs for Graphviz, pass `--format json` or `--format dot`.
- To fail a CI job on findings, use the exit status. `verify` exits 1 for findings of any [label](#labels-of-a-verify-finding).

### Read a TDMS file

```bash
buzzard tdms info crates/buzzard-tdms/tests/fixtures/zenodo-7758683/waveforms-point21.tdms
buzzard tdms export crates/buzzard-tdms/tests/fixtures/zenodo-7758683/waveforms-point21.tdms --out point21.parquet
```

`info` prints the groups, channels, properties, and anomalies of a file.
`export` writes the channels as one Parquet table, or as CSV with
`--format csv`. When a file is cut short or its last segment was never
finished, `buzzard` keeps the values that are there and reports an anomaly.
The [buzzard-tdms README](crates/buzzard-tdms/README.md) has the details.

- To replace a file that `--out` names, pass `--force`. Without it, `export` exits 2 and leaves the file as it was. `/dev/stdout` needs no `--force`.
- To see the rows, columns, and target of an export and write nothing, pass `--dry-run`.
- To print the result of an export or a dry run as JSON, pass `--json`.

### Run the language server or the MCP server

- To start the language server over stdio, run `buzzard lsp`. For VS Code, use the extension in [`editors/vscode`](editors/vscode).
- To start the MCP server over stdio, run `buzzard mcp`. To register the server in Claude Code, run `claude mcp add buzzard -- buzzard mcp`. To let its tools read only inside a directory, pass `--root <DIR>` once per directory.
- To print the guide for AI agents, run `buzzard guide`.

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
| A member that declares more than 128 MiB, or a compressed member that would take the compressed members of its archive past 1 GiB inflated | `BZ000` on the member path |
| An archive inside an archive | `inspect` lists the archive, and no command opens it |
| A member that is not a VI, a control, or a project file | `inspect` lists the member, and no command opens it |

`diff` of two `.llb` files exits 2 and asks for members, unless their bytes are
the same. SARIF and the language server locate a problem in a member at the
`.llb` file, with the member's name first in the message.

### Rules that `check` reports

| Code | Severity | Meaning |
| --- | --- | --- |
| `BZ000` | error | A file cannot be read or parsed, or a member of an archive cannot be opened. `check` reports the problem and does not exit 2 |
| `BZ001` | warning | Compiled code is stored inside the source file |
| `BZ002` | warning | Saved with a different version than most of the workspace. The rule reports nothing when the two most common versions have the same number of files |
| `BZ003` | warning | A link by a machine-specific absolute path, or a path over the length limit: a link's path, or the file's own path from the scan root |
| `BZ004` | warning | Breakpoints or suspend-when-called left on, or a password-protected block diagram |
| `BZ005` | error | Part of a circular dependency |
| `BZ006` | warning | A project, library, or class lists an item whose file does not exist on disk or in its archive. The rule skips an item under `Dependencies`, an item with a `<vilib>` path, and an item that the project file embeds |
| `BZ101` | warning or info | An error output on the block diagram is not wired. The problem is a warning when automatic error handling is off, and info when it is on |
| `BZ102` | warning | A reference is created and never reaches anything that could close it. The rule reports the reference only when every path from it ends at an unwired output |
| `BZ201` | warning | A state of a state machine can never be reached (proven) |
| `BZ202` | warning or info | A state machine has a state from which its loop can never stop (warning), or its loop can never stop at all (info). Proven or structural |
| `BZ203` | warning | A constant that no case handles can reach the state register (structural or heuristic) |

In SARIF, a relative path gives a relative URI reference, and an absolute path
gives a `file:///` URI. Both are percent-encoded.

### Statuses and packages in a `deps` report

An external file is a file under a symbolic root such as `<vilib>`. `deps`
gives each external file one of these statuses:

- `found`: `--root` names a directory for the file's root, and the directory holds the file.
- `missing`: `--root` names a directory for the file's root, and the directory does not hold the file.
- `unchecked`: in every other case, such as when no `--root` names a directory for the file's root, or when the file is inside an `.llb` archive that `buzzard` cannot read.

`deps` groups the external files by package. A package is the first two
directories below the root, or the first three when the first is `addons`. An
archive, a library, or a class ends the name early. The rustdoc of
`buzzard_lint::deps::package` has the full rule. When no link is under a
`--root`, `deps` warns on stderr and lists the root in `unused_roots`.

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
| `textDocument/publishDiagnostics` | Every problem that `check` reports, within about 150 ms of a save |
| `workspace/symbol` | Fuzzy search over VIs, controls, classes, libraries, and projects |
| `textDocument/hover` | Saved version, execution flags, and counts of dependencies and callers |
| `textDocument/references` | Every file in the workspace that uses the file |
| `buzzard/inspect` | Custom request with `{ "uri": ... }`: metadata, callers, callees, and owners as JSON |
| `buzzard/exportAst` | Custom request with `{ "uri": ..., "format": "json" or "markdown" }`: the block diagram |

A file that fails to parse right after a save may still be in the middle of a
write. The server reads the file again for about 0.4 s before it reports
`BZ000`.

### MCP tools

The tools are `buzzard_check`, `buzzard_verify`, `buzzard_deps`,
`buzzard_inspect`, `buzzard_diagram`, `buzzard_callers`, `buzzard_callees`,
`buzzard_symbols`, `buzzard_rules`, `buzzard_diff`, and `buzzard_tdms_info`.
The server's `tools/list` response gives the arguments and the `outputSchema`
of each tool.

- Every tool is read-only.
- `buzzard_deps` returns counts per package. With `detail` `files`, it returns what `buzzard deps` prints.
- A result is at most `max_bytes` bytes. The default is 65536. A call that passes less than 1024 fails with `bad_argument`. The size is the text plus the `structuredContent` as compact JSON.
- A larger answer comes as a smaller one. Its `budget` entry gives `max_bytes`, the size of the whole answer in `full_bytes`, and how to get the rest in `more`.
- `buzzard_diagram` with `detail` `full` and no `max_bytes` is the only result without a bound. A whole diagram in format `json` has the diagram itself (`root`) as `structuredContent`.
- A failed call is a result with `isError` set and the [error object](#errors) as `structuredContent`. Findings and differences are not failures.
- The server resolves a relative path against the directory it was started in.
- With `--root`, the server resolves links and `..` in a path. It refuses a path that then leads outside every root, with the kind `outside_root`.
- The server caches the most recent parsed workspace. It parses again when a file was added or removed, when a file's size or modification time differs, or when a call sets `refresh: true`.
- The prompt `buzzard` (`prompts/list`, `prompts/get`) is a guide for agents to the tools and the commands. `buzzard guide` prints it, and `.claude/skills/buzzard/SKILL.md` holds the same text as a skill.

### Errors

On an error, a command exits with status 2 and prints `error: <message>` on
stderr. With `--error-format json`, and with `--format json`, `--format sarif`,
or `--json`, the command also prints one JSON object on one line of stdout:
`{"error":{"kind":"path_not_found","message":"...","path":"lib/a.vi"}}`.

- The object has `path` when the error is about one path.
- The kinds are `path_not_found`, `unknown_rule`, `unreadable_file`, `bad_argument`, `outside_root`, `unsupported`, `output_exists`, `write_failed`, `usage`, and `internal`. `buzzard --help` gives the meaning of each kind.
- A command fails with `bad_argument` on a path argument that holds an ASCII control character. The one exception is `diff --git`, which takes the names that git passes as they are. `buzzard` takes every other character of a file name, such as `?`, `#`, or `%`, as it is.
- `inspect`, `tdms info`, `tdms export`, and the MCP tools that take one file fail with `bad_argument` on a directory. `diff` and `buzzard_diff` fail with `unreadable_file`.
- Human, text, and Markdown output never print an ASCII control character, or a character that reorders text such as U+202E, from a file name, a member name, or a name or value of a TDMS file. They print an escape such as `\n`, and `deps` prints a space. JSON and SARIF hold the text itself.

## Limitations

- `verify` models the classic state machine only. Where it cannot follow a next state or a stop condition, it reports less, never a false proof. On a JKI-style machine that queues its states as strings, it makes one heuristic check.
- `verify` does not model a for loop, a timed loop, a state that a cluster or a feedback node holds, or a case structure with more than 1,024 cases. It does not model a selector that is a Boolean or an error cluster, or a selector that the register does not feed. It does not know which case of an event structure handles which event.
- The decoder records everything it skips as a loss, and `verify` does not search a block diagram that has a loss. The rustdoc of `buzzard_lint::verify::extract` lists every assumption.
- A proof is about the file as saved. `verify` takes the items of a type definition from the copy in the VI and does not open the `.ctl` file.
- The tests check the decoder against files that LabVIEW saved. No test runs LabVIEW.
- `buzzard` names built-in functions from a vendored table and shows an id outside the table as a number. No second source confirms most of those names.
- The decoder reads only Boolean, integer, enumeration, floating-point, and string constants. It does not read free labels or wire routes.
- `buzzard` does not decode the block diagram of a file saved before version 8. It does not open packed libraries (`.lvlibp`) or installer packages.
- The authors found no real archive with zlib members, so only archives that the tests build check that code.
- `buzzard` counts a file twice when a symbolic link also leads to the file.
- Text output prints the characters U+0080 to U+009F of a name as they are, because they are letters in a member name of a legacy code page. Some terminals act on U+009B.

## Contributing

Before you open a pull request, run the local commands in the "Checks" table of
[AGENTS.md](AGENTS.md). The same file has the rules for tests, fixtures, and
commit subjects. Do not copy npTDMS source, tests, or fixtures into this
repository. To report a vulnerability, follow [SECURITY.md](SECURITY.md).

## Acknowledgements

`buzzard` builds on these projects:

- [pylabview](https://github.com/mefistotelis/pylabview) (MIT) and [pylavi](https://github.com/marcpage/pylavi) for the file format. The table of heap tag and class names is generated from pylabview.
- [LVKit](https://github.com/pragmatest-dev/lvkit) (Apache-2.0) for the table of built-in function names.
- [npTDMS](https://github.com/adamreeve/npTDMS) (LGPL-3.0), which the tests run as a separate program. None of it is in this repository.
- The fixtures come from pylavi, pylabview, ni/grpc-labview, JKISoftware/JKI-State-Machine, JKISoftware/JKI-State-Machine-Objects, LabVIEW-Open-Source/LV-MQTT-Broker, and a [recording](https://doi.org/10.5281/zenodo.7758683) by Saullo G. P. Castro (CC BY 4.0). NOTICE gives each license.

## License and trademark

The license is Apache-2.0, and [LICENSE](LICENSE) holds the text. One generated
table in `buzzard-rsrc` comes from pylabview under the MIT license, and
[NOTICE](NOTICE) holds that text.

LabVIEW, NI, and National Instruments are trademarks of National Instruments
Corporation, used only to identify the file formats that buzzard reads. buzzard
is not affiliated with, endorsed by, or sponsored by that company.
