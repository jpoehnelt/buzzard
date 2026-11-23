# buzzard-lint

The analysis library of [buzzard](https://github.com/jpoehnelt/buzzard), a
static analyzer and language server for LabVIEW projects. It holds the lint
rules, the dataflow checks, the project XML parser, the dependency graph,
SARIF output, diagram rendering, the semantic diff, and the state machine
verification.

This crate is an internal part of the buzzard tool. Its API may change in
any 0.x release, including patch releases. It is published so that
`cargo install buzzard-cli` works. To use the analysis, run the `buzzard`
binary and read its JSON or SARIF output. That output is the supported
interface.

```rust,no_run
use std::path::PathBuf;

use buzzard_lint::{lint, DependencyGraph, LintConfig, Workspace};

let workspace = Workspace::scan(&[PathBuf::from(".")]);
let graph = DependencyGraph::build(&workspace);
for finding in lint(&workspace, &graph, &LintConfig::default()) {
    println!("{} {} {}", finding.path.display(), finding.code, finding.message);
}
```

## License

Apache-2.0. See `LICENSE` and `NOTICE` in this package.

LabVIEW, NI, and National Instruments are trademarks of National Instruments
Corporation, used only to identify the file formats that buzzard reads.
buzzard is not affiliated with, endorsed by, or sponsored by that company.
