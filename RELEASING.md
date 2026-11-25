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

## Binaries

The Binaries workflow, `.github/workflows/binaries.yml`, builds the
`buzzard` binary for the targets below. It uploads the archives as workflow
artifacts only. It creates no tag and no GitHub Release, and it uploads
nothing to crates.io. The Release workflow runs it after its checks, and
you can also start it by hand. Neither the dry run nor the upload of the
crates waits for it.

| Target | Runner |
| --- | --- |
| `x86_64-unknown-linux-musl` | `ubuntu-latest` |
| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` |
| `x86_64-apple-darwin` | `macos-15-intel` |
| `aarch64-apple-darwin` | `macos-latest` |
| `x86_64-pc-windows-msvc` | `windows-latest` |
| `aarch64-pc-windows-msvc` | `windows-11-arm` |

Each runner builds for its own architecture and runs the binary on the
fixtures before it packages it. The Linux binaries link musl statically,
and the Windows binaries link the C runtime statically. The arm runners are
free for public repositories only. In a private repository, those jobs wait
for a runner that never comes.

Each archive is `buzzard-<version>-<target>.tar.gz`, or `.zip` on Windows.
It holds one directory of the same name, with the binary, `README.md`,
`LICENSE`, and `NOTICE`. Each archive has a `<archive>.sha256` file beside
it, and `SHA256SUMS` lists every archive. The artifact `buzzard-binaries`
holds them all.

To attach the binaries to a release:

1. Run the Binaries workflow on `main`, or take the run of the Release
   workflow that published the crates.
2. Download the artifact `buzzard-binaries` from the run's page. Downloads
   need a signed-in GitHub user, and the artifacts expire after the
   retention period of the repository.
3. Unpack the artifact and run `sha256sum -c SHA256SUMS` in its directory.
4. Only you create the tag and the GitHub Release, by hand. Attach the
   archives, their `.sha256` files, and `SHA256SUMS` to the release.
