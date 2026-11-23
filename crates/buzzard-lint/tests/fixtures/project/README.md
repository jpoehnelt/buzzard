# Project file fixtures

These are real project files, copied unmodified from two public
repositories. Each directory holds the license of the repository that its
files come from.

| File | Source | License |
| --- | --- | --- |
| `jki-smo/SMO.SelfTerminating.lvclass` | [JKISoftware/JKI-State-Machine-Objects](https://github.com/JKISoftware/JKI-State-Machine-Objects) at `d496f05`, `tests/_test-support-SMOs/SMO.SelfTerminating/` | BSD 3-Clause, `jki-smo/LICENSE` |
| `jki-smo/URI.lvlib` | same repository, `src/URI Parsing/` | BSD 3-Clause, `jki-smo/LICENSE` |
| `grpc-labview/helloworld.lvproj` | [ni/grpc-labview](https://github.com/ni/grpc-labview) at `2f07f03`, `examples/helloworld/` | MIT, `grpc-labview/LICENSE` |

The tests in `crates/buzzard-lint/src/project/tests.rs` use short
`NI.LVClass.ParentClassLinkInfo` strings as test vectors. The strings are
quoted from class files in the same two repositories.
