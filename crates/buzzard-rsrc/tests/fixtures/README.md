# Conformance fixtures

The conformance tests in `crates/buzzard-rsrc/tests/conformance.rs` read
these real binary files. Each directory holds unmodified copies from one
project.

| Directory | Source | License |
| --- | --- | --- |
| `pylavi/` | The test suite of [marcpage/pylavi](https://github.com/marcpage/pylavi) | Public domain (Unlicense) |
| `pylabview/` | `examples/` of [mefistotelis/pylabview](https://github.com/mefistotelis/pylabview). The three files are byte-identical to those at commit `5f20e23de6a386021eb955c825cc43d221f09ff1` | MIT. The text is in the repository's `NOTICE` |

This table gives the upstream path and the SHA-256 of each file under
`pylabview/`.

| Fixture | Upstream path | SHA-256 |
| --- | --- | --- |
| `blank_project1_extr_from_exe_lv14f1.llb` | `examples/blank_project1_extr_from_exe_lv14f1.llb` | `4cd99a6653486696f3336a866d0b1242b8f73837a0766f16d549206d633279ce` |
| `empty_libfile.llb` | `examples/lv14f1/empty_libfile.llb` | `b2a0fe9d9246f55157a779cf58d685c0fd350407d8d2438f39f1f7f78cf2fe24` |
| `empty_vifile.vi` | `examples/lv14f1/empty_vifile.vi` | `b6f8934ee58eb3418902b181379064aa77e558cb33f46e45bb179a96b235dd8a` |

No fixture is part of a published package.

Each fixture has a snapshot in `crates/buzzard-rsrc/tests/snapshots`, and
`BUZZARD_BLESS=1 cargo test -p buzzard-rsrc --test conformance` regenerates
the snapshots. Each of the two `.llb` files also has an `.archive.json`
snapshot. That snapshot holds what the archive reader lists, or why the
reader does not open the file.
