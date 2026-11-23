# buzzard-rsrc

A read-only parser for the binary RSRC resource container that LabVIEW saves
`.vi`, `.ctl`, `.vit`, `.ctt` and `.vim` files in, and for the `.llb`
archives that hold such files. It is pure Rust, has no
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

A member name is data from the file: it may be `..` or hold a separator, so
do not join it to a path. A member that declares more than
`llb::MAX_MEMBER_BYTES`, or more than its compressed bytes could hold, is
refused before anything is inflated, and so is an archive in which two
members share bytes of the file. Members are read whether they are stored as
they are, as a zlib stream, or in the older compression that leaves out zero
bytes. An archive taken out of a built application has no directory
(`llb::is_program_tree`) and is not read; the module documentation of `llb`
has the layout.

This crate is the parser of [buzzard](https://github.com/jpoehnelt/buzzard),
a static analyzer and language server for these files; the tool is the
`buzzard` binary of the `buzzard-cli` crate, and the repository's README says
how to install it. That README also lists what the parser does not read yet.

## Stability

The crate is at 0.x. A minor release (0.1 to 0.2) may change the API; a patch
release does not. The enumerations that will grow as more of the format is
decoded are `#[non_exhaustive]`. `builder::RsrcBuilder` writes synthetic
files for tests; it is public so that programs built on this crate can test
against it, and it covers only what buzzard's own tests need.

## Licence

Apache-2.0, with one table under MIT (`Apache-2.0 AND MIT`). See `LICENSE` and `NOTICE` in this package. Two generated tables
hold names taken from other projects: built-in function names from LVKit
(Apache-2.0) and heap tag and class names from pylabview (MIT). `NOTICE` has
their copyright lines, what was taken, and the MIT licence text.

LabVIEW and National Instruments are trademarks of National Instruments
Corporation. buzzard is an independent project and is not affiliated with,
endorsed by, or sponsored by it; the names are used only to identify the
file formats that buzzard reads.
