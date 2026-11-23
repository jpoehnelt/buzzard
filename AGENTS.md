# Agent guide

This file lists the repo facts that the skills in `.claude/skills/` read. The guide for people is [README.md](README.md).

## Layout

The workspace has six crates. Each crate depends only on crates listed above it.

| Crate | Owns |
| --- | --- |
| `crates/buzzard-rsrc` | The RSRC parser, the block diagram model (`BlockDiagram` with its `Loss`es), the `.llb` reader (`src/llb.rs`), and the builders for synthetic test files (`builder::RsrcBuilder`, `builder::LlbBuilder`) |
| `crates/buzzard-lint` | Rules, dataflow checks, the project file parser (`src/project`), the dependency graph and report (`src/deps.rs`), SARIF, rendering, the semantic diff, the state machine extraction (`src/verify`), and archives in a workspace (`src/archive.rs`) |
| `crates/buzzard-ls` | The language server |
| `crates/buzzard-mcp` | The Model Context Protocol server and its tools |
| `crates/buzzard-tdms` | The TDMS reader, its Parquet and CSV export (`src/export`, Parquet behind the `parquet` feature), and `build::FileBuilder` for synthetic test files. It depends on no other workspace crate |
| `crates/buzzard-cli` | The `buzzard` binary. `src/main.rs` dispatches the subcommands `check`, `inspect`, `deps`, `diff`, `verify`, `tdms`, `lsp`, and `mcp` |

`editors/vscode` is a VS Code extension, written in TypeScript, that starts `buzzard lsp`. It is outside the Cargo workspace.

`README.md` holds the commands, rules, and limitations. `buzzard <command> --help` prints the options, formats, and exit statuses of each command. `RELEASING.md` holds the steps for publishing to crates.io. `SECURITY.md` says how to report a vulnerability. The repo has no design records, ADRs, changelog, or changesets.

Each crate has its own `README.md`, `LICENSE`, and `NOTICE`, because `cargo package` takes files from the crate's directory only. The `include` list in each `Cargo.toml` keeps a package to `src`, `examples`, and those three files. Each pattern starts with `/`, because a pattern without a leading `/` matches at any depth.

The `LICENSE` files of the crates and of `editors/vscode` are copies of the root file. Only `crates/buzzard-rsrc/NOTICE` carries the LVKit and pylabview sections of the root `NOTICE`. Those sections cover the generated tables in `crates/buzzard-rsrc/src/blocks/heap`.

The tests in `crates/buzzard-cli/tests/packaging.rs` check each crate's `LICENSE` and `NOTICE` against the root files. `npm test` in `editors/vscode` checks the extension's `LICENSE`. After a change to the root `LICENSE` or `NOTICE`, make the same change in the crate files and in the extension's `LICENSE`.

The code examples in the crate READMEs compile as doctests (`ReadmeDoctests` in the `src/lib.rs` of each library crate).

## Checks

CI is `.github/workflows/ci.yml`. It runs every job on every pull request, merge group, and push to `main`, with no path filters. Before a PR, run the same commands locally.

| CI job | Local command |
| --- | --- |
| `fmt` | `cargo fmt --all --check` |
| `lint` | `cargo +1.100.0 clippy --workspace --all-targets --locked -- -D warnings` and `RUSTDOCFLAGS="-D warnings" cargo +1.100.0 doc --workspace --no-deps --locked`. The `lint` job pins this toolchain |
| `test` | `cargo test --workspace --locked`, on Ubuntu, macOS, and Windows |
| `msrv` | `cargo +1.91 check --workspace --all-targets --locked`, using the `rust-version` in `Cargo.toml` |
| `deny` | `cargo deny --all-features --locked check`, configured in `deny.toml` |
| `package` | `cargo package --workspace --locked`, with Cargo 1.90 or later. It builds every crate from its packaged files and publishes nothing. Delete `target/package` afterwards |
| `vscode` | In `editors/vscode`: `npm ci`, `npm run lint`, `npm run compile`, `npm test`, and `npm run package`. Build the binary first with `cargo build --locked -p buzzard-cli`. For `npm test`, set `BUZZARD_BIN` to the binary and `BUZZARD_FIXTURE` to a `.vi` fixture. Without those two variables, `npm test` skips the test that runs the binary. CI does not run the tests inside VS Code. To run them, run `BUZZARD_BIN=<absolute path> xvfb-run -a npm run test:integration`. They need a display, and on Linux `xvfb-run` gives them one |
| `tdms-oracle` | `BUZZARD_TDMS_ORACLE=<python with npTDMS and pytest> BUZZARD_TDMS_ORACLE_STRICT=1 cargo test -p buzzard-tdms --test oracle`, on Ubuntu. When `BUZZARD_TDMS_ORACLE` is not set, the test compares nothing and passes |
| `ci` | None. It fails when any job above failed or was cancelled. Use it for branch protection |

