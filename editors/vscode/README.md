# Buzzard for VS Code

This extension is a client for the language server of
[`buzzard`](https://github.com/jpoehnelt/buzzard#readme). It shows in VS Code what `buzzard` reports
about LabVIEW `.vi`, `.ctl`, `.lvproj`, `.lvlib`, and `.lvclass` files. LabVIEW does not have to be
installed. The `buzzard` executable, the binary that `cargo install` builds, does all the analysis,
and it only reads the files.

## Install the `buzzard` executable

The extension looks for the `buzzard` executable on `PATH`, or where the `buzzard.path` setting
says. To install the executable from a checkout of the
[buzzard repository](https://github.com/jpoehnelt/buzzard), run this command:

```bash
cargo install --path crates/buzzard-cli
```

Without a checkout, run `cargo install --git https://github.com/jpoehnelt/buzzard buzzard-cli`.
Both commands need Rust 1.91 or later.

When the extension does not find the executable, the status bar shows "Buzzard: not found" and a
notification gives the `cargo install --path` command.

## What the extension adds

The extension adds these views and commands:

- **Problems panel** lists the `BZxxx` problems of every file in the workspace folders. The files
  do not have to be open. The list updates when you save a file.
- **Go to Symbol in Workspace** (`Ctrl+T`) finds VIs, controls, classes, libraries, and projects.
- **Buzzard: Show File Summary** shows the saved version, execution flags, dependencies, and caller
  counts of a file.
- **Buzzard: Show Block Diagram** shows a picture of the block diagram of a `.vi` or `.ctl` file.
- **Buzzard: Find Callers** lists the files that use a file. When you select a caller, Explorer
  shows it.
- **Buzzard: Restart Server** restarts the language server. A click on the status bar item does
  the same.
- **Buzzard Viewer** is a read-only editor for `.vi` and `.ctl` files. It shows the summary and the
  block diagram, and it reloads when another program saves the file.

Show File Summary, Show Block Diagram, and Find Callers are in the Explorer context menu of the
files they apply to. They are also in the Command Palette, where they apply to the active tab or
ask for a file.

## Open a file in the Buzzard Viewer

VS Code cannot show a `.vi` or `.ctl` file as text, so it asks how to open one.

- To open one file in the viewer, right-click the file, choose "Open With...", and choose Buzzard
  Viewer.
- To open a file from the Problems panel, click the problem. Choose Buzzard Viewer when VS Code
  asks how to open the file.
- To make the viewer the default, set `"workbench.editorAssociations"` to
  `{ "*.vi": "buzzard.viewer", "*.ctl": "buzzard.viewer" }`.

## Settings

| Setting | Default | Meaning |
| --- | --- | --- |
| `buzzard.path` | `buzzard` | The executable. The extension looks up a bare name on `PATH`. It resolves `~/...` against the home directory, and a relative path with a directory in it against the first workspace folder. |
| `buzzard.trace.server` | `off` | `messages` or `verbose` logs the messages between the extension and the language server to the output panel. |

## Security and privacy

- The extension has no telemetry and makes no network request.
- The extension never writes to a workspace file. `buzzard lsp` and `buzzard inspect` only read.
- The extension starts the executable directly, without a shell. It looks up a bare name in the
  absolute directories of `PATH` only, never in the current directory.
- In an untrusted workspace, the extension ignores a `buzzard.path` that the workspace sets. Opening
  a folder therefore cannot choose the program that runs.
- The webviews run no script. Their Content-Security-Policy is `default-src 'none'` plus one
  stylesheet (by nonce) and `data:` images. The extension escapes text taken from files. It shows
  the block diagram as an image and never inserts it as markup.

## Limitations

- The summary and the block diagram are as complete as `buzzard inspect` is for the file. See the
  [limitations of buzzard](https://github.com/jpoehnelt/buzzard#limitations).
- Callers, symbols, and problems need an open folder, because the language server analyzes
  workspace folders. The viewer and the two "Show" commands also work on a single file.

## Trademark

LabVIEW, NI, and National Instruments are trademarks of National Instruments Corporation, used only
to identify the file formats that buzzard reads. buzzard is not affiliated with, endorsed by, or
sponsored by that company.

## License

The license is Apache-2.0. [LICENSE](LICENSE) holds the text.
