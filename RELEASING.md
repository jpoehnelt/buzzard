# Releasing

How the six crates get to crates.io. Nothing here runs by itself: a release
starts when the owner starts it.

No crate is published yet. Until the first release, install from a checkout
(`cargo install --path crates/buzzard-cli`).

## What is published

| Crate | For | Depends on |
| --- | --- | --- |
| `buzzard-rsrc` | other programs: the RSRC parser | none |
| `buzzard-tdms` | other programs: the TDMS reader | none |
| `buzzard-lint` | internal part of the tool | `buzzard-rsrc` |
| `buzzard-ls` | internal part of the tool | `buzzard-rsrc`, `buzzard-lint` |
| `buzzard-mcp` | internal part of the tool | `buzzard-rsrc`, `buzzard-lint`, `buzzard-ls` |
| `buzzard-cli` | users: `cargo install buzzard-cli` installs the `buzzard` binary | the five above |

The name `buzzard` on crates.io belongs to someone else, which is why the
binary's crate is `buzzard-cli`.

A crate is published after every crate it depends on:
`buzzard-rsrc`, `buzzard-tdms`, `buzzard-lint`, `buzzard-ls`, `buzzard-mcp`,
`buzzard-cli`. `buzzard-tdms` depends on nothing else here and can be
released alone.

All six share one version, `version` in `[workspace.package]` in the root
`Cargo.toml`.

A package holds the crate's `src` (and `examples`, for `buzzard-rsrc` and
`buzzard-tdms`), its `README.md`, `LICENSE` and `NOTICE`; the `include` list
in each `Cargo.toml` says so. Tests and fixtures are not packaged: the
fixtures are third-party files under their own licences, and the tests read
them. Each crate's `LICENSE` is a copy of the root file, and each `NOTICE`
covers what that package holds: only the one of `buzzard-rsrc` has the LVKit
section. `crates/buzzard-cli/tests/packaging.rs` fails when a copy differs
from the root. After a change to the root `LICENSE`, or to the trademark or
LVKit part of the root `NOTICE`, copy it into the crates.

A published version is permanent. It can be yanked, never changed or
deleted, and its name cannot be given back.

## The workflow

`.github/workflows/release.yml` ("Release" under Actions) runs only when
someone starts it by hand (`workflow_dispatch`). It has two inputs:

- `crates`: `tdms` for `buzzard-tdms` alone, or `all` for the six in
  dependency order.
- `dry_run`: on by default.

It first runs every job of `ci.yml` on the chosen commit. Then:

- With `dry_run` on, it runs `cargo publish --dry-run`: each crate is
  packaged and built from its packaged files, and nothing is uploaded. This
  job has no access to crates.io.
- With `dry_run` off, it publishes from `main` only, in the `release`
  environment. It gets a 30-minute token from crates.io through
  [Trusted Publishing](https://crates.io/docs/trusted-publishing) (OpenID
  Connect, no stored secret). A version that crates.io already has is
  skipped with a notice, so a run that failed halfway can be started again.

Trusted Publishing can be set up for a crate only after its first version
exists. The workflow therefore cannot do the first release of a crate; the
owner does that once, by hand, with a token.

## The first release of a crate

Do this for `buzzard-tdms` first. Do it again later for the other five; the
steps are the same with more crates.

1. On GitHub, run **Release** on `main` with `crates` set to `tdms` (or
   `all`) and `dry_run` on. Go on only when it is green.
2. Check out that same commit of `main` with a clean working tree.
3. On crates.io, under Account Settings, API Tokens, create a token with
   the scope `publish-new` only and the shortest expiry offered. Run
   `cargo login` and paste it. Do not store it in the repository or in its
   secrets.
4. Publish. For `buzzard-tdms` alone:

   ```bash
   cargo publish --locked -p buzzard-tdms
   ```

   For the other five, in this order; each command returns when crates.io
   lists the new version, which the next one needs:

   ```bash
   cargo publish --locked -p buzzard-rsrc
   cargo publish --locked -p buzzard-lint
   cargo publish --locked -p buzzard-ls
   cargo publish --locked -p buzzard-mcp
   cargo publish --locked -p buzzard-cli
   ```

5. Run `cargo logout` and revoke the token on crates.io.
6. For each crate just published, on crates.io open the crate's Settings,
   Trusted Publishing, Add, GitHub, and enter:

   | Field | Value |
   | --- | --- |
   | Repository owner | `jpoehnelt` |
   | Repository name | `buzzard` |
   | Workflow filename | `release.yml` |
   | Environment | `release` |

7. Once, in the repository's Settings, Environments, create the environment
   `release`. Limit its deployment branches to `main` and add yourself as a
   required reviewer, so that every upload waits for an approval.
8. In `README.md`, change the "Install" section to say that
   `cargo install buzzard-cli` works (after `buzzard-cli` is published), and
   remove the "No crate is published yet" paragraph above.

## Later releases

1. Raise the version in the root `Cargo.toml`: `version` in
   `[workspace.package]` and the five `version = "..."` entries of the
   `buzzard-*` crates in `[workspace.dependencies]`. Run `cargo check` so
   that `Cargo.lock` follows. While the crates are at 0.x, a change that
   breaks the public API of `buzzard-rsrc` or `buzzard-tdms` raises the
   minor version (0.1 to 0.2).
2. Merge that through a pull request like any other change.
3. Run **Release** on `main` with `dry_run` on.
4. Run it again with `dry_run` off and approve the `release` environment.
   With `tdms`, the other crates stay at their published version until a
   run with `all`.
5. If a run fails after some crates were uploaded, fix the cause and run it
   again; the uploaded versions are skipped.

A crate added to the workspace later needs "The first release of a crate"
once, and a place in the list of crates in `release.yml`.