CI uses no secrets, so pull requests from forks run every job.

`.github/workflows/release.yml` is not a check. It publishes to crates.io and starts by hand only (`workflow_dispatch`). `RELEASING.md` holds the steps. A published version is permanent, and the owner starts every release. Do not run `cargo publish` without `--dry-run`. Do not create a tag. Do not add a trigger to that workflow.

To run one integration test file, use `cargo test -p <crate> --test <file>`. To run unit tests by name, use `cargo test -p <crate> --lib <name>`.

Every crate inherits the lint policy in `[workspace.lints]` in the root `Cargo.toml`. `clippy.toml` lets tests unwrap and print. To suppress one lint, put `#[expect(<lint>, reason = "...")]` on the smallest item that needs it. A bare `#[allow]` fails the `lint` job.

## Tests

The tests are laid out as follows.

- Unit tests are beside the code in `#[cfg(test)] mod tests` or a `tests.rs` submodule. Integration tests are in `crates/<crate>/tests/`.
- The tests in `crates/buzzard-cli/tests/` run the built binary and write synthetic `.vi` files with the helpers in `tests/common/mod.rs`.
- `crates/buzzard-rsrc/tests/conformance.rs` compares the real binaries in `tests/fixtures` with the snapshots in `tests/snapshots`. To regenerate the snapshots, run `BUZZARD_BLESS=1 cargo test -p buzzard-rsrc --test conformance`. Review the diff before you commit.
- Fixture files are byte-exact copies of upstream files (`.gitattributes`). Do not edit them. A new fixture records its source and license in the README of its fixture directory and in the "Test fixtures" section of `NOTICE`.
- The repo holds no `.llb` archive with members. The archive tests build their archives with `LlbBuilder`. Its layout was read off public archives that LabVIEW 5.1 to 2023 saved. `encode_adir` writes the bytes that the reader skips as those archives have them, not as zeros. No real archive with zlib (`ZCRF`) members was found. Do not add a third-party archive without its source and license.
- `crates/buzzard-lint/tests/verify.rs` tests the state machine extraction on the files in `tests/fixtures/verify` and on block diagrams built by `tests/verify/build.rs`. Real classic enumeration machines have no license, so the repo holds none. The tests marked `#[ignore]` check them when `BUZZARD_FSM_SAMPLES` points at clones. To run those tests, add `-- --ignored`.
- The TDMS tests write their files at test time with `FileBuilder`. `FileBuilder` keeps its own model of what each channel holds. A failing proptest run in `tests/property.rs` prints its seed, and `PROPTEST_RNG_SEED=<seed>` repeats it. With `-- --ignored`, `tests/real.rs` reads every file in the directory that `BUZZARD_TDMS_SAMPLES` names.
- `crates/buzzard-tdms/tests/oracle.rs` compares the reader with npTDMS. The test runs npTDMS as a separate program through `tests/oracle/dump.py`. A disagreement that is not in `KNOWN_DIFFERENCES` fails the test, and so does an entry that no longer occurs. Do not loosen the comparison. Fix the reader, or add an entry with its reason to `KNOWN_DIFFERENCES`.
- npTDMS is LGPL-3.0. Do not read, copy, or port its source, its tests, or its fixtures into this repo. Use npTDMS only through `dump.py`, as a program.

