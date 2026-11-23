# Agent guide

Humans: see [README.md](README.md). This file lists the repo facts that the skills in `.claude/skills/` read.

## Layout

The workspace has six crates. Each crate depends only on crates listed above it.

| Crate | Owns |
| --- | --- |
| `crates/buzzard-rsrc` | The RSRC parser, the block diagram model (`BlockDiagram` with its `Loss`es), the `.llb` reader (`src/llb.rs`), and the builders for synthetic test files (`builder::RsrcBuilder`, `builder::LlbBuilder`) |
| `crates/buzzard-lint` | Rules, dataflow checks, project XML, the dependency graph and report (`src/deps.rs`), SARIF, rendering, the semantic diff, the state machine extraction (`src/verify`), and archives in a workspace (`src/archive.rs`) |
| `crates/buzzard-ls` | The language server |
| `crates/buzzard-mcp` | The Model Context Protocol server and its tools |
| `crates/buzzard-tdms` | The TDMS reader, its Parquet and CSV export (`src/export`, Parquet behind the `parquet` feature), and `build::FileBuilder` for synthetic test files. It depends on no other workspace crate |
| `crates/buzzard-cli` | The `buzzard` binary. `src/main.rs` dispatches the subcommands `check`, `inspect`, `deps`, `diff`, `verify`, `tdms`, `lsp`, and `mcp`. |

`editors/vscode` is a TypeScript VS Code extension over `buzzard lsp`, outside the Cargo workspace.

`README.md` holds the commands, rules, exit statuses, and limitations. `RELEASING.md` holds the steps for publishing to crates.io. `SECURITY.md` says how to report a vulnerability. The repo has no design records, ADRs, changelog, or changesets.

Each crate has its own `README.md`, `LICENSE`, and `NOTICE`, because `cargo package` takes files from the crate's directory only. The `include` list in each `Cargo.toml` keeps a package to `src`, `examples`, and those three files. Its patterns start with `/`, because a pattern without one matches at any depth.

The `LICENSE` files of the crates and of `editors/vscode` are copies of the root file. Only `crates/buzzard-rsrc/NOTICE` carries the LVKit and pylabview sections of the root `NOTICE`, for the two generated tables in `src/blocks/heap`.

`crates/buzzard-cli/tests/packaging.rs` checks the crates, and `npm test` in `editors/vscode` checks the extension's `LICENSE`. After a change to the root `LICENSE` or `NOTICE`, copy the change into those files.

The code examples in the crate READMEs compile as doctests (`ReadmeDoctests` in the `src/lib.rs` of each library crate).

## Checks

CI is `.github/workflows/ci.yml`. It runs every job on every pull request, merge group, and push to `main`, with no path filters. Run the same commands locally before a PR:

| CI job | Local command |
| --- | --- |
| `fmt` | `cargo fmt --all --check` |
| `lint` | `cargo +1.100.0 clippy --workspace --all-targets --locked -- -D warnings` and `RUSTDOCFLAGS="-D warnings" cargo +1.100.0 doc --workspace --no-deps --locked`. The `lint` job pins this toolchain |
| `test` | `cargo test --workspace --locked`, on Ubuntu, macOS, and Windows |
| `msrv` | `cargo +1.91 check --workspace --all-targets --locked`, using the `rust-version` in `Cargo.toml` |
| `deny` | `cargo deny --all-features --locked check`, configured in `deny.toml` |
| `package` | `cargo package --workspace --locked`, with Cargo 1.90 or later. It builds every crate from its packaged files and publishes nothing. Delete `target/package` afterwards |
| `vscode` | In `editors/vscode`: `npm ci`, `npm run lint`, `npm run compile`, `npm test`, and `npm run package`. Build the binary first with `cargo build --locked -p buzzard-cli`. For `npm test`, set `BUZZARD_BIN` to the binary and `BUZZARD_FIXTURE` to a `.vi` fixture |
| `tdms-oracle` | `BUZZARD_TDMS_ORACLE=<python with npTDMS and pytest> BUZZARD_TDMS_ORACLE_STRICT=1 cargo test -p buzzard-tdms --test oracle`, on Ubuntu. Without the first variable the test passes without comparing anything |
| `ci` | None. It fails when any job above failed or was cancelled. Use it for branch protection |

CI uses no secrets, so pull requests from forks run every job.

`.github/workflows/release.yml` is not a check. It publishes to crates.io and starts by hand only (`workflow_dispatch`). See `RELEASING.md`. Do not run `cargo publish` without `--dry-run`, create a tag, or add a trigger to that workflow. A published version is permanent, and the owner starts every release.

