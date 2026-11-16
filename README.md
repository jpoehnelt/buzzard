# buzzard

A fast, cross-platform static analyzer and language server for LabVIEW™
projects, written in Rust.

`buzzard` reads binary `.vi` and `.ctl` files and the XML of `.lvproj`,
`.lvlib` and `.lvclass` files directly from disk. It does not need the
LabVIEW IDE or runtime, so it runs on Linux, macOS and Windows CI runners
alike.

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
reported as added or removed. Built-in functions are named by their
function id, because the file does not hold their names.

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
| `buzzard_inspect` | `path` | Metadata and dependency list of one file, as Markdown and JSON |
| `buzzard_diagram` | `path`, `format?` (`markdown`, `json`, `mermaid`, `dot`, `svg`) | The block diagram |
| `buzzard_callers` | `path`, `root`, `refresh?` | Files under `root` that use the file |
| `buzzard_callees` | `path`, `root`, `refresh?` | Files under `root` that the file uses |
| `buzzard_symbols` | `root`, `query`, `limit?`, `refresh?` | Fuzzy file search |
| `buzzard_rules` | none | The rule table |

Every tool is read-only. Relative paths are resolved against the directory
the server was started in. The parsed workspace is cached per root; each call
compares file sizes and modification times with the cached scan and re-parses
when anything differs, and `refresh: true` forces a re-parse. A failed tool
call (missing or unreadable file, bad argument) is returned as a result with
`isError` set, so the agent can read the reason.

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
  A release build checks about 10,800 real VIs in roughly 130 ms on four
  cores.
- **Low latency.** A save re-parses only the files that changed, and only
  diagnostics that differ from the last published set are sent.
- **Deterministic.** Output is sorted and independent of thread scheduling;
  the test suite asserts byte-identical output across runs.
- **Safe on hostile input.** Every read is bounds-checked. The test suite
  truncates and corrupts real files and requires an error, never a panic.

## Workspace

| Crate | Purpose |
| --- | --- |
| `crates/buzzard-rsrc` | Parser for the binary RSRC resource container |
| `crates/buzzard-lint` | Lint rules, project file parser, dependency graph, SARIF, the `ViInspector` backend trait |
| `crates/buzzard-ls` | Language Server Protocol implementation |
| `crates/buzzard-mcp` | Model Context Protocol server |
| `crates/buzzard-cli` | The `buzzard` binary (`check`, `inspect`, `lsp`, `mcp`) |

## Development

```bash
cargo test --workspace
```

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
- The block diagram itself is not decoded yet, so there are no dataflow
  rules.
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
(MIT); their licence texts are kept next to the files.

## Trademark disclaimer

LabVIEW and National Instruments are trademarks of National Instruments
Corporation (NI), now part of Emerson. `buzzard` is an independent open-source
project. It is not affiliated with, endorsed by, or sponsored by NI or
Emerson. Those names are used here only to describe the file formats this
tool reads.

## License

MIT. See [LICENSE](LICENSE).
