# Release the crates to crates.io

This guide is for the owner, who starts every release by hand. No crate is
published yet.

A published version is permanent. You can yank a version, but you cannot
change it. Before every upload, run the Release workflow with `dry_run` on.

All six crates share the `version` in `[workspace.package]` in the root
`Cargo.toml`. `buzzard-tdms` depends on none of the others, so you can
release it alone.

The Release workflow is `.github/workflows/release.yml`, and its comments say
what each job does. It takes two inputs:

- `crates` is `tdms` for `buzzard-tdms` alone, or `all` for the six in order.
- `dry_run` is on by default, and the workflow then uploads nothing.

The workflow uploads with
[Trusted Publishing on crates.io](https://crates.io/docs/trusted-publishing),
which works only for a crate that crates.io already has. For a new crate,
follow "The first release of a crate". Otherwise, follow "Later releases".

## The first release of a crate

1. Protect the `release` environment. crates.io checks only the names of the
   repository, the workflow file, and the environment, so without both rules
   below anyone with write access can publish. In the repository's Settings,
   under Environments, create the environment `release`. Limit its
   deployment branches to `main`. Add yourself as a required reviewer.
2. Run the Release workflow on `main` with `dry_run` on. Continue only when
   the run is green.
3. Check out that commit of `main` with a clean working tree.
4. Do not store the token of this step in the repository or in its secrets.
   On crates.io, create an API token with only the scope `publish-new` and
   the shortest expiry offered. Run `cargo login` and paste the token.
5. Publish in this order. `buzzard-tdms` can go alone.

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
   not list the previous crate yet. Wait a minute and run the command again.
6. Run `cargo logout`. Revoke the token on crates.io.
7. Check that the `release` environment still has both rules of step 1. Do
   this check every time, before you add a trusted publisher. Then, for each
   crate just published, open its Settings on crates.io. Choose Trusted
   Publishing, Add, and GitHub, and enter these values:

   | Field | Value |
   | --- | --- |
   | Repository owner | `jpoehnelt` |
   | Repository name | `buzzard` |
   | Workflow filename | `release.yml` |
   | Environment | `release` |

8. After you publish `buzzard-cli`, make `cargo install buzzard-cli` the
   install command in three places: "Install" in `README.md`, "Install the
   `buzzard` executable" in `editors/vscode/README.md`, and `INSTALL_COMMAND`
   in `editors/vscode/src/binary.ts`. In `README.md`, also remove the
   sentence "The crates are not on crates.io yet". In this file, remove "No
   crate is published yet".

## Later releases

1. Raise `version` in `[workspace.package]` in the root `Cargo.toml`. If the
   release breaks the public API of `buzzard-rsrc` or `buzzard-tdms` while
   the crates are at 0.x, raise the minor version. Give the `version` of
   each `buzzard-*` entry in `[workspace.dependencies]` the same value. Run
   `cargo check` so that `Cargo.lock` follows.
2. Merge the change through a pull request.
3. Run the Release workflow on `main` with `dry_run` on. Then run it with
   `dry_run` off and approve the `release` environment. When `crates` is
   `tdms`, the other crates stay at their published version.
4. If a run fails after it uploaded some crates, fix the cause and run the
   workflow again. The workflow skips each version that crates.io already
   has. If the run failed with the index error that step 5 of "The first
   release of a crate" describes, change nothing and run the workflow again.

## Add a crate to the workspace later

Add the new crate to the list of crates in the `publish` job of
`release.yml`. Follow "The first release of a crate" once for that crate.
