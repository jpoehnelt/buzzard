# Buzzard for VS Code

This extension is a client for the [`buzzard`](https://github.com/jpoehnelt/buzzard#readme) language
server. It shows what `buzzard` finds in LabVIEW `.vi`, `.ctl`, `.lvproj`, `.lvlib`, and `.lvclass`
files inside VS Code. LabVIEW does not have to be installed. All analysis comes from the `buzzard` executable, which
only reads the files.

## Requirements

The extension needs the `buzzard` executable, on `PATH` or named by the `buzzard.path` setting. To
install it from a checkout of the repository, run:

```bash
cargo install --path crates/buzzard-cli
```

Without the executable, the status bar shows "Buzzard: not found" and a notification gives that
command.

## Features

- **Problems panel.** It lists the `BZxxx` findings of every file in the workspace folders and
  updates when a file is saved. The files do not have to be open.
- **Go to Symbol in Workspace** (`Ctrl+T`) finds VIs, controls, classes, libraries, and projects.
- **Buzzard: Show File Summary** shows the saved version, execution flags, dependencies, and caller
  counts.
- **Buzzard: Show Block Diagram** shows a picture of the block diagram of a `.vi` or `.ctl` file.
- **Buzzard: Find Callers** lists the files that use a file. Select one to reveal it in Explorer.
- **Buzzard: Restart Server** restarts the language server. A click on the status bar item does
  the same.
- **Buzzard Viewer** is a read-only editor for `.vi` and `.ctl` files with the summary and the
  diagram. "Open With..." offers it. To make it the default, set `"workbench.editorAssociations"` to
  `{ "*.vi": "buzzard.viewer", "*.ctl": "buzzard.viewer" }`.

The three file commands are in the Explorer context menu of the matching files and in the Command
Palette, where they apply to the active tab or ask for a file.

## Settings

| Setting | Default | Meaning |
| --- | --- | --- |
| `buzzard.path` | `buzzard` | The executable. A bare name is looked up on `PATH`. `~/...` is taken from the home directory. A relative path with a directory in it is taken from the first workspace folder. |
| `buzzard.trace.server` | `off` | `messages` or `verbose` logs the protocol messages to the output panel. |

## Security and privacy

- The extension has no telemetry and makes no network request.
- The extension never writes to a workspace file. `buzzard lsp` and `buzzard inspect` only read.
- The executable is started directly, without a shell. A bare name is only looked up in the absolute
  directories of `PATH`, never in the current directory.
- In an untrusted workspace, a `buzzard.path` set by the workspace is ignored, so opening a folder
  cannot choose the program that is run.
- The webviews run no script. Their Content-Security-Policy is `default-src 'none'` plus one
  stylesheet (by nonce) and `data:` images. Text taken from files is escaped. The diagram is shown
  as an image and is never inserted as markup.

## Limitations

- The summary and diagram are as complete as `buzzard inspect` is for the file. See the limitations
  in the main README.
- Clicking a problem in the Problems panel opens the file, which VS Code cannot show as text. Choose
  the Buzzard Viewer when asked how to open it.
- Callers, symbols, and diagnostics need an open folder, because the server analyzes workspace
  folders. The viewer and the two "Show" commands also work on a single file.

## Development

```bash
npm ci
npm run compile     # type-check, then bundle to dist/extension.js
npm run lint
npm test            # unit tests in plain Node
npm run package     # builds the .vsix and publishes nothing
BUZZARD_BIN=<absolute path> xvfb-run -a npm run test:integration   # tests inside VS Code
```

`npm test` also runs the real tool when `BUZZARD_BIN` names a built `buzzard` and `BUZZARD_FIXTURE`
names a `.vi` file. The tests inside VS Code need a display, which `xvfb-run` gives on Linux.

## Trademark

LabVIEW, NI, and National Instruments are trademarks of National Instruments Corporation, used only
to identify the file formats that buzzard reads. buzzard is not affiliated with, endorsed by, or
sponsored by that company.

## License

Apache-2.0. See [LICENSE](LICENSE).