The tests named in parentheses enforce these invariants.

- Output is byte-identical across runs (`check_output_is_byte_identical_across_runs`, `parsing_is_deterministic`).
- Truncated or corrupted input returns an error and never panics (`hostile_input_never_panics`, `hostile_archives_never_panic`).
- A finding labeled `PROVEN` is never false. When the decoder lost or did not understand anything about a block diagram, the result is unknown, never a default. The decoder records everything that it skips or cannot trust as a `Loss`. The extraction does not search a block diagram that has a `Loss` for classic state machines (`every_loss_of_the_decoder_is_recorded`).
- A decoder change that skips something new records a `Loss` for it. A decoder change that reads an absent field as a default first makes sure the field cannot be hiding.
- `a_damaged_heap_never_adds_a_proven_finding` changes single bytes of each fixture's inflated heap. The test checks that the damage adds no proven finding and leaves no block diagram without a recorded `Loss`. `BUZZARD_HEAP_DAMAGE_STEP` sets the distance between changed bytes (499 by default). A step of 1 takes hours, so use `--release`.
- That test has one exception: damage that keeps the structure of the block diagram and changes only a value, as the test's `skeleton` function defines. The damaged file is a complete block diagram of another program, so the test counts its proofs and does not fail.
- An `.llb` archive is untrusted. The reader allocates nothing from a declared count or size. Before the reader inflates anything, it refuses a member that declares more than `llb::MAX_MEMBER_BYTES`, or more than the member's stream could hold (`a_declared_size_of_gigabytes_is_refused_quickly`).
- The size of an archive bounds its cost. Two content resources may not share bytes of the file (`content_resources_that_share_bytes_are_refused`). A scan stops reading the compressed members of an archive once they would inflate to more than `archive::MAX_INFLATED_BYTES` together (`the_compressed_members_of_an_archive_share_one_budget`).
- A member name is data, never a path. A member becomes a workspace file only under `<archive path>/<name>`, and only if the name is one path component without ASCII control characters (`archive::is_addressable`). On Windows, `is_addressable` also refuses a name with a colon, because `C:x.vi` names another drive there. Build the path with `archive::member_path`, never with `Path::join`, because a name can redirect `Path::join`. `archive::split` refuses the same names.
- `a_member_name_never_escapes_or_aliases_another_file` and `a_member_name_cannot_forge_a_line_of_output` enforce the name rule.
- A scan opens a member when the member's type in the archive's directory or the member's name says VI, control, or project file. A changed type code therefore cannot hide a VI (`the_type_in_the_directory_does_not_hide_a_member`).
- buzzard never opens an archive inside an archive. A link that leads into such an archive is unknown, not missing (`Archives::contains`). When the reader does not know the layout of an archive's directory, it reports an error on the archive and never guesses at the members.
- The state machine model over-approximates the block diagram. Anything the extraction cannot follow becomes `Successors::Unknown`, `Stop::Maybe`, or an unknown initial state (`an_unknowable_next_state_is_unknown_and_claims_nothing`). A change that makes the extraction follow something new documents the assumption in `crates/buzzard-lint/src/verify/extract.rs` and adds a test.
- The TDMS reader agrees with the generator's model (`every_shape_of_the_cross_product_reads_back`, `random_segment_sequences_read_back`) and with npTDMS (`the_reader_agrees_with_nptdms`). A new format feature gets a case in the generator first.
- The TDMS reader never panics, never hangs, and never allocates more than a file accounts for. The reader uses checked arithmetic on sizes that come from a file (`a_file_cut_at_any_byte_gives_a_prefix_of_every_channel`, `a_file_cannot_make_the_index_grow_without_paying_for_it`). The crate has no `unsafe` code and does not memory-map. Reads go through the `ReadAt` trait.
- A truncated or unfinished TDMS file gives a prefix of every channel and an `Anomaly`, never invented values. Anything else that contradicts the format is an `Error` with a byte offset. A new way of recovering data adds an `Anomaly` variant and a row to the table in the crate's README (`anomalies_say_exactly_what_is_wrong`).
- The TDMS reader believes an index file only after it compared the lead-ins and metadata with those of the data file, byte for byte. It compares every segment of a file of up to 64 segments, and evenly spaced segments of a longer file. Do not make the reader believe more (`an_index_file_of_a_long_file_is_checked_in_evenly_spaced_segments`).

