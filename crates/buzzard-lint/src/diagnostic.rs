use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
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
    pub path: PathBuf,
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    /// Other files involved, e.g. the members of a dependency cycle.
    pub related: Vec<PathBuf>,
}
