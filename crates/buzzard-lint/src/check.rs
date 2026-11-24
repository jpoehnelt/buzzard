//! The report of `buzzard check`: the findings, their counts, and a limit
//! on how many findings are shown.
//!
//! A limit shortens the list and nothing else. The counts are always those
//! of every finding, and so is the answer to whether the check failed
//! ([`Shown::failed`]). Every format states how many findings were left
//! out.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Serialize;
use serde_json::{json, Value};

use crate::diagnostic::{Diagnostic, Severity};
use crate::printable;
use crate::workspace::Workspace;

/// How many findings a check gave, of every severity and of every rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Counts {
    /// Number of files that were checked.
    pub files: usize,
    /// Number of findings of any severity.
    pub problems: usize,
    /// Findings of severity error.
    pub errors: usize,
    /// Findings of severity warning.
    pub warnings: usize,
    /// Findings of severity info.
    pub infos: usize,
    /// Findings per rule code, for the codes that have any.
    pub by_code: BTreeMap<&'static str, usize>,
}

impl Counts {
    /// Count `diagnostics`, the findings about `files` files.
    pub fn of(diagnostics: &[Diagnostic], files: usize) -> Self {
        let count = |severity| {
            diagnostics
                .iter()
                .filter(|d| d.severity == severity)
                .count()
        };
        let mut by_code = BTreeMap::new();
        for diagnostic in diagnostics {
            *by_code.entry(diagnostic.code).or_default() += 1;
        }
        Self {
            files,
            problems: diagnostics.len(),
            errors: count(Severity::Error),
            warnings: count(Severity::Warning),
            infos: count(Severity::Info),
            by_code,
        }
    }

    /// Whether any finding is an error or a warning. `buzzard check`
    /// exits with status 1 then, unless `--exit-zero` is set.
    pub fn failed(&self) -> bool {
        self.errors > 0 || self.warnings > 0
    }
}

/// The findings of a check that are shown, with the counts of all of
/// them. See [`limit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown<'a> {
    /// The findings that are shown: the first ones in the order of
    /// [`lint`](crate::lint).
    pub diagnostics: &'a [Diagnostic],
    /// The counts of every finding, shown or not.
    pub counts: Counts,
    /// How many findings are not shown.
    pub not_shown: usize,
}

/// Keep the first `limit` findings, or all of them without a limit.
///
/// `diagnostics` is the result of [`lint`](crate::lint), which is sorted
/// by path, code, severity, and message, so the same input and limit give
/// the same findings. A limit of 0 shows none and still counts all.
pub fn limit(diagnostics: &[Diagnostic], files: usize, limit: Option<usize>) -> Shown<'_> {
    let shown = limit.map_or(diagnostics.len(), |limit| limit.min(diagnostics.len()));
    Shown {
        diagnostics: &diagnostics[..shown],
        counts: Counts::of(diagnostics, files),
        not_shown: diagnostics.len() - shown,
    }
}

/// The line that says how many findings a limit left out. `None` when
/// none was left out.
pub fn not_shown_line(not_shown: usize) -> Option<String> {
    (not_shown > 0).then(|| format!("... {not_shown} more not shown."))
}

/// The summary of a [`Shown`] in JSON: the counts, and how many findings
/// are in the list and how many are not.
#[derive(Serialize)]
struct SummaryJson<'a> {
    #[serde(flatten)]
    counts: &'a Counts,
    shown: usize,
    not_shown: usize,
}

/// The JSON shape of a [`Shown`].
#[derive(Serialize)]
struct ShownJson<'a> {
    diagnostics: &'a [Diagnostic],
    summary: SummaryJson<'a>,
}

