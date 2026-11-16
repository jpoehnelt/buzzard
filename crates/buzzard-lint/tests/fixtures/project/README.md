# Project file fixtures

Real project-level XML files, copied unmodified from two public
repositories. Each directory holds the licence of the repository its files
come from.

| File | Source | Licence |
| --- | --- | --- |
| `jki-smo/SMO.SelfTerminating.lvclass` | [JKISoftware/JKI-State-Machine-Objects](https://github.com/JKISoftware/JKI-State-Machine-Objects) at `d496f05`, `tests/_test-support-SMOs/SMO.SelfTerminating/` | BSD 3-Clause, `jki-smo/LICENSE` |
| `jki-smo/URI.lvlib` | same repository, `src/URI Parsing/` | BSD 3-Clause, `jki-smo/LICENSE` |
| `grpc-labview/helloworld.lvproj` | [ni/grpc-labview](https://github.com/ni/grpc-labview) at `2f07f03`, `examples/helloworld/` | MIT, `grpc-labview/LICENSE` |

The short `NI.LVClass.ParentClassLinkInfo` strings used as test vectors in
`src/project.rs` are quoted from class files in the same two repositories.

## What these files established

Both repositories were scanned in full (67 project-level files, 2308 items
with a URL, 22 classes with a parent) to settle two points that the parser
depends on:

- **Item URLs are relative to the XML file itself.** A sibling of
  `URI.lvlib` is stored as `../Construct URI.vi`. Resolving against the
  file's directory instead finds none of the files; resolving against the
  file finds all of them except a handful that are genuinely absent from
  the repositories. A URL with no leading `..`, such as the
  `Class Private Data` item, names something stored inside the XML file.
- **`ParentClassLinkInfo` is a flattened byte string in a 64-character
  printable encoding.** Each character is `0x21 + value` for a six-bit
  value, most significant bits first, with no padding character. The
  decoded bytes hold the saving version, a fixed 25-byte type description,
  and a length-prefixed array containing the parent's qualified name and a
  `PTH0` path. All 22 values decode this way; the 15 with a relative path
  resolve to a class file that exists, and the other 7 point into
  `<vilib>`.

The files were saved by editor versions 13.0 and 19.0. Other versions have
not been checked; a value that does not match the layout exactly is
reported as "no parent known" rather than guessed at.
