//! Static analysis rules and the workspace dependency graph.
//!
//! The crate keeps files in path order and sorts every result list, so two
//! runs over the same tree give identical output whatever the thread
//! scheduling.

pub mod archive;
pub mod dataflow;
pub mod deps;
pub mod diagnostic;
pub mod diff;
pub mod graph;
pub mod inspector;
pub mod project;
pub mod render;
pub mod rules;
pub mod sarif;
pub mod summary;
pub mod verify;
pub mod workspace;

pub use diagnostic::{Diagnostic, Severity};
pub use graph::{DependencyGraph, EdgeKind};
pub use inspector::{Inspection, RsrcInspector, ViInspector};
pub use project::{Member, ParentClass, ProjectFile, ProjectKind};
pub use rules::{lint, LintConfig, Rule, RULES};
pub use workspace::{Analysis, FileState, Workspace};

/// Compiles the examples of the crate's `README.md` as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
