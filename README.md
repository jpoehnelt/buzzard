# buzzard

A fast, cross-platform static analyzer and language server for LabVIEW™
projects, written in Rust.

`buzzard` reads binary `.vi` and `.ctl` files and the XML of `.lvproj`,
`.lvlib` and `.lvclass` files directly from disk. It does not need the
LabVIEW IDE or runtime, so it runs on Linux, macOS and Windows CI runners
alike.

It also reads the measurement files those projects produce: `buzzard tdms`
describes a TDMS file and exports its channels to Parquet or CSV. See
[TDMS measurement files](#tdms-measurement-files).

## Install

```bash
cargo install --path crates/buzzard-cli
```

## Usage

Check a project. The exit status is 1 when problems are found, so this works
as a CI gate:

```bash
buzzard check path/to/project
```

Write SARIF for GitHub or GitLab code scanning:

```bash
buzzard check . --format sarif > buzzard.sarif
```

Summarize one file as Markdown or JSON (handy as context for AI agents).
Projects, libraries and classes work too:

```bash
buzzard inspect Main.vi --format json
buzzard inspect Motor.lvclass
```

`inspect` includes the block diagram: an outline of nodes, terminals and
wires in Markdown (with the label of each case and the value of each
constant), the full graph in JSON, or a picture:

```bash
buzzard inspect Main.vi --format svg > Main.svg
buzzard inspect Main.vi --format mermaid
buzzard inspect Main.vi --format dot | dot -Tpng -o Main.png
```

The SVG places nodes, structures and tunnels where the editor drew them.
Wires are drawn as elbows between their end points, not along their original
routes, and built-in functions are labelled boxes because their icons are
not stored in the file. Their names are not stored either: a box is labelled
from a vendored table of function names (`Add`), or with the function's id
(`function 1050`) when the table does not have it.

Run the language server over stdio:

```bash
buzzard lsp
```

`check` also accepts `--format json`, `--ignore <CODE>` (repeatable),
`--max-path-length <N>` and `--exit-zero`.

Exit status of `check` and `inspect`: 0 for success, 1 when `check` found
problems, 2 for an error. A report that could not be written (a full disk,
for example) is an error, so a CI step never passes with an empty or
truncated report. A reader that closes the pipe early, as in
`buzzard check | head`, is not.

## Semantic diff

`buzzard diff` compares two versions of a VI and says what changed, as
text. It works on the files alone, so it runs in CI and in `git diff`:

```bash
buzzard diff old/Main.vi new/Main.vi
```

```text
1 node added, 1 removed, 2 moved; 2 wires added, 2 removed, 2 changed
added: subVI `Conditional State String.vi` #42
removed: subVI `Conditional State String.vi` #224
moved: subVI `Build State String with Arguments.vi` #178 (484,192 -> 524,192)
moved: subVI `Periodic State String.vi` #180 (486,314 -> 528,306)
added: boolean wire from diagram boundary #7 to subVI `Conditional State String.vi` #42
...
changed: numeric wire from diagram boundary #7 to subVI `Periodic State String.vi` #180: attached to a different terminal
```

It compares:

- **Settings**: saved version, compiled code separation, automatic error
  handling, debugging, breakpoints, reentrancy, inlining, locked, password
  protection, owning library and the other execution flags.
- **Dependencies**: links added and removed, by path and name.
- **Block diagram**: nodes added, removed and changed (relinked to another
  file, terminals added or removed, put into or taken out of a structure),
  and wires added, removed and re-attached. A node inside a structure is
  located by the structures around it, e.g. `in case structure #50 frame 1
  > while loop #60`. `#42` is the object's id, the same one that
  `inspect --format json` prints.

A node that only changed position or size is reported as `moved`, apart
from real changes. `--ignore-moves` leaves those out, so that tidying up a
diagram is not a difference at all.

`--format markdown` gives the same report as a bullet list for a review
comment, `--format json` as structured data.

The exit status is that of `diff`: 0 when the files are the same, 1 when
they differ, 2 for an error such as a file that is not an RSRC container.
A file that does not exist (or `/dev/null`) is an empty side: everything in
the other file is then reported as added or removed, with a note.

Nodes are matched between the two versions by their object id, which the
editor keeps across saves: over 1,340 pairs of consecutive revisions from
two public repositories, 96% of nodes kept their id. Nodes whose id did
change are matched by what they are and where they are (class, name,
function id, terminal types, enclosing structures); what is left is
reported as added or removed. The file does not hold the names of built-in
functions: they are named from a vendored table (``built-in function `Add` ``)
and, where the table has no entry, by their function id
(`built-in function 1234`).

### In git

There are two ways to make `git diff` show something better than "Binary
files differ". Both start with one line in `.gitattributes`:

```gitattributes
*.vi diff=buzzard
```

**Text conversion** needs no more than one setting. Git converts both
versions to the Markdown of `buzzard inspect` and shows an ordinary line
diff of the two. It also works in `git log -p` and `git show`:

```bash
git config diff.buzzard.textconv "buzzard inspect --format markdown"
```

**External diff driver** gives the semantic diff described above:

```bash
git config diff.buzzard.command "buzzard diff --git"
```

Options go into the command, for example
`"buzzard diff --git --ignore-moves"`. `git diff` uses the driver by
default; `git log -p` and `git show` use it with `--ext-diff`. With
`--git`, `buzzard diff` takes the seven arguments git passes to a driver
and always exits with status 0, because git treats anything else as a
failed diff; a file it cannot read is reported in the output instead.

Set either with `git config --global` to have it in every repository; the
`.gitattributes` line stays per repository.

## Verification

`buzzard verify` finds the state machines on block diagrams and checks
them:

```bash
buzzard verify path/to/project
buzzard verify Main.vi --format dot | dot -Tsvg -o Main-states.svg
```

```text
Main.vi
  state machine at while loop #38, case structure #49
    states (5): Initialize, Wait for Event, User State 1, User State 2, Exit
    initial state: Initialize
    default state: none
    states are the items of the type definition State.ctl as saved in this file; the type definition itself was not read
    model: exact (every next state and stop condition is a constant on the diagram)
    transitions:
      Initialize -> Wait for Event; never stops the loop
      Wait for Event -> Exit; never stops the loop
      User State 1 -> Initialize; never stops the loop
      User State 2 -> Initialize; never stops the loop
      Exit -> Exit; stops the loop
    PROVEN [BZ201] state `User State 1` can never be reached (going by the items of the type definition `State.ctl` as saved in this file)
      why: no chain of next-state values the diagram can write leads from the initial state to this one, and every value the analysis could not follow was assumed to lead anywhere
    PROVEN [BZ201] state `User State 2` can never be reached (going by the items of the type definition `State.ctl` as saved in this file)
      why: ...

Checked 1 file: 1 with a state machine (1 state machine, 0 string/queue machines), 0 without.
2 findings: 2 proven, 0 structural, 0 heuristic.
```

A state machine here is the classic pattern: a while loop, a shift register,
and a case structure inside the loop whose selector is wired from the
register. The selector is an enumeration, a string or an integer. Each case
is a state, named by its case label; a case that handles several values or
a range is one state. `#38` and `#49` are the object ids that
`inspect --format json` prints.

For every machine the report gives its states, the transitions, the initial
state (the constant wired to the register from outside the loop), the
default state, and whether the model is exact. `--format json` has the same
data and `--format dot` draws the transition graphs: a double border for a
state that stops the loop, a dashed one for a state proven unreachable, and
a dashed edge to `?` where the next state could not be followed. Files
without a state machine are counted, not listed. In the text report a state
name that holds a comma (a case for several values, `A, B`) is put in
backticks where names are listed. A state whose next state could not be
followed may go to any state; the list of transitions (`transitions` in the
JSON) holds the known ones only. The exit status is 0 when there are no
findings, 1 when there are, and 2 for an error such as a path that does not
exist. A file that cannot be parsed is listed and does not change the
status, and neither does a file whose block diagram did not decode
completely (see below).

The same findings are the lint rules `BZ201`, `BZ202` and `BZ203`, so they
also appear in `buzzard check`, in SARIF, in the language server and in the
MCP tools.

### What is proven

The model **over-approximates** the diagram. The next state of a case is
followed backwards from the shift register through constants, tunnels, the
Select function and nested case, event, disable and sequence structures.
Where it reaches anything else (a subVI, arithmetic, a control, a local
variable, a property node, a cluster, a wire that cannot be followed) the
case is taken to be able to go to *any* state. A stop condition that is not
a constant is taken to be able to go either way, and an initial state that
is not a constant is unknown. So the program can do no more than the model,
and what the model cannot do, the program cannot do either:

- **`BZ201`, a state that can never be reached** is proven: no sequence of
  next-state values the diagram can write leads to it.
- **`BZ202`, a state from which the loop's condition terminal can never
  stop the loop** is proven: no state that can follow it can make the
  condition terminal end the loop. When that is true of every state the
  terminal never stops the loop at all, which is reported once, as
  information, because a loop that runs until the program is aborted is a
  legitimate design. This says nothing about other ways out, so it is
  labelled `PROVEN` only when the block diagram holds nothing that could
  end the program or abort the VI some other way: no subVI call, call by
  reference, invoke node, property node, call into a library or XNode,
  and no node of a class that is not known to be harmless, anywhere on the
  diagram. When there is one, the finding names the nodes and is labelled
  `STRUCTURAL`. This is the conservative choice: such a node counts
  wherever it sits, also outside the loop, because it may run in the same
  iteration or in parallel.

Each finding is labelled `PROVEN`, `STRUCTURAL` or `HEURISTIC` and carries
one sentence saying why it holds:

- `PROVEN` holds for every execution of the VI as saved in the file.
- `STRUCTURAL` is a fact about what is wired on the diagram. It is not a
  statement about what runs. Besides the `BZ202` case above this is
  **`BZ203`, a constant that no case handles and that can reach the state
  register**: no case is labelled with the value (even ignoring letter
  case), so the default case would run for it. The constant may sit on a
  path that never executes, such as the disabled frame of a disable
  structure. It is reported once per state machine and value, and not for
  a value that only states which always stop the loop can write.
- `HEURISTIC` is a match on names and can be wrong in both directions.

A proof is about the file as it was saved:

- **Type definitions.** When the selector's enumeration is an instance of a
  type definition, the states are the items of the copy the VI stored when
  it was saved. `buzzard` does not open the `.ctl` file and does not compare
  the two, so if the type definition changed afterwards and the VI was not
  saved again, the proof is about the old item list. Every `BZ201` and
  `BZ202` finding about such a machine names the type definition and says
  "as saved in this file".
- **A complete decode.** The decoder skips what it cannot resolve, and a
  wire it skipped would look like a tunnel that was left unwired. It
  records every such loss: a signal, node, terminal or frame that does not
  resolve or is listed twice, a wire end that is no terminal of its diagram,
  two wires into one terminal, an object defined twice, an object that no
  list names, a list that does not hold as many elements as it states, an
  object with a field it cannot name or with the same field twice, and a
  heap whose byte stream breaks a rule that every real one keeps (a
  closing entry that does not match its opening one or carries data, an
  attribute of an unknown kind or given twice, a node left open). A block
  diagram with any loss is listed as not decoded completely and is not
  searched for classic state machines at all. None of the 1,773 real files
  it was checked on has a loss. A selector table is used only when it is
  whole: every entry has its five fields exactly once, every case that is
  not the default has a value, and a case structure that does not say
  which case is the default (which means the first) has no field that a
  case structure is not known to have. A loop's condition terminal is read
  as "stop if true" or "continue if true" only when its flags are present.
- **Values are taken as they are.** What is left is damage that changes a
  value and nothing else, such as the item an enumeration constant holds,
  the text of a case label, the number of the default case or the
  polarity of the condition terminal. Nothing is lost then: the file
  describes another program, completely, and the findings are about that
  program. The file format has no checksum that would tell.

### What is not proven

- **The reverse of the above.** A state the report does not call
  unreachable may still never run, and a loop it does not report may still
  never stop. Whenever the model is not exact, findings can be missing;
  they are never invented.
- **Anything about string/queue state machines.** In a machine in the style
  of the JKI State Machine the register holds a queue of state names as one
  string, and library subVIs split and extend it. That is not a finite
  automaton and it is not modelled. Such a machine is recognised (a string
  case structure selected by an output of `Parse State Queue`), and one
  check is made, labelled `HEURISTIC`: a state name in a string constant
  wired into an `Add State(s) to Queue` call, or into the queue before the
  loop, that matches no case label. A queue string is read as the real
  files use it: one state per line, the name ending at `>>`, with `//` and
  `/* */` comments. Names built at run time are not seen, so the check can
  miss a misspelled state, and it can be wrong about a name that never
  reaches the queue.
- **Termination in general.** "Stop" means that the loop's own condition
  terminal ends the loop, and `BZ202` claims nothing else. A diagram that
  calls the built-in Stop or Quit function gets no `BZ202` at all, and one
  with a subVI call or another node that could end the program gets it as
  `STRUCTURAL` only. What such a node really does is not looked into. An
  error dialog or an abort by the user is outside the model. That a state
  finishes its own work (an event structure that waits forever, an inner
  loop) is not checked.
- **The reading of the file.** The proofs rest on how the binary file is
  decoded: shift registers, case selector tables, tunnels, "use default if
  unwired" and the stop terminal's polarity. Each was checked against some
  1,750 real files saved by versions 13 to 25, and the extraction against
  nine real state machine files, but none of it against a running editor.

The extraction gives up rather than guess. It does not model: a state held
in a cluster or a feedback node; for loops and timed loops; selectors that
are Booleans or error clusters; a selector fed by anything but the
register; strings that are not plain ASCII; an uninitialised register's
first state (it keeps its value between calls); a case structure with more
than 1,024 cases; and which case of an event structure handles which event
(all cases are assumed possible, and a wired timeout is assumed able to run
none).

- **Selector types.** Next states are resolved only for the selector types
  whose table encoding was seen in real files: strings; enumerations of 8,
  16 and 32 bits; `I32`; and `U8` and `I16` as long as every number in the
  table is between 0 and the type's largest value. For `I8`, `U16`, `U32`,
  the 64-bit types, and a `U8` or `I16` table with a number outside that
  range, every next state is unknown, because how a narrow or unsigned
  value is widened into the table was never observed for them.
- **Stacked shift registers are unverified.** None of the 634 loops in the
  real files has one, so how a file stores one is known from third-party
  notes only. A loop gives no state machine unless its registers pair one
  to one: one left terminal per register, and as many left and right
  register terminals on the loop as it has registers.
- **Unwired tunnels of an enumeration** are taken to give the first item,
  the default of the data type, as the product documentation says. This
  was not checked in an editor for a type definition whose control has
  another default value.

The module documentation of `crates/buzzard-lint/src/verify` lists every
assumption.

## Rules

| Code | Severity | Meaning |
| --- | --- | --- |
| `BZ000` | error | File cannot be read, or is not a valid RSRC container or project XML file |
| `BZ001` | warning | Compiled code is not separated from the source file |
| `BZ002` | warning | Saved with a different version than most of the workspace |
| `BZ003` | warning | Dependency linked by a machine-specific absolute path, or a path over the length limit |
| `BZ004` | warning | Leftover breakpoints or suspend-when-called, or a password-protected block diagram |
| `BZ005` | error | Part of a circular dependency (VI calls and class inheritance) |
| `BZ006` | warning | A project, library or class lists a member whose file does not exist on disk |
| `BZ101` | warning or info | An error output on the block diagram is not wired |
| `BZ102` | warning | A reference is created and never reaches anything that could close it |
| `BZ201` | warning | A state of a state machine can never be reached (proven) |
| `BZ202` | warning or info | A state machine has a state from which the loop's condition terminal can never stop the loop, or the terminal can never stop it at all (proven, or structural when a subVI call or a similar node could end the program some other way) |
| `BZ203` | warning | A constant that no case handles can reach the state register of a state machine, so the default case would run for it (structural; heuristic for a string/queue machine) |

`BZ201` to `BZ203` come from the state machine verification; see
[Verification](#verification) for what they do and do not prove. Each
message ends in `(proven)`, `(structural, not proven)` or `(heuristic, not
proven)`. `BZ202` is a warning for a trap state and informational when the
condition terminal never stops the loop at all. `BZ203` is structural for a
classic state machine and a heuristic for a string/queue state machine.

`BZ101` is a warning when the file has automatic error handling off, because
the error is then silently discarded, and informational when it is on. Error
handler subVIs are not reported; ending the chain is their job.

`BZ102` is deliberately conservative. It follows a queue, file, network
connection or similar reference from the built-in function that creates it,
through functions that pass it on, and reports only when every path ends at
an unwired output. A reference that reaches a subVI without coming back out,
a front-panel terminal, a tunnel or anything else it cannot follow is
assumed to be handled.

`BZ002` needs one version that more files use than any other. When two
versions are tied for the lead, nothing is reported.

`BZ003` measures a file's own path from the scan root, so the result is the
same wherever the workspace is checked out. To leave room for the directory
it will be checked out into, lower `--max-path-length`.

In SARIF output, a file given by a relative path is a relative URI reference
and a file given by an absolute path is a `file:///` URI. Both are
percent-encoded.

`BZ006` skips what it cannot check: installation paths such as
`/<vilib>/...`, items stored inside the XML file itself, and the
`Dependencies` list of a project, which the IDE computes on its own.

## Language server

The server analyzes the workspace folders on startup and then watches the
file system, because these files are saved by the LabVIEW IDE rather than
edited as text buffers. `workspace/didChangeWatchedFiles` from the client is
accepted as well.

- Changes are applied one batch at a time, in order, and their diagnostics
  are published in that order. Requests are answered from the last complete
  state and do not wait for a scan or a batch in progress; right after
  startup that state is empty until the first scan is done.
- Renaming, moving or deleting a directory moves or removes every file below
  it.
- The watcher follows the same rules as the initial scan and as `check`:
  hidden files and directories (names starting with `.`), symbolic links and
  anything outside the workspace folders are ignored, whichever way a change
  is reported.
- A file is always named by its path under the workspace folder as the
  client spelled it, also when the folder is reached through a symbolic link
  and the operating system reports the resolved path.
- A file that fails to parse right after it was written is read again a few
  times (for about 0.4 s in total) before `BZ000` is reported, so a file
  caught in the middle of a save does not flash up as broken.
- The process exits with status 0 when the client sends `shutdown` and then
  `exit`, and with status 1 when `exit` arrives without `shutdown`.

| Feature | Behaviour |
| --- | --- |
| `textDocument/publishDiagnostics` | All `BZxxx` findings, updated within about 150 ms of a save |
| `workspace/symbol` | Fuzzy search over VIs, controls, classes, libraries and projects |
| `textDocument/hover` | Saved version, execution flags, dependency and caller counts; for a project, library or class: kind, version, member count and parent class |
| `textDocument/references` | Every file in the workspace that uses the target file |
| `buzzard/exportAst` | Custom request: `{ "uri": ..., "format": "json" or "markdown" }` returns the block diagram of one file, decoded from disk on request |
| `buzzard/inspect` | Custom request: `{ "uri": ... }` returns metadata, callers, callees and owners as JSON; for a project, library or class: the parsed file, its members, owners, parent and child classes |

## Use with AI agents (MCP)

`buzzard mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server over stdio, so an AI agent can call the analysis as tools. Register it
in Claude Code:

```bash
claude mcp add buzzard -- buzzard mcp
```

Or commit a `.mcp.json` at the root of the project to share it with a team:

```json
{
  "mcpServers": {
    "buzzard": { "command": "buzzard", "args": ["mcp"] }
  }
}
```

| Tool | Arguments | Returns |
| --- | --- | --- |
| `buzzard_check` | `paths[]`, `ignore?[]`, `limit?`, `refresh?` | Diagnostics (code, severity, path, message) and a summary |
| `buzzard_verify` | `paths[]`, `format?` (`text`, `json`, `dot`) | The state machines under `paths`: states, transitions, initial state, and findings labelled `PROVEN`, `STRUCTURAL` or `HEURISTIC` |
| `buzzard_inspect` | `path` | Metadata and dependency list of one file, as Markdown and JSON |
| `buzzard_diagram` | `path`, `format?` (`markdown`, `json`, `mermaid`, `dot`, `svg`) | The block diagram |
| `buzzard_callers` | `path`, `root`, `refresh?` | Files under `root` that use the file |
| `buzzard_callees` | `path`, `root`, `refresh?` | Files under `root` that the file uses |
| `buzzard_symbols` | `root`, `query`, `limit?`, `refresh?` | Fuzzy file search |
| `buzzard_rules` | none | The rule table |

Every tool is read-only. Relative paths are resolved against the directory
the server was started in. The parsed workspace is cached per root; each call
compares file sizes and modification times with the cached scan and re-parses
when anything differs, and `refresh: true` forces a re-parse. `buzzard_verify`,
like `buzzard_inspect` and `buzzard_diagram`, reads its files on every call. A failed tool
call (missing or unreadable file, bad argument) is returned as a result with
`isError` set, so the agent can read the reason.

## TDMS measurement files

TDMS is the format data-acquisition software logs test data in. `buzzard
tdms` reads such files with the `buzzard-tdms` crate, a read-only reader
made for test-stand logs of several gigabytes.

```bash
buzzard tdms info run-0042.tdms
buzzard tdms info run-0042.tdms --format json
```

`info` prints the groups, the channels with their types, lengths, units
and sample intervals, every property, the number of segments, and
anything wrong with the file, such as a last segment that a crashed
writer left unfinished. It reads only the file's metadata, so it takes
milliseconds on a file of any size.

```bash
buzzard tdms export run-0042.tdms --out slow.parquet --channels Chamber
buzzard tdms export run-0042.tdms --out pt101.csv --format csv --channels Chamber/PT-101
```

`export` writes one table: a column per channel named `group/channel`, a
leading `time_s` column when the channels share their waveform timing,
and, for Parquet, every property of the file, the groups and the channels
as key-value metadata. `--channels` takes `group/channel` names and group
names; without it every channel is exported. A table suits channels of
one sample rate, so export the slow and the fast channels separately.
Parquet is the default format, compressed with Snappy (`--compression
none` turns that off); `--threads` sets the number of worker threads;
`--raw` exports stored values without applying linear scaling.

Both exit with status 0, or 2 when the file cannot be read or a named
channel does not exist. A file that is damaged at its end is read as far
as it goes, reported by `info`, and is not an error.

The crate's [README](crates/buzzard-tdms/README.md) says exactly which
parts of the format are supported, how truncated files are treated, how
fast the reader is, and how it is tested against generated files, against
[npTDMS](https://github.com/adamreeve/npTDMS) as an oracle, and against
damaged input.

## Editors

[`editors/vscode`](editors/vscode) is a thin VS Code extension over
`buzzard lsp`. It puts the findings in the Problems panel, adds commands for
a file summary, a block diagram preview and the callers of a file, and
offers a read-only viewer for `.vi` and `.ctl` files. It needs the `buzzard`
executable on `PATH`; see its [README](editors/vscode/README.md) for how to
build and install it.

Any other editor with a Language Server Protocol client can run
`buzzard lsp` over stdio.

## Design goals

- **Fast.** Files are parsed in parallel without copying resource payloads.
  A release build checks 1,300 real files from three open-source projects,
  block diagrams included, in about 0.2 s on four cores.
- **Low latency.** A save re-parses only the files that changed, and only
  diagnostics that differ from the last published set are sent.
- **Deterministic.** Output is sorted and independent of thread scheduling;
  the test suite asserts byte-identical output across runs.
- **Safe on hostile input.** Every read is bounds-checked. The test suite
  truncates and corrupts real files and requires an error, never a panic.

The TDMS reader follows the same goals. It opens a 400 MB log in under a
millisecond, reads it at 2 to 5 GB/s on four cores, has no `unsafe` code,
and its tests cut and damage files at every byte.

## Workspace

| Crate | Purpose |
| --- | --- |
| `crates/buzzard-rsrc` | Parser for the binary RSRC resource container |
| `crates/buzzard-lint` | Lint rules, state machine verification, project file parser, dependency graph, SARIF, the `ViInspector` backend trait |
| `crates/buzzard-ls` | Language Server Protocol implementation |
| `crates/buzzard-mcp` | Model Context Protocol server |
| `crates/buzzard-tdms` | Reader for TDMS measurement files, Parquet and CSV export |
| `crates/buzzard-cli` | The `buzzard` binary (`check`, `inspect`, `diff`, `verify`, `tdms`, `lsp`, `mcp`) |

## Development

```bash
cargo test --workspace
```

### Lints and checks

CI runs these, and fails on any warning. Clippy and rustdoc run on a pinned
toolchain, Rust 1.100.0 (see the `lint` job in `.github/workflows/ci.yml`), so
that a new release cannot fail CI with lints that are new in it. Use the same
version locally to see what CI sees:

```bash
cargo fmt --all --check
```

```bash
cargo +1.100.0 clippy --workspace --all-targets --locked -- -D warnings
```

```bash
RUSTDOCFLAGS="-D warnings" cargo +1.100.0 doc --workspace --no-deps --locked
```

```bash
cargo deny check
```

The lint policy lives in `[workspace.lints]` in the root `Cargo.toml`, and
every crate inherits it. It forbids `unsafe`, requires docs on public items,
enables clippy's `pedantic` and `cargo` groups, and adds a few `restriction`
lints:

- no `unwrap()` outside tests;
- no printing from library crates;
- no iterating hash maps, so output stays deterministic;
- no bare `#[allow]`.

When a lint has to be silenced, use `#[expect(lint, reason = "...")]` on the
smallest item that needs it. `deny.toml` configures [cargo-deny], which checks
for RustSec advisories, licenses outside the allow-list, wildcard versions and
crates from unknown sources.

[cargo-deny]: https://github.com/EmbarkStudios/cargo-deny

### Conformance fixtures

`crates/buzzard-rsrc/tests/fixtures` holds real binaries saved by several
LabVIEW versions (14, 21, 23 and 26). Each one has a golden snapshot of its
parsed metadata, plus hand-written expectations for the setting it was saved
to exercise. To cover a new version or file kind, add the file and run:

```bash
BUZZARD_BLESS=1 cargo test -p buzzard-rsrc --test conformance
```

Review the generated snapshot before committing it.

`crates/buzzard-lint/tests/fixtures/project` holds real `.lvproj`, `.lvlib`
and `.lvclass` files; its README records where they come from and what they
established about the format.

`crates/buzzard-lint/tests/fixtures/verify` holds real state machines and
real loops that are not state machines; its README records their sources
and licences. The seven classic enumeration state machines the extraction
was developed against have no licence and are not redistributed: the tests
rebuild their shapes as diagrams, and ignored tests check the real files
when `BUZZARD_FSM_SAMPLES` points at clones of their repositories. One test
changes single bytes of the inflated block diagram heap of every fixture
and checks that a damaged file never gains a proven finding, and that
nothing is gone from a decoded diagram without a recorded loss.

`crates/buzzard-tdms/tests/fixtures` holds one real TDMS recording from a
public data set; its README records the source, the licence and the
checksums. Every other TDMS test file is written at test time by the
crate's own generator. To compare the reader with npTDMS, install npTDMS
and pytest into a virtual environment and name its interpreter:

```bash
BUZZARD_TDMS_ORACLE=/path/to/venv/bin/python cargo test -p buzzard-tdms --test oracle -- --nocapture
```

Without the variable that test passes without comparing anything; the
`tdms-oracle` CI job always runs it. `tools/tdms-bench.sh` measures the
reader's speed on generated files.

### Dependency graph

The graph has three kinds of edge. `Uses` is a VI referencing another file,
`Contains` is a project, library or class listing a member, and `Inherits`
is a class naming its parent. Callers, callees and references follow `Uses`
only. Cycle detection follows `Uses` and `Inherits`; `Contains` is left out
because a member normally refers back to its owner.

A link is matched to a workspace file by its path first. A file has one
identity however its path is spelled, so `buzzard check .` and
`buzzard check "$PWD"` see the same graph, and a file reached through two
roots is analyzed once, under the spelling of the first root. When the path
does not lead to a workspace file:

- A relative path, or an absolute path from another machine, falls back to
  the file name if exactly one workspace file has that name. A symbolic path
  such as `<vilib>/...` or `<userlib>/...` never does: it names an installed
  file, not a workspace file that shares its name. The parent of a class is
  the exception and is still looked up by name.
- On Windows and macOS a match that differs only in letter case is accepted
  when it is the only one. On other systems case always matters.
- Paths older than the `PTH2` form hold the bytes of the saving machine's
  code page. A name with bytes outside ASCII is read as Windows-1252,
  Shift-JIS, GBK, Big5, EUC-KR, Windows-1251, Windows-1250 and Mac Roman in
  turn, and the link is followed when all readings that match a workspace
  file agree on which one.

## Current limitations

- Symbolic links in the file system are not followed when comparing paths:
  a file reached through a symbolic link and through its real location
  counts as two files.
- Hover text and `BZ003` messages show a name from a non-UTF-8 code page as
  stored, one character per byte, even when its link was followed.
- Dependencies are read from the link-info blocks by locating each stored
  path and the name in front of it, rather than by decoding every link
  record variant. This holds on all fixtures but is a heuristic.
- The parent of a class is decoded from `NI.LVClass.ParentClassLinkInfo`.
  The layout was verified on files saved by versions 13.0 and 19.0 only. A
  value with any other layout is treated as "parent unknown", never guessed.
- Item URLs that start with `/` are never resolved, so a member linked by a
  machine-absolute URL is not checked by `BZ006` and gets no graph edge.
- `BZ006` is re-evaluated when a watched `.vi`, `.ctl` or project file
  changes. Deleting a member of another type (a `.dll`, a text file) is only
  noticed at the next such change.
- Built-in functions on the block diagram are stored as a numeric id; nothing
  in the file names them. They are named from a vendored table that covers
  687 ids (see `NOTICE`). An id outside the table is shown as a number. The
  table marks 162 names as verified, meaning its source records that it
  confirmed which function the id is; the other 525 were taken over from an
  export without independent confirmation and may be wrong. Verified and
  unverified names are printed the same way. The names are used for wording
  only: the dataflow rules still reason from terminal types and wiring
  alone.
- Constants are decoded when they are Booleans, integers, enumerations,
  single or double precision numbers or strings; constants of other types
  (arrays, clusters, references, text rings) are not. A constant has no
  position in the decoded diagram, so the pictures do not draw it. Free
  labels and wire routes are not decoded.
- Case selector tables are decoded for Boolean, integer, enumeration,
  string and error cluster selectors. Which event each case of an event
  structure handles, and which frame of a disable structure is enabled,
  are not decoded.
- State machine verification covers the classic pattern only and proves
  only what [Verification](#verification) says it proves.
- The dataflow rules and the diagram's terminal directions were validated
  on files saved by versions 13 to 26. Files saved before version 8 use a
  different heap format and have no decoded diagram.
- VIs inside `.llb` archives, packed libraries and installer packages are
  not opened.
- The dependency graph is rebuilt from in-memory metadata on every change.
  That costs milliseconds at the sizes tested; it is not yet edge-incremental.

## Acknowledgements

The RSRC container layout was learned from the open-source work of
[mefistotelis/pylabview](https://github.com/mefistotelis/pylabview) (MIT) and
[marcpage/pylavi](https://github.com/marcpage/pylavi) (public domain). The
conformance fixtures under `crates/buzzard-rsrc/tests/fixtures` come from
pylavi. The project file fixtures under
`crates/buzzard-lint/tests/fixtures/project` come from
[JKISoftware/JKI-State-Machine-Objects](https://github.com/JKISoftware/JKI-State-Machine-Objects)
(BSD 3-Clause) and [ni/grpc-labview](https://github.com/ni/grpc-labview)
(MIT); their licence texts are kept next to the files. The state machine
fixtures under `crates/buzzard-lint/tests/fixtures/verify` come from
JKISoftware/JKI-State-Machine-Objects,
[JKISoftware/JKI-State-Machine](https://github.com/JKISoftware/JKI-State-Machine)
(BSD 3-Clause) and
[LabVIEW-Open-Source/LV-MQTT-Broker](https://github.com/LabVIEW-Open-Source/LV-MQTT-Broker)
(Zero-Clause BSD), with their licence texts next to them. The names of heap
tags and object classes are generated from pylabview.

The table of built-in function names
(`crates/buzzard-rsrc/src/blocks/heap/primitives.rs`) is generated from
[pragmatest-dev/lvkit](https://github.com/pragmatest-dev/lvkit) (Apache-2.0,
Copyright 2025-2026 Ryan Friedman) by `tools/gen_primitives.py`. Only ids,
function names and a verified flag were taken. `NOTICE` records the commit,
what was changed, and where LVKit got the names from;
`LICENSES/Apache-2.0.txt` is the licence that covers the table. Everything
else in this repository is under the MIT licence in `LICENSE`.

The conformance suite in this repository uses 34 small files. During
development the parser, diagram decoder and renderers are also run over
about 1,300 files from ni/grpc-labview, JKISoftware/JKI-State-Machine and
LabVIEW-Open-Source/LV-MQTT-Broker with
`cargo run --release -p buzzard-rsrc --example corpus -- <dir>`.

The TDMS reader follows the vendor's public description of the format.
Its tests run [adamreeve/npTDMS](https://github.com/adamreeve/npTDMS)
(LGPL-3.0) as a separate program to compare results; none of npTDMS is
included in this repository. The recording under
`crates/buzzard-tdms/tests/fixtures` is from Saullo G. P. Castro, "Impact
hammer test of an Aluminium 6082 T6 plate, 260 x 246 x 1.5 mm, free
boundary conditions", Zenodo,
[doi:10.5281/zenodo.7758683](https://doi.org/10.5281/zenodo.7758683),
under CC BY 4.0.

## Trademark disclaimer

LabVIEW and National Instruments are trademarks of National Instruments
Corporation (NI), now part of Emerson. `buzzard` is an independent open-source
project. It is not affiliated with, endorsed by, or sponsored by NI or
Emerson. Those names are used here only to describe the file formats this
tool reads.

## License

MIT. See [LICENSE](LICENSE).