impl Shown<'_> {
    /// Whether the check failed. This counts every finding, not only the
    /// ones that are shown.
    pub fn failed(&self) -> bool {
        self.counts.failed()
    }

    /// The report for a person: one line per shown finding, the line of
    /// [`not_shown_line`] when findings were left out, and the summary
    /// line with the counts of all findings.
    ///
    /// Without a limit this is [`to_human`], byte for byte.
    pub fn to_human(&self) -> String {
        let mut out = String::new();
        for d in self.diagnostics {
            finding_line(&mut out, d);
        }
        if let Some(line) = not_shown_line(self.not_shown) {
            let _ = writeln!(out, "{line}");
        }
        summary_line(&mut out, &self.counts);
        out
    }

    /// The report as JSON: an object with the shown findings under
    /// `diagnostics`, each as `buzzard check --format json` prints it,
    /// and under `summary` the counts of all findings (`files`,
    /// `problems`, `errors`, `warnings`, `infos`, `by_code`) with `shown`
    /// and `not_shown`.
    ///
    /// This is the shape of a check with a limit. Without one,
    /// `buzzard check --format json` prints the list alone.
    #[expect(
        clippy::missing_panics_doc,
        reason = "paths, strings, and counts always serialize"
    )]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self.json()).expect("a check report always serializes")
    }

    /// [`Shown::to_value`] as indented text that ends in a line break,
    /// with the keys in the order of the description there.
    #[expect(
        clippy::missing_panics_doc,
        reason = "paths, strings, and counts always serialize"
    )]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.json()).expect("a check report always serializes") + "\n"
    }

    fn json(&self) -> ShownJson<'_> {
        ShownJson {
            diagnostics: self.diagnostics,
            summary: SummaryJson {
                counts: &self.counts,
                shown: self.diagnostics.len(),
                not_shown: self.not_shown,
            },
        }
    }

    /// The report as a SARIF log ([`crate::sarif::to_sarif`]) of the
    /// shown findings. When findings were left out, the run has the
    /// property bag `properties.buzzard` with the keys of the JSON
    /// summary, so that a reader of the log can tell it is not complete.
    /// Without a limit the log is that of [`crate::sarif::to_sarif`].
    pub fn to_sarif(&self, workspace: &Workspace, tool_version: &str) -> Value {
        let mut log = crate::sarif::to_sarif(workspace, self.diagnostics, tool_version);
        if self.not_shown > 0 {
            let summary = SummaryJson {
                counts: &self.counts,
                shown: self.diagnostics.len(),
                not_shown: self.not_shown,
            };
            if let (Some(run), Ok(summary)) = (
                log.get_mut("runs").and_then(|runs| runs.get_mut(0)),
                serde_json::to_value(summary),
            ) {
                run["properties"] = json!({ "buzzard": summary });
            }
        }
        log
    }
}

fn finding_line(out: &mut String, d: &Diagnostic) {
    // The path and the message are data from a file system and from
    // files. Neither may start a second line.
    let _ = writeln!(
        out,
        "{}: {}[{}]: {}",
        printable::path(&d.path),
        d.severity.as_str(),
        d.code,
        printable::text(&d.message)
    );
}

fn summary_line(out: &mut String, counts: &Counts) {
    let file_count = counts.files;
    let files = if file_count == 1 { "file" } else { "files" };
    if counts.problems == 0 {
        let _ = writeln!(out, "No problems found in {file_count} {files}.");
    } else {
        let (errors, warnings) = (counts.errors, counts.warnings);
        let _ = writeln!(
            out,
            "Found {} problem{} ({errors} error{}, {warnings} warning{}) in {file_count} {files}.",
            counts.problems,
            if counts.problems == 1 { "" } else { "s" },
            if errors == 1 { "" } else { "s" },
            if warnings == 1 { "" } else { "s" },
        );
    }
}

