# Agent guide

Humans: see [README.md](README.md). This file lists the repo facts that the skills in `.claude/skills/` read.

## Layout

The workspace has five crates. Each crate depends only on crates listed above it.

| Crate | Owns |
| --- | --- |
| `crates/buzzard-rsrc` | The binary RSRC container parser, block decoders, and `builder::RsrcBuilder` for synthetic test files |
| `crates/buzzard-lint` | Lint rules, dataflow checks, the project XML parser, the dependency graph, SARIF, diagram rendering, the semantic diff, the state machine checks, and the `ViInspector` backend trait |
| `crates/buzzard-ls` | The language server: engine, ordered update worker, file watching, and fuzzy symbol search |
| `crates/buzzard-mcp` | The Model Context Protocol server and its tools |
| `crates/buzzard-cli` | The `buzzard` binary with the `check`, `inspect`, `diff`, `verify`, `lsp`, and `mcp` subcommands |

`editors/vscode` is a TypeScript VS Code extension over `buzzard lsp`. It is outside the Cargo workspace and has its own `npm` scripts (`lint`, `compile`, `test`, `package`).

`README.md` holds the durable docs: usage, rules, language server features, design goals, and current limitations. The repo has no design records, ADRs, changelog, or changesets.

## Checks

CI is `.github/workflows/ci.yml`. It runs every job on every pull request, merge group, and push to `main`, with no path filters. Run the same commands locally before a PR:

| CI job | Local command |
| --- | --- |
| `fmt` | `cargo fmt --all --check` |
| `lint` | `cargo +1.100.0 clippy --workspace --all-targets --locked -- -D warnings` and `RUSTDOCFLAGS="-D warnings" cargo +1.100.0 doc --workspace --no-deps --locked`. The toolchain is pinned in the `lint` job, not stable. |
| `test` | `cargo test --workspace --locked`, on Ubuntu, macOS, and Windows |
| `msrv` | `cargo +1.91 check --workspace --all-targets --locked`, using the `rust-version` in `Cargo.toml` |
| `deny` | `cargo deny check`, configured in `deny.toml`: RustSec advisories, licenses, wildcard versions and sources |
| `ci` | None. It fails when any job above failed or was cancelled, and it is the job meant for branch protection. |

CI uses no secrets, so pull requests from forks run every job.

To run one integration test file, use `cargo test -p <crate> --test <file>`, for example `cargo test -p buzzard-lint --test rules`. To run unit tests by name, use `cargo test -p <crate> --lib <name>`.

Lints are clippy and rustdoc, both with `-D warnings`. The lint policy is `[workspace.lints]` in the root `Cargo.toml`, which every crate inherits with `[lints] workspace = true`; `clippy.toml` lets tests unwrap and print. To suppress one lint, put `#[expect(<lint>, reason = "...")]` on the smallest item that needs it. A bare `#[allow]` fails the `lint` job.

## Tests

- Unit tests sit beside the code in `#[cfg(test)] mod tests`, or in a `tests.rs` submodule such as `crates/buzzard-lint/src/project/tests.rs`. Integration tests live in `crates/<crate>/tests/`.
- The tests in `crates/buzzard-cli/tests/` run the built binary and write synthetic `.vi` files with the helpers in `tests/common/mod.rs`.
- `crates/buzzard-rsrc/tests/conformance.rs` parses the real binaries in `tests/fixtures` and compares them with golden snapshots in `tests/snapshots`. To regenerate the snapshots, run `BUZZARD_BLESS=1 cargo test -p buzzard-rsrc --test conformance`, then review the diff before committing.
- Fixture files are byte-exact copies of upstream files (`.gitattributes`). Do not edit them. A new fixture records its source and licence, as `crates/buzzard-lint/tests/fixtures/project/README.md` does.
- `crates/buzzard-lint/tests/verify.rs` tests the state machine extraction on the real files in `tests/fixtures/verify` and on diagrams built by `tests/verify/build.rs`. Real classic enumeration machines have no licence and are not in the repo; `real_machines_outside_the_repository` and `damage_to_the_real_machines_proves_nothing_new` check them when `BUZZARD_FSM_SAMPLES` points at clones (run with `-- --ignored`).

Three invariants have tests that enforce them:

- Output is byte-identical across runs, for example `check_output_is_byte_identical_across_runs` in `crates/buzzard-cli/tests/cli.rs` and `parsing_is_deterministic` in `crates/buzzard-rsrc/tests/conformance.rs`.
- Truncated or corrupted input returns an error and never panics, for example `hostile_input_never_panics` and `hostile_state_machine_fields_never_panic` in `crates/buzzard-rsrc/tests/conformance.rs`.
- A damaged file never gains a proven finding. The diagram decoder records everything it skips as a `Loss` on the `Diagram`, and a diagram with a loss is not searched for classic state machines. `a_damaged_heap_never_adds_a_proven_finding` in `crates/buzzard-lint/tests/verify.rs` changes single bytes of the inflated heap of each fixture and checks both that and that nothing is gone from a decoded diagram without a recorded loss; `BUZZARD_HEAP_DAMAGE_STEP=1` makes it try every byte. `every_loss_of_the_decoder_is_recorded` in `crates/buzzard-rsrc/tests/conformance.rs` covers each kind of loss. A decoder change that skips something new records a loss for it.
- The state machine model over-approximates the diagram: anything the extraction cannot follow becomes `Successors::Unknown`, `Stop::Maybe` or an unknown initial state, never a guess. `an_unknowable_next_state_is_unknown_and_claims_nothing` and its neighbours in `crates/buzzard-lint/tests/verify.rs` enforce it. A change that makes the extraction follow something new documents the assumption in `crates/buzzard-lint/src/verify/extract.rs` and adds the case to those tests.

