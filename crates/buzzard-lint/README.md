# buzzard-lint

`buzzard-lint` is the analysis library of
[buzzard](https://github.com/jpoehnelt/buzzard), a static analyzer and
language server for LabVIEW files. The crate holds the lint rules, the
dataflow checks, the project file parser, the dependency graph, SARIF
output, block diagram rendering, the semantic diff, and the state machine
checks. The crate is for the `buzzard` binary. To run the analysis, install
`buzzard-cli`.

## Stability

This crate is an internal part of the buzzard tool, published so that
`cargo install buzzard-cli` works. Its API may change in any 0.x release,
patch releases included. The supported interface is the JSON and SARIF
output of the `buzzard` binary.

## Example

This example scans the current directory and prints each problem.

```rust,no_run
use std::path::PathBuf;

use buzzard_lint::{lint, DependencyGraph, LintConfig, Workspace};

let workspace = Workspace::scan(&[PathBuf::from(".")]);
let graph = DependencyGraph::build(&workspace);
for problem in lint(&workspace, &graph, &LintConfig::default()) {
    println!("{} {} {}", problem.path.display(), problem.code, problem.message);
}
```

## License

The license is Apache-2.0. The `LICENSE` and `NOTICE` files of this package
hold the texts.

LabVIEW, NI, and National Instruments are trademarks of National Instruments
Corporation, used only to identify the file formats that buzzard reads.
buzzard is not affiliated with, endorsed by, or sponsored by that company.
