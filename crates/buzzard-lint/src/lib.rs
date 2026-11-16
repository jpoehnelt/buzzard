//! Static analysis rules and the workspace dependency graph.
//!
//! Everything here is deterministic: files are kept in path order and every
//! result list is sorted, so two runs over the same tree give identical
//! output regardless of thread scheduling.

pub mod diagnostic;
pub mod graph;
pub mod inspector;
pub mod rules;
pub mod sarif;
pub mod summary;
pub mod workspace;

pub use diagnostic::{Diagnostic, Severity};
pub use graph::DependencyGraph;
pub use inspector::{RsrcInspector, ViInspector};
pub use rules::{lint, LintConfig, Rule, RULES};
pub use workspace::{FileState, Workspace};
