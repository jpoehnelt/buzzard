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
and pylabview sections (the latter with the MIT licence text), for the two
generated tables in its `src/blocks/heap`.
`crates/buzzard-cli/tests/packaging.rs` fails when a copy differs from the
root. After a change to the root `LICENSE`, or to the trademark, LVKit or
pylabview part of the root `NOTICE`, copy it into the crates.

The published manifests list a few dev-dependencies that only the tests
left in the repository use (`proptest` and `serde_json` in `buzzard-tdms`,
`serde_json` in `buzzard-rsrc`, `url` in `buzzard-cli`). Cargo ignores the
dev-dependencies of a dependency, so users do not build them; they stay
because the tests in the repository need them.

Treat a published version as permanent. Its contents can never be changed.
It can be yanked, which hides it from new resolutions without removing it.
crates.io lets an owner delete a crate only under narrow conditions
([RFC 3660](https://rust-lang.github.io/rfcs/3660-crates-io-crate-deletions.html):
for example within 72 hours of its publication); do not plan on them.

## The workflow

`.github/workflows/release.yml` ("Release" under Actions) runs only when
someone starts it by hand (`workflow_dispatch`). It has two inputs:

- `crates`: `tdms` for `buzzard-tdms` alone, or `all` for the six in
  dependency order.
- `dry_run`: on by default.

It first runs every job of `ci.yml` on the chosen commit. Then:

- With `dry_run` on, it runs `cargo publish --dry-run`: each crate is
  packaged and built from its packaged files, and nothing is uploaded. The
  job's summary lists each package with its size. This job has no access to
  crates.io.
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

1. Once, before anything is published: in the repository's Settings,
   Environments, create the environment `release`. Limit its deployment
   branches to `main` and add yourself as a required reviewer, so that
   every upload waits for an approval. This comes first because crates.io
   checks only the repository, the workflow's file name and the
   environment's name: a trusted publisher that names an environment
   without these rules would let anyone with write access publish. Check
   that it exists with both rules before step 7, every time.
2. On GitHub, run **Release** on `main` with `crates` set to `tdms` (or
   `all`) and `dry_run` on. Go on only when it is green.
3. Check out that same commit of `main` with a clean working tree.
4. On crates.io, under Account Settings, API Tokens, create a token with
   the scope `publish-new` only and the shortest expiry offered. Run
   `cargo login` and paste it. Do not store it in the repository or in its
   secrets.
5. Publish. For `buzzard-tdms` alone:

   ```bash
   cargo publish --locked -p buzzard-tdms
   ```

   For the other five, in this order, because each needs the ones before
   it on crates.io:

   ```bash
   cargo publish --locked -p buzzard-rsrc
   cargo publish --locked -p buzzard-lint
   cargo publish --locked -p buzzard-ls
   cargo publish --locked -p buzzard-mcp
   cargo publish --locked -p buzzard-cli
   ```

   Each command waits, for up to a minute, until the crates.io index lists
   the new version. If the index takes longer, the next command fails with
   `failed to select a version for the requirement buzzard-...` or
   `no matching package named buzzard-...`. Nothing is wrong: wait a
   minute and run that command again.
6. Run `cargo logout` and revoke the token on crates.io.
7. For each crate just published, on crates.io open the crate's Settings,
   Trusted Publishing, Add, GitHub, and enter:

   | Field | Value |
   | --- | --- |
   | Repository owner | `jpoehnelt` |
   | Repository name | `buzzard` |
   | Workflow filename | `release.yml` |
   | Environment | `release` |

8. In `README.md`, change the "Install" section to say that
   `cargo install buzzard-cli` works (after `buzzard-cli` is published), and
   remove the "No crate is published yet" paragraph above. If
   `buzzard-tdms` went out alone, its README on crates.io does not name
   `cargo install buzzard-cli` for that reason; the wording can be changed
   in a later version once `buzzard-cli` exists.

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
   again; the uploaded versions are skipped. A failure with
   `failed to select a version for the requirement buzzard-...` means the
   crates.io index did not list the crate uploaded just before within the
   minute cargo waits: wait a minute and run it again, with nothing to fix.

A crate added to the workspace later needs "The first release of a crate"
once, and a place in the list of crates in `release.yml`.
