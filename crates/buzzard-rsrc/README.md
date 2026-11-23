# buzzard-rsrc

A read-only parser for the binary RSRC resource container that LabVIEW saves
`.vi`, `.ctl`, `.vit`, `.ctt` and `.vim` files in, and for the `.llb`
archives that hold such files. It is pure Rust, has no `unsafe` code, and
needs no LabVIEW installation.

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

An `.llb` archive is one such container that holds many files. `Llb::parse`
lists its members and `Llb::read` gives the bytes of one, which the parsers
above take:

```rust
use buzzard_rsrc::builder::{LlbBuilder, RsrcBuilder};
use buzzard_rsrc::{FileKind, Llb, Storage, ViMetadata};

fn main() -> Result<(), buzzard_rsrc::RsrcError> {
    // A synthetic archive. For one on disk, read it with `read_file`.
    let vi = RsrcBuilder::default()
        .save_record(0x2300_8000, &[0, 0x400, 0, 0, 0, 0])
        .build();
    let bytes = LlbBuilder::default().compressed("Helper.vi", &vi).build();

    let llb = Llb::parse(&bytes)?;
    let member = &llb.members()[0];
    assert_eq!((member.name.as_str(), member.kind), ("Helper.vi", FileKind::Vi));
    assert_eq!(member.storage, Storage::Zlib);
    let helper = ViMetadata::from_bytes(&llb.read(member)?)?;
    assert_eq!(helper.version.map(|v| v.major), Some(23));
    Ok(())
}
```

A member name is data from the file. It may be `..` or hold a separator, so
do not join it to a path. The module documentation of `llb` has the size
limits and the layout.

This crate is the parser of [buzzard](https://github.com/jpoehnelt/buzzard),
a static analyzer and language server for these files. The tool is the
`buzzard` binary of the `buzzard-cli` crate. The repository's README says
how to install it and lists what the parser does not read yet.

## Stability

The crate is at 0.x. A minor release (0.1 to 0.2) may change the API. A
patch release does not. The enumerations that will grow as more of the
format is decoded are `#[non_exhaustive]`. `builder::RsrcBuilder` and
`builder::LlbBuilder` write synthetic files for tests. They are public so
that programs built on this crate can test against them, and they cover only
what buzzard's own tests need.

## License

Apache-2.0, with one table under MIT. The SPDX expression is
`Apache-2.0 AND MIT`. See `LICENSE` and `NOTICE` in this package.

Two generated tables hold names taken from other projects: built-in function
names from LVKit (Apache-2.0) and heap tag and class names from pylabview
(MIT). `NOTICE` has their copyright lines, what was taken, and the MIT
license text.

LabVIEW, NI, and National Instruments are trademarks of National Instruments
Corporation, used only to identify the file formats that buzzard reads.
buzzard is not affiliated with, endorsed by, or sponsored by that company.