To run one integration test file, use `cargo test -p <crate> --test <file>`. To run unit tests by name, use `cargo test -p <crate> --lib <name>`.

The lint policy is `[workspace.lints]` in the root `Cargo.toml`, which every crate inherits. `clippy.toml` lets tests unwrap and print. To suppress one lint, put `#[expect(<lint>, reason = "...")]` on the smallest item that needs it. A bare `#[allow]` fails the `lint` job.

## Tests

- Unit tests sit beside the code in `#[cfg(test)] mod tests` or a `tests.rs` submodule. Integration tests live in `crates/<crate>/tests/`.
- The tests in `crates/buzzard-cli/tests/` run the built binary and write synthetic `.vi` files with the helpers in `tests/common/mod.rs`.
- `crates/buzzard-rsrc/tests/conformance.rs` compares the real binaries in `tests/fixtures` with the snapshots in `tests/snapshots`. To regenerate them, run `BUZZARD_BLESS=1 cargo test -p buzzard-rsrc --test conformance`. Review the diff before you commit.
- Fixture files are byte-exact copies of upstream files (`.gitattributes`). Do not edit them. A new fixture records its source and license in the README of its fixture directory and in the "Test fixtures" section of `NOTICE`.
- No `.llb` archive with members is in the repo. The archive tests build theirs with `LlbBuilder`, whose layout was read off public archives saved by LabVIEW 5.1 to 2023. `encode_adir` writes the bytes the reader skips as those archives have them, not as zeros. No real archive with zlib (`ZCRF`) members was found. Do not add a third-party archive without its source and license.
- `crates/buzzard-lint/tests/verify.rs` tests the state machine extraction on the files in `tests/fixtures/verify` and on diagrams built by `tests/verify/build.rs`. Real classic enumeration machines have no license and are not in the repo. Two tests check them when `BUZZARD_FSM_SAMPLES` points at clones (run with `-- --ignored`).
- The TDMS tests write their files at test time with `FileBuilder`, which keeps its own model of what each channel holds. A failing proptest run in `tests/property.rs` prints its seed, and `PROPTEST_RNG_SEED=<seed>` repeats it. With `-- --ignored`, `tests/real.rs` reads every file of the directory that `BUZZARD_TDMS_SAMPLES` names.
- `crates/buzzard-tdms/tests/oracle.rs` compares the reader with npTDMS, run as a separate program through `tests/oracle/dump.py`. A disagreement that is not in `KNOWN_DIFFERENCES` fails, and so does an entry that no longer occurs. Do not loosen the comparison. Fix the reader, or add an entry with its reason to `KNOWN_DIFFERENCES`.
- npTDMS is LGPL-3.0. Do not read, copy, or port its source, its tests, or its fixtures into this repo. Use it only through `dump.py`, as a program.

These invariants have tests that enforce them:

