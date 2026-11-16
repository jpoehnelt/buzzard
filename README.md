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

`BZ006` skips what it cannot check: installation paths such as
`/<vilib>/...`, items stored inside the XML file itself, and the
`Dependencies` list of a project, which the IDE computes on its own.

## Language server

The server analyzes the workspace folders on startup and then watches the
file system, because these files are saved by the LabVIEW IDE rather than
edited as text buffers.

| Feature | Behaviour |
| --- | --- |
| `textDocument/publishDiagnostics` | All `BZxxx` findings, updated within about 150 ms of a save |
| `workspace/symbol` | Fuzzy search over VIs, controls, classes, libraries and projects |
| `textDocument/hover` | Saved version, execution flags, dependency and caller counts; for a project, library or class: kind, version, member count and parent class |
| `textDocument/references` | Every file in the workspace that uses the target file |
| `buzzard/inspect` | Custom request: `{ "uri": ... }` returns metadata, callers, callees and owners as JSON; for a project, library or class: the parsed file, its members, owners, parent and child classes |

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
| `crates/buzzard-cli` | The `buzzard` binary (`check`, `inspect`, `lsp`) |

## Development

```bash
cargo test --workspace
```

### Lints and checks

CI runs these, and fails on any warning:

```bash
cargo fmt --all --check
```

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
```

```bash
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
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

### Dependency graph

The graph has three kinds of edge. `Uses` is a VI referencing another file,
`Contains` is a project, library or class listing a member, and `Inherits`
is a class naming its parent. Callers, callees and references follow `Uses`
only. Cycle detection follows `Uses` and `Inherits`; `Contains` is left out
because a member normally refers back to its owner.

## Current limitations

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
