//! Static analysis rules and the workspace dependency graph.
//!
//! Everything here is deterministic: files are kept in path order and every
//! result list is sorted, so two runs over the same tree give identical
//! output regardless of thread scheduling.

pub mod dataflow;
pub mod diagnostic;
pub mod diff;
pub mod graph;
pub mod inspector;
pub mod project;
pub mod render;
pub mod rules;
pub mod sarif;
pub mod summary;
pub mod workspace;

pub use diagnostic::{Diagnostic, Severity};
pub use graph::{DependencyGraph, EdgeKind};
pub use inspector::{Inspection, RsrcInspector, ViInspector};
pub use project::{Member, ParentClass, ProjectFile, ProjectKind};
pub use rules::{lint, LintConfig, Rule, RULES};
pub use workspace::{Analysis, FileState, Workspace};
