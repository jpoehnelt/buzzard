# Agent guide

Humans: see [README.md](README.md). This file lists the repo facts that the skills in `.claude/skills/` read.

## Layout

The workspace has four crates. Each crate depends only on the crates listed above it.

| Crate | Owns |
| --- | --- |
| `crates/buzzard-rsrc` | The binary RSRC container parser, block decoders, and `builder::RsrcBuilder` for synthetic test files |
| `crates/buzzard-lint` | Lint rules, the project XML parser, the dependency graph, SARIF, and the `ViInspector` backend trait |
| `crates/buzzard-ls` | The language server: engine, file watching, and fuzzy symbol search |
| `crates/buzzard-cli` | The `buzzard` binary with the `check`, `inspect`, and `lsp` subcommands |

`README.md` holds the durable docs: usage, rules, language server features, design goals, and current limitations. The repo has no design records, ADRs, changelog, or changesets.

## Checks

CI is `.github/workflows/ci.yml`. It runs every job on every pull request, merge group, and push to `main`, with no path filters. Run the same commands locally before a PR:

| CI job | Local command |
| --- | --- |
| `fmt` | `cargo fmt --all --check` |
| `lint` | `cargo clippy --workspace --all-targets --locked -- -D warnings` and `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` |
| `test` | `cargo test --workspace --locked`, on Ubuntu, macOS, and Windows |
| `msrv` | `cargo +1.91 check --workspace --all-targets --locked`, using the `rust-version` in `Cargo.toml` |
| `ci` | None. It fails when any job above failed or was cancelled, and it is the job meant for branch protection. |

CI uses no secrets, so pull requests from forks run every job.

To run one integration test file, use `cargo test -p <crate> --test <file>`, for example `cargo test -p buzzard-lint --test rules`. To run unit tests by name, use `cargo test -p <crate> --lib <name>`.

Lints are clippy and rustdoc, both with `-D warnings`. There is no custom lint config. To suppress one lint, put `#[allow(<lint>)]` on the item.

## Tests

- Unit tests sit beside the code in `#[cfg(test)] mod tests`, or in a `tests.rs` submodule such as `crates/buzzard-lint/src/project/tests.rs`. Integration tests live in `crates/<crate>/tests/`.
- The tests in `crates/buzzard-cli/tests/` run the built binary and write synthetic `.vi` files with the helpers in `tests/common/mod.rs`.
- `crates/buzzard-rsrc/tests/conformance.rs` parses the real binaries in `tests/fixtures` and compares them with golden snapshots in `tests/snapshots`. To regenerate the snapshots, run `BUZZARD_BLESS=1 cargo test -p buzzard-rsrc --test conformance`, then review the diff before committing.
- Fixture files are byte-exact copies of upstream files (`.gitattributes`). Do not edit them. A new fixture records its source and licence, as `crates/buzzard-lint/tests/fixtures/project/README.md` does.

Two invariants have tests that enforce them:

- Output is byte-identical across runs, for example `check_output_is_byte_identical_across_runs` in `crates/buzzard-cli/tests/cli.rs` and `parsing_is_deterministic` in `crates/buzzard-rsrc/tests/conformance.rs`.
- Truncated or corrupted input returns an error and never panics, for example `hostile_input_never_panics` in `crates/buzzard-rsrc/tests/conformance.rs`.

## Contracts

These are the surfaces that users and other tools depend on. Check them when you judge a change's blast radius.

- Rule codes (`BZ000` and up) are defined in `RULES` in `crates/buzzard-lint/src/rules.rs`. Users pass them to `--ignore`, and SARIF output uses them as `ruleId`. Renaming a code breaks user configs and code scanning history.
- `buzzard check` prints `--format human`, `json`, or `sarif`. `buzzard inspect` prints `--format markdown` or `json`.
- `check` exits 0 when it finds no problems and 1 when it finds any, unless `--exit-zero` is set. A file it cannot parse is a `BZ000` problem, not an error. It exits 2 for an unknown `--ignore` code or a path that does not exist. `inspect` exits 2 when it cannot read the file. CI pipelines gate on these statuses.
- The language server implements the standard methods listed in `README.md`, plus the custom requests `buzzard/inspect` and `buzzard/exportAst` in `crates/buzzard-ls/src/server.rs`. Editor clients depend on their JSON shape.
- Input formats are binary RSRC files (the extensions in `RSRC_EXTENSIONS` in `crates/buzzard-rsrc/src/lib.rs`) and project XML files (`.lvproj`, `.lvlib`, `.lvclass`), saved by the LabVIEW IDE.

## Running and measuring

- Run the CLI with `cargo run -p buzzard-cli -- check <path>` or `cargo run -p buzzard-cli -- inspect <file>`. Run the language server over stdio with `cargo run -p buzzard-cli -- lsp`. There are no ports, services, seed data, or credentials, so instances in separate worktrees do not collide.
- The fixtures under `crates/buzzard-rsrc/tests/fixtures` and `crates/buzzard-lint/tests/fixtures/project` are real inputs to run against.
- `README.md` claims that a release build checks about 10,800 real VIs in roughly 130 ms on four cores. That corpus is not in the repo. Time `target/release/buzzard check <dir>` after `cargo build --release`. To check that the parser accepts every file in a directory, run `cargo run --release --example corpus -- <dir>`.

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
