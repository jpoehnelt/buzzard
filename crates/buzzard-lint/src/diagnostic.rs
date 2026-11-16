//! Findings reported by the rules.

use std::path::PathBuf;

use serde::Serialize;

/// How serious a finding is. Errors and warnings make `buzzard check` fail;
/// info does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// A problem that must be fixed, such as an unreadable file or a
    /// circular dependency.
    Error,
    /// A likely mistake or a practice worth changing.
    Warning,
    /// Worth knowing, but not a problem by itself.
    Info,
}

impl Severity {
    /// The lowercase name, as used in text and JSON output.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

/// One finding about one file. Binary files have no lines, so a diagnostic
/// is anchored to the file as a whole.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Diagnostic {
    /// The file the finding is about.
    pub path: PathBuf,
    /// Code of the rule that reported it, e.g. `BZ001`.
    pub code: &'static str,
    /// Usually the rule's default severity; a rule may lower it for one
    /// finding.
    pub severity: Severity,
    /// Human-readable explanation, without the path or rule code.
    pub message: String,
    /// Other files involved, e.g. the members of a dependency cycle.
    pub related: Vec<PathBuf>,
}
