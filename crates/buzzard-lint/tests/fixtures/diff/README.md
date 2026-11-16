# Revision pair fixtures

Two consecutive git revisions of one file each, copied unmodified from two
public repositories, for the semantic diff tests. Each directory holds the
licence of the repository its files come from.

| Files | Source | Licence |
| --- | --- | --- |
| `jki-state-machine/vi-tree.old.vi`, `vi-tree.new.vi` | [JKISoftware/JKI-State-Machine](https://github.com/JKISoftware/JKI-State-Machine), `source/VI Tree - JKI State Machine.vi` at `23e4f34` and at its child `64ff84c` | BSD 3-Clause, `jki-state-machine/LICENSE` |
| `grpc-labview/register-reflection-descriptors.old.vi`, `.new.vi` | [ni/grpc-labview](https://github.com/ni/grpc-labview), `labview source/gRPC lv Support/Server API/Server/Register Reflection Descriptors.vi` at `60727eb` and at its child `a3c567d` | MIT, `grpc-labview/LICENSE` |

## What the revision history established

The matcher in `src/diff.rs` is designed from a measurement over 1,340 such
pairs (every modification of a `.vi` file in the last 80 commits of the
first repository and the last 150 of the second): heap object ids survive
a save for about 96% of nodes, are occasionally reused for an object of
another class, and are occasionally renumbered. The figures are in the
module documentation.