- Output is byte-identical across runs (`check_output_is_byte_identical_across_runs`, `parsing_is_deterministic`).
- Truncated or corrupted input returns an error and never panics (`hostile_input_never_panics`, `hostile_archives_never_panic`).
- A finding labelled `PROVEN` is never false. When anything about a diagram was lost or is not understood, the result is unknown, never a default. The decoder records everything it skips or cannot trust as a `Loss`, and a diagram with a loss is not searched for classic state machines (`every_loss_of_the_decoder_is_recorded`).
- A decoder change that skips something new records a loss for it. A decoder change that reads an absent field as a default first makes sure the field cannot be hiding.
- `a_damaged_heap_never_adds_a_proven_finding` changes single bytes of each fixture's inflated heap. It checks that no proven finding is added and that nothing leaves a diagram without a recorded loss. `BUZZARD_HEAP_DAMAGE_STEP` sets the distance between changed bytes (499 by default). A step of 1 takes hours, so use `--release`.
- That test has one exception: damage that keeps the structure of the diagram and changes only a value, as the test's `skeleton` function defines. The result is a complete diagram of another program, so the test counts its proofs and does not fail.
- An `.llb` archive is untrusted. The reader allocates nothing from a declared count or size. It refuses a member that declares more than `llb::MAX_MEMBER_BYTES`, or more than its stream could hold, before it inflates anything (`a_declared_size_of_gigabytes_is_refused_quickly`).
- The size of an archive bounds its cost. Two content resources may not share bytes of the file (`content_resources_that_share_bytes_are_refused`). A scan stops reading compressed members once they would inflate to more than `archive::MAX_INFLATED_BYTES` together (`the_compressed_members_of_an_archive_share_one_budget`).
- A member name is data, never a path. A member becomes a workspace file only under `<archive path>/<name>`, and only if the name is one path component without ASCII control characters (`archive::is_addressable`). Build the path with `archive::member_path`, never with `Path::join`, which a name could redirect. `archive::split` refuses the same names.
- `a_member_name_never_escapes_or_aliases_another_file` and `a_member_name_cannot_forge_a_line_of_output` enforce the name rule.
- A member is opened when its type in the directory or its name says VI, control, or project file, so a changed type code cannot hide a VI (`the_type_in_the_directory_does_not_hide_a_member`).
- An archive inside an archive is never opened, and a link that leads into one is unknown, not missing (`Archives::contains`). A directory layout that the reader does not know is an error on the archive, never a guess at its members.
- The state machine model over-approximates the diagram. Anything the extraction cannot follow becomes `Successors::Unknown`, `Stop::Maybe`, or an unknown initial state (`an_unknowable_next_state_is_unknown_and_claims_nothing`). A change that makes the extraction follow something new documents the assumption in `crates/buzzard-lint/src/verify/extract.rs` and adds a test.
- The TDMS reader agrees with the generator's model (`every_shape_of_the_cross_product_reads_back`, `random_segment_sequences_read_back`) and with npTDMS (`the_reader_agrees_with_nptdms`). A new format feature gets a case in the generator first.
- The TDMS reader never panics, never hangs, and never allocates more than a file accounts for. Arithmetic on sizes from a file is checked (`a_file_cut_at_any_byte_gives_a_prefix_of_every_channel`, `a_file_cannot_make_the_index_grow_without_paying_for_it`). The crate has no `unsafe` code and does not memory-map. Reads go through the `ReadAt` trait.
- A truncated or unfinished TDMS file gives a prefix of every channel and an `Anomaly`, never invented values. Anything else that contradicts the format is an `Error` with a byte offset. A new way of recovering data adds an `Anomaly` variant and a row to the table in the crate's README (`anomalies_say_exactly_what_is_wrong`).
- The reader believes an index file only after it compared the lead-ins and metadata with the data file's, byte for byte: in every segment of a file of up to 64 segments, and in evenly spaced segments of a longer one. Do not make the reader believe more (`an_index_file_of_a_long_file_is_checked_in_evenly_spaced_segments`).

## Contracts

Users and other tools depend on these surfaces. Check them when you judge a change's blast radius. The named type or constant defines each one.

