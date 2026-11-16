# Buzzard for VS Code

A thin client for the [`buzzard`](../../README.md) language server. It shows
what `buzzard` finds in LabVIEW™ `.vi`, `.ctl`, `.lvproj`, `.lvlib` and
`.lvclass` files inside VS Code. The LabVIEW IDE is not needed.

The extension contains no analysis of its own. Everything comes from the
`buzzard` executable, which only ever reads the files.

## Requirements

The `buzzard` executable, on `PATH` or named by the `buzzard.path` setting.
From a checkout of the repository:

```bash
cargo install --path crates/buzzard-cli
```

If the executable cannot be found, the status bar shows
"Buzzard: not found" and a notification gives the install command.

## Features

- **Problems panel.** The `BZxxx` findings of every file in the workspace
  folders, updated when a file is saved by the IDE. The files do not have to
  be open (they are binary, and have no text editor anyway).
- **Go to Symbol in Workspace** (`Ctrl+T`) finds VIs, controls, classes,
  libraries and projects by name.
- **Buzzard: Show File Summary.** Saved version, execution flags,
  dependencies and caller counts of a file.
- **Buzzard: Show Block Diagram.** A picture of the block diagram of a
  `.vi` or `.ctl`.
- **Buzzard: Find Callers.** Lists the files that use a file; selecting one
  reveals it in the Explorer.
- **Buzzard: Restart Server.** Also what a click on the status bar item does.
- **Buzzard Viewer.** A read-only editor for `.vi` and `.ctl` with the
  summary and the diagram. It is offered through "Open With..." and is never
  the default on its own. To make it the default:

  ```json
  "workbench.editorAssociations": {
    "*.vi": "buzzard.viewer",
    "*.ctl": "buzzard.viewer"
  }
  ```

The three file commands are in the Explorer context menu of the matching
files, and in the Command Palette, where they apply to the active tab or ask
for a file.

## Settings

| Setting | Default | Meaning |
| --- | --- | --- |
| `buzzard.path` | `buzzard` | The executable. A bare name is looked up on `PATH`; `~/...` is taken from the home directory; a relative path with a directory in it is taken from the first workspace folder. |
| `buzzard.trace.server` | `off` | `messages` or `verbose` logs the protocol messages to the output panel. |

## Security and privacy

- No telemetry. The extension makes no network request.
- The extension never writes to a workspace file. It runs `buzzard lsp` and
  `buzzard inspect`, which read.
- The executable is started directly, without a shell. A bare name is only
  looked up in the absolute directories of `PATH`, never in the current
  directory.
- In an untrusted workspace, a `buzzard.path` set by the workspace is
  ignored, so opening a folder cannot choose the program that is run.
- The webviews run no script. Their Content-Security-Policy is
  `default-src 'none'` plus one stylesheet (by nonce) and `data:` images.
  Text taken from files is escaped; the diagram is shown as an image, not
  inserted as markup.

## Limitations

- The summary and diagram are as complete as `buzzard inspect` is for the
  file; see the limitations in the main README.
- Clicking a problem in the Problems panel opens the file, which VS Code
  cannot show as text. Choose the Buzzard Viewer when asked how to open it.
- Callers, symbols and diagnostics need an open folder: the server analyzes
  workspace folders. The viewer and the two "Show" commands also work on a
  single file.

## Development

```bash
npm ci
npm run compile     # type-check, then bundle to dist/extension.js
npm run lint
npm test            # unit tests, plain Node, no display needed
npm run package     # builds buzzard-0.1.0.vsix; nothing is published
```

`npm test` also runs the real tool on a real file when both variables are
set:

```bash
BUZZARD_BIN=../../target/debug/buzzard \
BUZZARD_FIXTURE=../../crates/buzzard-rsrc/tests/fixtures/pylabview/empty_vifile.vi \
npm test
```

`npm run test:integration` downloads VS Code and runs the extension in it
against a temporary workspace. It needs a display; on Linux:

```bash
BUZZARD_BIN="$PWD/../../target/debug/buzzard" xvfb-run -a npm run test:integration
```

Press `F5` in VS Code with this directory open to try the extension in an
Extension Development Host (after `npm run compile`).

## Trademark disclaimer

LabVIEW and National Instruments are trademarks of National Instruments
Corporation (NI), now part of Emerson. `buzzard` is an independent
open-source project. It is not affiliated with, endorsed by, or sponsored by
NI or Emerson. Those names are used here only to describe the file formats
this tool reads.

## License

Apache-2.0. See [LICENSE](LICENSE), a copy of the [licence of the repository](../../LICENSE).
