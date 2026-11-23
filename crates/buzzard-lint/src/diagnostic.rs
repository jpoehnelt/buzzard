//! Findings reported by the rules.

use std::path::PathBuf;

use serde::Serialize;

use crate::workspace::Workspace;

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

impl Diagnostic {
    /// The finding as a program that only knows files on disk can show
    /// it: an editor, or a code-scanning service.
    ///
    /// A finding about a member of an `.llb` archive is moved to the
    /// archive, and its message starts with the member's name; the name
    /// is also returned. Related files inside archives become their
    /// archives. Any other finding is returned as it is.
    pub fn on_disk(&self, workspace: &Workspace) -> (Self, Option<String>) {
        let mut related: Vec<PathBuf> = self
            .related
            .iter()
            .map(|path| workspace.file_on_disk(path))
            .collect();
        let Some((archive, member)) = workspace.archive_of(&self.path) else {
            related.dedup();
            return (
                Self {
                    related,
                    ..self.clone()
                },
                None,
            );
        };
        related.retain(|path| *path != archive);
        related.sort();
        related.dedup();
        let shown = crate::archive::printable(&member);
        (
            Self {
                path: archive,
                code: self.code,
                severity: self.severity,
                message: format!("{shown}: {}", self.message),
                related,
            },
            Some(member),
        )
    }
}