## Contracts

These are the surfaces that users and other tools depend on. Check them when you judge a change's blast radius.

- Rule codes (`BZ000` and up) are defined in `RULES` in `crates/buzzard-lint/src/rules.rs`. Users pass them to `--ignore`, and SARIF output uses them as `ruleId`. Renaming a code breaks user configs and code scanning history. `BZ201`, `BZ202` and `BZ203` are the state machine findings; their messages end in `(proven)`, `(structural, not proven)` or `(heuristic, not proven)`, and `README.md` states what each means. Do not label a finding proven unless it follows from the over-approximating model. A fact about the wiring that says nothing about what runs is structural.
- `buzzard check` prints `--format human`, `json`, or `sarif`. `buzzard inspect` prints `--format markdown`, `json`, `svg`, `dot`, or `mermaid`. `buzzard diff` prints `--format text`, `markdown`, or `json`, and exits 0 when the two files are the same, 1 when they differ, and 2 on an error. `buzzard verify` prints `--format text`, `json`, or `dot`, and exits 0 when it has no findings, 1 when it has any, and 2 on an error; a file it cannot parse is listed and does not change the status.
- The JSON of `buzzard inspect` and `buzzard verify` is the serialized form of `BlockDiagram` (`crates/buzzard-rsrc/src/diagram.rs`) and `Verified` (`crates/buzzard-lint/src/verify/report.rs`). Optional diagram fields (`constant`, `numeric`, `tunnel`, `param`, `selector`, `loop_info`, `losses`) are left out when absent or empty.
- `buzzard mcp` serves the tools listed in `README.md` over stdio. Agents depend on the tool names and their argument schemas.
- `check` exits 0 when it finds no problems and 1 when it finds any, unless `--exit-zero` is set. A file it cannot parse is a `BZ000` problem, not an error. It exits 2 for an unknown `--ignore` code or a path that does not exist. `inspect` exits 2 when it cannot read the file. CI pipelines gate on these statuses.
- The language server implements the standard methods listed in `README.md`, plus the custom requests `buzzard/inspect` and `buzzard/exportAst` in `crates/buzzard-ls/src/server.rs`. Editor clients depend on their JSON shape.
- Input formats are binary RSRC files (the extensions in `RSRC_EXTENSIONS` in `crates/buzzard-rsrc/src/lib.rs`) and project XML files (`.lvproj`, `.lvlib`, `.lvclass`), saved by the LabVIEW IDE.

## Running and measuring

- Run the CLI with `cargo run -p buzzard-cli -- check <path>` or `cargo run -p buzzard-cli -- inspect <file>`. Run the language server over stdio with `cargo run -p buzzard-cli -- lsp`. There are no ports, services, seed data, or credentials, so instances in separate worktrees do not collide.
- The fixtures under `crates/buzzard-rsrc/tests/fixtures` and `crates/buzzard-lint/tests/fixtures/project` are real inputs to run against.
- `README.md` claims that a release build checks 1,300 real files from three open-source projects in about 0.2 s on four cores. That corpus is not in the repo; the README names the three projects. Time `target/release/buzzard check <dir>` after `cargo build --release`. To check that the parser accepts every file in a directory, run `cargo run --release --example corpus -- <dir>`.

## Commits and pull requests

- Commit subjects and PR titles are short imperative sentences in sentence case, without Conventional Commit prefixes. For example: "Add dataflow rules BZ101 and BZ102 and block diagram export".
- There is no PR template. Impact statements go in the PR description. Its readers are LabVIEW developers who run `buzzard check` in CI or use the language server in an editor.

## Skills

`.claude/skills/` holds skills copied unchanged from [jpoehnelt/skills](https://github.com/jpoehnelt/skills) at commit `0eefeeebdc08f9f4d085af9ff814828f4d3e5690`. To change one, change it upstream and copy it again. Do not edit the copy. A skill made for this repo only, such as a `verify-` skill, also goes in `.claude/skills/` and is added to this list.

- Engineering: `eng-change-checklist`, `eng-blast-radius`, `eng-benchmark-checklist`, `eng-tdd`, `eng-testing`, `eng-cutover-verification`, `eng-agent-dx-cli-scale`
- Communication: `comms-how`, `comms-why`, `comms-interrogate`, `comms-technical-writing`, `comms-unslop`, `comms-impact`
- Agents: `agents-swarm`, `agents-reflect`, `agents-correct`, `agents-create-verification-skill`
- GitHub CLI: `gh-pr-feedback-loop`, `gh-pr-commit-review`, `gh-ci-triage`

The upstream skills for Effect, Svelte, Alchemy, nix-darwin, Sentry, slides, and entity state machines are not copied because this repo uses none of them. Some copied skills mention them for repos that do. Those lines do not apply here.