/// The report of `buzzard check` for a person: one line per finding and
/// a summary line. A path and a message are printed on one line each
/// ([`crate::printable`]), so a file name cannot add a line that looks
/// like a finding.
pub fn to_human(diagnostics: &[Diagnostic], file_count: usize) -> String {
    limit(diagnostics, file_count, None).to_human()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn finding(path: &str, code: &'static str, severity: Severity) -> Diagnostic {
        Diagnostic {
            path: PathBuf::from(path),
            code,
            severity,
            message: format!("message of {code}"),
            related: Vec::new(),
        }
    }

    fn findings() -> Vec<Diagnostic> {
        vec![
            finding("a.vi", "BZ000", Severity::Error),
            finding("a.vi", "BZ001", Severity::Warning),
            finding("b.vi", "BZ001", Severity::Warning),
            finding("c.vi", "BZ202", Severity::Info),
        ]
    }

    #[test]
    fn the_report_without_a_limit_is_the_one_the_command_always_printed() {
        assert_eq!(to_human(&[], 1), "No problems found in 1 file.\n");
        assert_eq!(to_human(&[], 0), "No problems found in 0 files.\n");
        assert_eq!(
            to_human(&findings(), 3),
            "a.vi: error[BZ000]: message of BZ000\n\
             a.vi: warning[BZ001]: message of BZ001\n\
             b.vi: warning[BZ001]: message of BZ001\n\
             c.vi: info[BZ202]: message of BZ202\n\
             Found 4 problems (1 error, 2 warnings) in 3 files.\n"
        );
        assert_eq!(
            to_human(&findings()[..1], 1),
            "a.vi: error[BZ000]: message of BZ000\n\
             Found 1 problem (1 error, 0 warnings) in 1 file.\n"
        );
    }

    #[test]
    fn a_limit_shortens_the_list_and_keeps_the_counts() {
        let all = findings();
        let shown = limit(&all, 3, Some(1));
        assert_eq!(shown.diagnostics, &all[..1]);
        assert_eq!(shown.not_shown, 3);
        assert_eq!(shown.counts, Counts::of(&all, 3));
        assert_eq!(
            (
                shown.counts.problems,
                shown.counts.errors,
                shown.counts.warnings,
                shown.counts.infos
            ),
            (4, 1, 2, 1)
        );
        assert_eq!(
            shown.to_human(),
            "a.vi: error[BZ000]: message of BZ000\n\
             ... 3 more not shown.\n\
             Found 4 problems (1 error, 2 warnings) in 3 files.\n"
        );
        let json = shown.to_value();
        assert_eq!(json["diagnostics"].as_array().unwrap().len(), 1);
        assert_eq!(
            json["diagnostics"][0],
            serde_json::to_value(&all[0]).unwrap()
        );
        assert_eq!(
            json["summary"],
            json!({
                "files": 3, "problems": 4, "errors": 1, "warnings": 2, "infos": 1,
                "by_code": { "BZ000": 1, "BZ001": 2, "BZ202": 1 },
                "shown": 1, "not_shown": 3,
            })
        );
        assert_eq!(
            serde_json::from_str::<Value>(&shown.to_json()).unwrap(),
            json
        );
        assert!(shown.to_json().ends_with("}\n"));

        // A limit at or over the number of findings leaves nothing out.
        for enough in [Some(4), Some(5), Some(usize::MAX), None] {
            let shown = limit(&all, 3, enough);
            assert_eq!(shown.not_shown, 0);
            assert_eq!(shown.to_human(), to_human(&all, 3));
            assert_eq!(shown.to_value()["summary"]["shown"], 4);
        }
        // A limit of zero shows the counts alone.
        let none = limit(&all, 3, Some(0));
        assert_eq!(
            none.to_human(),
            "... 4 more not shown.\nFound 4 problems (1 error, 2 warnings) in 3 files.\n"
        );
        assert_eq!(not_shown_line(0), None);
    }

    #[test]
    fn the_status_counts_the_findings_that_are_not_shown() {
        // The only finding that fails the check is past the limit.
        let all = vec![
            finding("a.vi", "BZ202", Severity::Info),
            finding("b.vi", "BZ001", Severity::Warning),
        ];
        assert!(limit(&all, 2, Some(1)).failed());
        assert!(limit(&all, 2, Some(0)).failed());
        assert!(!limit(&all[..1], 1, Some(1)).failed());
        assert!(!limit(&[], 1, None).failed());
    }

    #[test]
    fn a_sarif_log_says_when_it_is_not_complete() {
        let all = findings();
        let workspace = Workspace::default();
        let whole = limit(&all, 3, None).to_sarif(&workspace, "1.2.3");
        assert_eq!(whole, crate::sarif::to_sarif(&workspace, &all, "1.2.3"));
        assert!(whole["runs"][0].get("properties").is_none());
        let cut = limit(&all, 3, Some(2)).to_sarif(&workspace, "1.2.3");
        assert_eq!(cut["runs"][0]["results"].as_array().unwrap().len(), 2);
        let summary = &cut["runs"][0]["properties"]["buzzard"];
        assert_eq!(summary["not_shown"], 2);
        assert_eq!(summary["shown"], 2);
        assert_eq!(summary["problems"], 4);
    }

    #[test]
    fn a_file_name_cannot_forge_a_line_of_the_report() {
        let mut forged = finding(
            "dir/a.vi\nforged.vi: warning[BZ001]: compiled code",
            "BZ000",
            Severity::Error,
        );
        forged.message = "cannot analyze file: x\nb.vi: error[BZ005]: y".to_owned();
        let escape = finding("\u{1b}[2Jclear.vi", "BZ001", Severity::Warning);
        let report = to_human(&[forged.clone(), escape.clone()], 2);
        // Two findings and the summary, and no line of the file's making.
        assert_eq!(report.lines().count(), 3, "{report}");
        assert!(!report.contains('\u{1b}'));
        assert!(report
            .lines()
            .all(|line| !line.starts_with("forged.vi") && !line.starts_with("b.vi")));
        assert!(report.contains("a.vi\\nforged.vi: warning[BZ001]: compiled code: error[BZ000]"));
        assert!(report.contains("\\u{1b}[2Jclear.vi: warning[BZ001]"));
        // JSON holds the name itself, as a string that JSON escapes.
        let all = [forged, escape];
        let json = limit(&all, 2, Some(2)).to_json();
        assert!(!json.contains('\u{1b}'));
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed["diagnostics"][0]["path"],
            "dir/a.vi\nforged.vi: warning[BZ001]: compiled code"
        );
        assert_eq!(parsed["diagnostics"][1]["path"], "\u{1b}[2Jclear.vi");
    }
}
