# buzzard-rsrc

A read-only parser for the binary RSRC resource container that LabVIEW saves
`.vi`, `.ctl`, `.vit`, `.ctt` and `.vim` files in. It is pure Rust, has no
`unsafe` code, and needs neither the editor nor its runtime.

It reads the resource directory, the version and save settings, the owning
libraries, the links to other files, the type table and the block diagram
(nodes, terminals, wires, constants, case selectors, loops and tunnels).
Truncated or corrupted input gives an `RsrcError`, never a panic, and what
the diagram decoder had to skip is recorded as a `Loss` on the diagram.

```rust
use buzzard_rsrc::builder::RsrcBuilder;
use buzzard_rsrc::{FileKind, LvPath, ViMetadata};

fn main() -> Result<(), buzzard_rsrc::RsrcError> {
    // A synthetic file. For a file on disk, use `ViMetadata::from_path`.
    let bytes = RsrcBuilder::default()
        .save_record(0x2300_8000, &[0, 0x400, 0, 0, 0, 0])
        .links(&[("Helper.vi", LvPath::sibling("Helper.vi"))])
        .build();

    let vi = ViMetadata::from_bytes(&bytes)?;
    assert_eq!(vi.kind, FileKind::Vi);
    assert_eq!(vi.links[0].name(), Some("Helper.vi"));
    Ok(())
}
```

`RsrcFile::parse` gives raw access to the resources, and
`BlockDiagram::from_rsrc` decodes the block diagram.

This crate is the parser of [buzzard](https://github.com/jpoehnelt/buzzard),
a static analyzer and language server for these files; install the tool with
`cargo install buzzard-cli`. The repository's README lists what the parser
does not read yet.

## Stability

The crate is at 0.x. A minor release (0.1 to 0.2) may change the API; a patch
release does not. The enumerations that will grow as more of the format is
decoded are `#[non_exhaustive]`. `builder::RsrcBuilder` writes synthetic
files for tests; it is public so that programs built on this crate can test
against it, and it covers only what buzzard's own tests need.

## Licence

Apache-2.0. See `LICENSE` and `NOTICE` in this package.

LabVIEW and National Instruments are trademarks of National Instruments
Corporation. buzzard is an independent project and is not affiliated with,
endorsed by, or sponsored by it; the names are used only to identify the
file formats that buzzard reads.