## Contracts

Users and other tools depend on the following contracts. Check them when you judge the blast radius of a change. The named type or constant defines each contract.

- `RULES` in `crates/buzzard-lint/src/rules.rs` defines the rule codes. Users pass them to `--ignore`, and SARIF uses them as `ruleId`. Renaming a code breaks user configs and code scanning history.
- Each state machine finding (`BZ201`, `BZ202`, `BZ203`) carries a `Confidence`: `PROVEN`, `STRUCTURAL`, or `HEURISTIC` in text, lower case in the JSON field `confidence`. Do not label a finding proven unless the finding follows from the over-approximating model. A fact about the wiring that says nothing about what runs is structural.
- In `check` output the message of a state machine finding ends in `(proven)`, `(structural, not proven)`, or `(heuristic, not proven)` (`Confidence::suffix`).
- The exit statuses that `--help` prints are the `after_help` text of each subcommand in `crates/buzzard-cli/src`. Change that text together with the statuses below.
- `check` prints `human`, `json`, or `sarif`. It exits 0 when no problem is an error or a warning, or when `--exit-zero` is set. It exits 1 when a problem is an error or a warning. It exits 2 for an unknown `--ignore` code, a path that does not exist, or a report that it cannot write. A file, archive, or member that `check` cannot read is a `BZ000` problem and does not make `check` exit 2.
- `inspect` prints `markdown`, `json`, `svg`, `dot`, or `mermaid`. For a VI or a control, the `json` format is `InspectOutput` in `crates/buzzard-cli/src/main.rs`: the serialized `ViMetadata` fields, and the `BlockDiagram` under `diagram`. It exits 2 when it cannot read the file. It also exits 2 when the format is `svg`, `dot`, or `mermaid` and the file has no block diagram. On an `.llb` file, `inspect` prints the archive's directory, and the three diagram formats exit 2 and ask for a member.
- An archive built into an application (`llb::is_program_tree`) has no directory and is a plain resource file to every command.
- `diff` prints `text`, `markdown`, or `json`. It exits 0 when the files are the same, 1 when they differ, and 2 on an error. With `--git` it always exits 0. `diff` of two `.llb` files exits 2 and asks for members, unless the two files hold the same bytes.
- `verify` prints `text`, `json` (the serialized `Verified`), or `dot`. It exits 0 with no findings, 1 with any, and 2 on an error. `verify` lists a file that does not parse or decode completely, and that file does not change the status.
- `deps` prints `text`, `json` (the serialized `DepsReport`), or `markdown`. It exits 0 whatever the report holds, and 2 for a path that does not exist or a `--root` that cannot be used. The report lists a `--root` that no link is under in `unused_roots`, and such a root is not an error.
- A `deps` target has a `Status` of `found`, `missing`, or `unchecked`. The status is `unchecked` unless `--root` gave the target's root a directory. It is also `unchecked` when the target is inside an `.llb` archive that buzzard cannot read. Do not report a file as found or missing without a look at the disk.
- `deps::package` is the package rule, and tools that group by package depend on it. `DependencyGraph::link_target`, `DependencyGraph::parent_target`, and `rules::member_is_missing` decide for both `deps` and `BZ006` whether a relative link is missing, so the `deps` report and `check` never disagree.
- A member path names a member of an `.llb` archive: the archive's path, a separator, and the member's name (`lib/util.llb/Init.vi`). Every command and MCP tool that takes a file accepts a member path, and reports print members as member paths. A member is one file in any letter case (`archive::canonical`, `Workspace::stored_path`). A member that its archive on disk does not hold is a path that does not exist.
- SARIF and the language server never show a member path as a location. They locate the problem at the `.llb` file and start the message with the member's name (`Diagnostic::on_disk`). SARIF adds a `logicalLocations` entry of `kind` `member` and the partial fingerprint `buzzardMember/v1`.
- `buzzard mcp` serves the tools in `NAMES` (`crates/buzzard-mcp/src/tools.rs`) over stdio. Agents depend on the tool names and argument schemas. Findings do not make `buzzard_verify` an error result, although they make the CLI exit 1. `buzzard_deps` returns `deps::Overview` counts by default and the full report with `detail` `files`. `buzzard mcp` exits 0 when the client closes the connection, and 2 when reading a request or writing a response fails.
- The language server implements the standard methods listed in `README.md` and the custom requests `buzzard/inspect` and `buzzard/exportAst` (`crates/buzzard-ls/src/server.rs`). `buzzard lsp` exits 0 after `shutdown` and then `exit`, or when the client closes the connection. It exits 1 after `exit` without `shutdown`, and 2 when the server cannot start.
- `tdms info` prints `text` or `json` (`InfoJson` in `crates/buzzard-cli/src/tdms_cmd.rs`) and exits 0, or 2 when it cannot open the file. Anomalies are part of the report and do not change the status.
- `tdms export` exits 0, or 2 on any error, and leaves no output file behind on an error. It names each column `group/channel`, and it adds a leading `time_s` column when the channels share their timing. `crates/buzzard-tdms/src/export/parquet.rs` lists the Parquet metadata keys. The same input and options give the same bytes.
- Once the crates are released, users depend on the public APIs of `buzzard-tdms` and `buzzard-rsrc`, builders included. While the crates are at 0.x, a breaking change needs a new minor version. The other library crates are published only so that `cargo install buzzard-cli` works, and their APIs may change in any release.
- Public enums that may grow are `#[non_exhaustive]`. `SelectorMatch`, `PathKind`, `FileKind`, and `Direction` in `buzzard-rsrc` are exhaustive, because `buzzard-lint` matches them exhaustively.
- The six crate names on crates.io and the binary name `buzzard` are permanent once published.
- Input formats are binary RSRC files (`RSRC_EXTENSIONS`), `.llb` archives of RSRC files, project files (`.lvproj`, `.lvlib`, `.lvclass`), and TDMS files (`.tdms`, with an optional `.tdms_index` beside them).