- Rule codes are defined in `RULES` in `crates/buzzard-lint/src/rules.rs`. Users pass them to `--ignore`, and SARIF uses them as `ruleId`. Renaming a code breaks user configs and code scanning history.
- Each state machine finding (`BZ201`, `BZ202`, `BZ203`) carries a `Confidence`: `PROVEN`, `STRUCTURAL`, or `HEURISTIC` in text, lower case in the JSON field `confidence`. Do not label a finding proven unless it follows from the over-approximating model. A fact about the wiring that says nothing about what runs is structural.
- In `check` output the message of a state machine finding ends in `(proven)`, `(structural, not proven)`, or `(heuristic, not proven)` (`Confidence::suffix`).
- The exit statuses that `--help` prints are the `after_help` text of each subcommand in `crates/buzzard-cli/src`. Change that text together with the statuses below.
- `check` prints `human`, `json`, or `sarif`. It exits 0 with no problems, 1 with any unless `--exit-zero` is set, and 2 for an unknown `--ignore` code or a path that does not exist. A file, archive, or member that cannot be read is a `BZ000` problem, not an error.
- `inspect` prints `markdown`, `json` (the serialized `BlockDiagram`), `svg`, `dot`, or `mermaid`, and exits 2 when it cannot read the file, or when `svg`, `dot`, or `mermaid` is asked of a file with no block diagram. On an `.llb` file it prints the archive's directory, and the three diagram formats exit 2 and ask for a member.
- An archive built into an application (`llb::is_program_tree`) has no directory and is a plain resource file to every command.
- `diff` prints `text`, `markdown`, or `json`. It exits 0 when the files are the same, 1 when they differ, and 2 on an error. `diff` of two `.llb` files exits 2 and asks for members, unless their bytes are the same.
- `verify` prints `text`, `json` (the serialized `Verified`), or `dot`. It exits 0 with no findings, 1 with any, and 2 on an error. A file that does not parse or decode completely is listed and does not change the status.
- `deps` prints `text`, `json` (the serialized `DepsReport`), or `markdown`. It exits 0 whatever the report holds, and 2 for a path that does not exist or a `--root` that cannot be used. A `--root` that no link is under is listed in `unused_roots` and is not an error.
- A `deps` target has a `Status` of `found`, `missing`, or `unchecked`. It is `unchecked` unless `--root` gave its root a directory, and also when it is inside an `.llb` that cannot be read. Do not report a file as found or missing without a look at the disk.
- `deps::package` is the package rule, and tools that group by package depend on it. `DependencyGraph::link_target`, `DependencyGraph::parent_target`, and `rules::member_is_missing` decide whether a relative link is missing for both `deps` and `BZ006`, so the report and `check` never disagree.
- A member path names a member of an `.llb` archive: the archive's path, a separator, and the member's name (`lib/util.llb/Init.vi`). Every command and MCP tool that takes a file accepts one, and reports print members that way. A member is one file in any letter case (`archive::canonical`, `Workspace::stored_path`). A member that its archive on disk does not hold is a path that does not exist.
- SARIF and the language server never show a member path as a location. They locate the finding at the `.llb` file and start the message with the member's name (`Diagnostic::on_disk`). SARIF adds a `logicalLocations` entry of `kind` `member` and the partial fingerprint `buzzardMember/v1`.
- `buzzard mcp` serves the tools in `NAMES` (`crates/buzzard-mcp/src/tools.rs`) over stdio. Agents depend on the tool names and argument schemas. Unlike the CLI, findings do not make `buzzard_verify` an error result. `buzzard_deps` returns `deps::Overview` counts by default and the full report with `detail` `files`.
- The language server implements the standard methods listed in `README.md` and the custom requests `buzzard/inspect` and `buzzard/exportAst` (`crates/buzzard-ls/src/server.rs`).
- `tdms info` prints `text` or `json` (`InfoJson` in `crates/buzzard-cli/src/tdms_cmd.rs`) and exits 0, or 2 when the file cannot be opened. Anomalies are part of the report and do not change the status.
- `tdms export` exits 0, or 2 on any error, and leaves no output file behind on an error. Columns are named `group/channel`, with a leading `time_s` when the channels share their timing. `crates/buzzard-tdms/src/export/parquet.rs` lists the Parquet metadata keys. The same input and options give the same bytes.
- The public APIs of `buzzard-tdms` and `buzzard-rsrc`, builders included, are published surfaces once released. A breaking change needs a new minor version while the crates are at 0.x. The other library crates are published only so that `cargo install buzzard-cli` works, and their API may change in any release.
- Public enums that may grow are `#[non_exhaustive]`. `SelectorMatch`, `PathKind`, `FileKind`, and `Direction` in `buzzard-rsrc` are not, because `buzzard-lint` matches them exhaustively.
- The six crate names on crates.io and the binary name `buzzard` are permanent once published.
- Input formats are binary RSRC files (`RSRC_EXTENSIONS`), `.llb` archives of them, project XML files (`.lvproj`, `.lvlib`, `.lvclass`), and TDMS files (`.tdms`, with an optional `.tdms_index` beside them).

## Running and measuring

- Run the CLI with `cargo run -p buzzard-cli -- check <path>`. Run the language server over stdio with `cargo run -p buzzard-cli -- lsp`. There are no ports, services, seed data, or credentials, so instances in separate worktrees do not collide.
- The fixtures under `crates/buzzard-rsrc/tests/fixtures`, `crates/buzzard-lint/tests/fixtures/project`, and `crates/buzzard-tdms/tests/fixtures` are real inputs to run against.
- To time `check`, run `cargo build --release` and then `target/release/buzzard check <dir>`. To check that the parser accepts every file in a directory, run `cargo run --release --example corpus -- <dir>`.
- To measure the TDMS reader, run `tools/tdms-bench.sh [SIZE_MB] [DIR]`. It writes each file under `target/tdms-bench` by default and deletes it afterwards. Update the table in the crate's `README.md` when a change moves the numbers.

## Commits and pull requests

- Commit subjects and PR titles are short imperative sentences in sentence case, without Conventional Commit prefixes. For example: "Add dataflow rules BZ101 and BZ102 and block diagram export".
- There is no PR template. Impact statements go in the PR description. Its readers are LabVIEW developers who run `buzzard check` in CI or use the language server in an editor.

## Skills

`.claude/skills/` holds skills copied unchanged from [jpoehnelt/skills](https://github.com/jpoehnelt/skills) at commit `0eefeeebdc08f9f4d085af9ff814828f4d3e5690`. To change one, change it upstream and copy it again. Do not edit the copy. A skill made for this repo only, such as a `verify-` skill, also goes in `.claude/skills/`. `ls .claude/skills` lists them.

The upstream skills for Effect, Svelte, Alchemy, nix-darwin, Sentry, slides, and entity state machines are not copied because this repo uses none of them. Some copied skills mention them for repos that do. Those lines do not apply here.
