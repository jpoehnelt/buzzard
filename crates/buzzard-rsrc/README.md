# buzzard-rsrc

`buzzard-rsrc` is a read-only parser for the binary RSRC container. LabVIEW
saves `.vi`, `.ctl`, `.vit`, `.ctt`, and `.vim` files in that container, and
an `.llb` archive holds many such files. The crate is for Rust programs that
read these files. It is pure Rust, has no `unsafe` code, and needs no
LabVIEW installation.

The parser reads the resource directory, the version and save settings, the
owning libraries, the links to other files, the type table, and the block
diagram: nodes, terminals, wires, constants, case selectors, loops, and
tunnels. Truncated or corrupted input gives an `RsrcError`, never a panic.
The block diagram decoder records everything that it skips as a `Loss` on
the `BlockDiagram`.

`buzzard-rsrc` is the parser of
[buzzard](https://github.com/jpoehnelt/buzzard), a static analyzer and
language server for these files. The `buzzard-cli` crate installs the tool as
the `buzzard` binary. The
[buzzard README](https://github.com/jpoehnelt/buzzard#readme) lists what the
parser does not read yet.

## Parse a file

To read the settings and links of a file, call `ViMetadata::from_bytes`, or
`ViMetadata::from_path` for a file on disk.

```rust
use buzzard_rsrc::builder::RsrcBuilder;
use buzzard_rsrc::{FileKind, LvPath, ViMetadata};

fn main() -> Result<(), buzzard_rsrc::RsrcError> {
    // `RsrcBuilder` writes a synthetic file.
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

For raw access to the resources, call `RsrcFile::parse`. To decode the block
diagram, pass the `RsrcFile` to `BlockDiagram::from_rsrc`.

## Read a member of an `.llb` archive

A member name is data from the file. The name may be `..` or hold a
separator, so do not join a member name to a path.

To list the members of an archive, call `Llb::parse`. To get the bytes of
one member, call `Llb::read`, and pass those bytes to the parsers above. For
an archive on disk, read the bytes with `buzzard_rsrc::read_file`.

```rust
use buzzard_rsrc::builder::{LlbBuilder, RsrcBuilder};
use buzzard_rsrc::{FileKind, Llb, Storage, ViMetadata};

fn main() -> Result<(), buzzard_rsrc::RsrcError> {
    // `LlbBuilder` writes a synthetic archive.
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

The module documentation of `llb` has the size limits and the layout of an
archive.

## Stability

The crate is at 0.x. A minor release, such as 0.1 to 0.2, may change the
API. A patch release does not. Each public enum that will grow as the
decoder reads more of the format is `#[non_exhaustive]`.

`builder::RsrcBuilder` and `builder::LlbBuilder` write synthetic files for
tests. Both builders are public so that a program built on this crate can
test against them. They cover only what buzzard's own tests need.

## License

The license is Apache-2.0, with one table under MIT. The SPDX expression is
`Apache-2.0 AND MIT`. The `LICENSE` and `NOTICE` files of this package hold
the texts.

Two generated tables hold names from other projects. One table has the
built-in function names from LVKit (Apache-2.0). The other has the heap tag
and class names from pylabview (MIT). `NOTICE` gives the copyright lines of
both projects, what each table took, and the MIT license text.

LabVIEW, NI, and National Instruments are trademarks of National Instruments
Corporation, used only to identify the file formats that buzzard reads.
buzzard is not affiliated with, endorsed by, or sponsored by that company.