## Running and measuring

These commands run and measure buzzard.

- Run the CLI with `cargo run -p buzzard-cli -- check <path>`. Run the language server over stdio with `cargo run -p buzzard-cli -- lsp`. There are no ports, services, seed data, or credentials, so instances in separate worktrees do not collide.
- The fixtures under `crates/buzzard-rsrc/tests/fixtures`, `crates/buzzard-lint/tests/fixtures/project`, and `crates/buzzard-tdms/tests/fixtures` are real inputs to run against.
- To time `check`, run `cargo build --release` and then `target/release/buzzard check <dir>`. To check that the parser accepts every file in a directory, run `cargo run --release --example corpus -- <dir>`.
- To measure the TDMS reader, run `tools/tdms-bench.sh [SIZE_MB] [DIR]`. The script writes each file under `target/tdms-bench` by default and deletes the file afterwards. Update the table in the crate's `README.md` when a change moves the numbers.

## Commits and pull requests

Commits and pull requests follow these conventions.

- Commit subjects and PR titles are short imperative sentences in sentence case, without Conventional Commit prefixes. For example: "Add dataflow rules BZ101 and BZ102 and block diagram export".
- There is no PR template. Impact statements go in the PR description. The readers of a PR description are LabVIEW developers who run `buzzard check` in CI or use the language server in an editor.

## Skills

`.claude/skills/` holds skills copied unchanged from [jpoehnelt/skills](https://github.com/jpoehnelt/skills) at commit `0eefeeebdc08f9f4d085af9ff814828f4d3e5690`. To change one, change it upstream and copy it again. Do not edit the copy. A skill made for this repo only, such as a `verify-` skill, also goes in `.claude/skills/`. `ls .claude/skills` lists them.

The upstream skills for Effect, Svelte, Alchemy, nix-darwin, Sentry, slides, and entity state machines are not copied because this repo uses none of them. Some copied skills mention those tools for repos that use them. Those lines do not apply here.
