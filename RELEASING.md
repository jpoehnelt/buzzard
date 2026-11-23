# Releasing

This file describes how the owner publishes the six crates to crates.io.
Nothing here runs by itself. No crate is published yet.

Publish in this order: `buzzard-rsrc`, `buzzard-tdms`, `buzzard-lint`,
`buzzard-ls`, `buzzard-mcp`, `buzzard-cli`. `buzzard-tdms` depends on none of
the others and can be released alone. All six share the `version` in
`[workspace.package]` in the root `Cargo.toml`. A published version is
permanent. You can yank it, but you cannot change it.

## The workflow

`.github/workflows/release.yml` ("Release" under Actions) runs only when
someone starts it by hand. It has two inputs:

- `crates`: `tdms` for `buzzard-tdms` alone, or `all` for the six in order.
- `dry_run`: on by default.

The workflow first runs every job of `ci.yml` on the chosen commit. With
`dry_run` on, it runs `cargo publish --dry-run` and uploads nothing. With
`dry_run` off, it publishes from `main` only, in the `release` environment,
with a 30-minute token from
[Trusted Publishing](https://crates.io/docs/trusted-publishing). It skips a
version that crates.io already has, so you can start a failed run again.
Trusted Publishing needs a crate that already exists on crates.io, so the
owner does the first release of each crate by hand.

## The first release of a crate

1. In the repository's Settings, Environments, create the environment
   `release`. Limit its deployment branches to `main` and add yourself as a
   required reviewer. crates.io checks only the names of the repository, the
   workflow file, and the environment, so without both rules anyone with
   write access can publish. Check both rules before step 7, every time.
2. Run **Release** on `main` with `dry_run` on. Go on only when it is green.
3. Check out that commit of `main` with a clean working tree.
4. On crates.io, create an API token with the scope `publish-new` only and
   the shortest expiry offered. Run `cargo login` and paste it. Do not store
   it in the repository or in its secrets.
5. Publish, in this order (`buzzard-tdms` can go alone):

   ```bash
   cargo publish --locked -p buzzard-rsrc
   cargo publish --locked -p buzzard-tdms
   cargo publish --locked -p buzzard-lint
   cargo publish --locked -p buzzard-ls
   cargo publish --locked -p buzzard-mcp
   cargo publish --locked -p buzzard-cli
   ```

   If a command fails with `failed to select a version for the requirement
   buzzard-...` or `no matching package named buzzard-...`, the index does
   not list the previous crate yet. Wait a minute and run it again.
6. Run `cargo logout` and revoke the token on crates.io.
7. For each crate just published, open its Settings on crates.io, then
   Trusted Publishing, Add, GitHub, and enter:

   | Field | Value |
   | --- | --- |
   | Repository owner | `jpoehnelt` |
   | Repository name | `buzzard` |
   | Workflow filename | `release.yml` |
   | Environment | `release` |

8. After `buzzard-cli` is published, make `cargo install buzzard-cli` the
   install command in three places: `README.md` "Install", "Requirements"
   in `editors/vscode/README.md`, and `INSTALL_COMMAND` in
   `editors/vscode/src/binary.ts`. In `README.md`, also remove the sentence
   "The crates are not on crates.io yet". Remove "No crate is published yet"
   above.

## Later releases

1. Raise `version` in `[workspace.package]` in the root `Cargo.toml` and the
   five `version = "..."` entries of the `buzzard-*` crates in
   `[workspace.dependencies]`. Run `cargo check` so that `Cargo.lock`
   follows. While the crates are at 0.x, a change that breaks the public API
   of `buzzard-rsrc` or `buzzard-tdms` raises the minor version.
2. Merge that through a pull request.
3. Run **Release** on `main` with `dry_run` on, then with `dry_run` off, and
   approve the `release` environment. With `tdms`, the other crates stay at
   their published version.
4. If a run fails after some crates were uploaded, fix the cause and run it
   again. The index error of step 5 above needs no fix. Run the workflow a second
   time.

A crate added to the workspace later needs "The first release of a crate"
once, and a place in the list of crates in `release.yml`.
