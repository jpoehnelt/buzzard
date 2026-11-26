//! The report of one file as one HTML page.
//!
//! The page holds everything it shows. It loads no script, style sheet,
//! font, or image, and links to nothing. The only address in it is the
//! namespace of the inlined SVG. It has no time and no path but the one
//! the caller passed, so the same file gives the same bytes.
//!
//! Every path, name, and message is data. Each is printed on one line
//! ([`crate::printable`]) and then escaped as HTML ([`escape`]), so a name
//! cannot add a tag, a script, or a line to the page.
//!
//! The page's own style uses only classes that start with `bz-`. The SVG
//! of [`crate::render::to_svg`] brings a style for its own classes
//! (`node`, `wire`, and the like), and the two do not meet.

use std::fmt::Write;
use std::path::Path;

use buzzard_rsrc::{BlockDiagram, ViMetadata};

use crate::deps::plural;
use crate::diagnostic::Diagnostic;
use crate::{printable, render, summary};

/// What the report of one file shows.
#[derive(Debug, Clone, Copy)]
pub struct Page<'a> {
    /// The file as the caller named it: a path or a member path.
    pub path: &'a Path,
    /// The SHA-256 of the bytes that were read, as lowercase hex. For a
    /// member of an archive, these are the member's own bytes.
    pub sha256: &'a str,
    /// What the file is.
    pub metadata: &'a ViMetadata,
    /// The block diagram, when buzzard decoded one.
    pub diagram: Option<&'a BlockDiagram>,
    /// The findings of `check` on this file alone.
    pub findings: &'a [Diagnostic],
}

const STYLE: &str = "\
.bz-page{margin:0;padding:16px;background:#ffffff;color:#111827;\
font-family:system-ui,-apple-system,Segoe UI,Helvetica,Arial,sans-serif;line-height:1.45}\n\
.bz-main{max-width:1100px;margin:0 auto}\n\
.bz-title{font-size:1.4rem;margin:0 0 4px;overflow-wrap:anywhere}\n\
.bz-hash{margin:0 0 16px;color:#374151;overflow-wrap:anywhere}\n\
.bz-section{margin:24px 0}\n\
.bz-heading{font-size:1.1rem;margin:0 0 8px;border-bottom:1px solid #d1d5db;padding-bottom:4px}\n\
.bz-code{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:0.9em}\n\
.bz-subheading{font-size:1rem;margin:16px 0 8px}\n\
.bz-table{border-collapse:collapse;margin:0}\n\
.bz-table th,.bz-table td{text-align:left;vertical-align:top;padding:4px 16px 4px 0;\
border-bottom:1px solid #e5e7eb;overflow-wrap:anywhere}\n\
.bz-table th{font-weight:600;white-space:nowrap}\n\
.bz-list{margin:0;padding-left:20px}\n\
.bz-note{margin:0 0 8px}\n\
.bz-warn{margin:0 0 8px;padding:8px 12px;background:#fef3c7;border-left:4px solid #b45309}\n\
.bz-diagram{overflow:auto;border:1px solid #d1d5db;background:#ffffff}\n\
.bz-findings{margin:0;padding-left:20px}\n\
.bz-finding{margin:4px 0;overflow-wrap:anywhere}\n\
.bz-error{color:#b91c1c;font-weight:bold}\n\
.bz-warning{color:#b45309;font-weight:bold}\n\
.bz-info{color:#1d4ed8;font-weight:bold}\n\
@media (prefers-color-scheme: dark){\
.bz-page{background:#111827;color:#f3f4f6}\
.bz-hash{color:#d1d5db}\
.bz-table th,.bz-table td{border-color:#374151}\
.bz-warn{background:#451a03}\
.bz-heading{border-color:#374151}\
.bz-error{color:#f87171}.bz-warning{color:#fbbf24}.bz-info{color:#93c5fd}}\n";

/// `text` as HTML: on one line ([`printable::text`]), with `&`, `<`, `>`,
/// `"`, and `'` written as entities. The result may go in an element or
/// in a quoted attribute.
pub fn escape(text: &str) -> String {
    markup(&printable::text(text))
}

/// `text` with the five characters that HTML gives a meaning written as
/// entities. It does nothing else.
fn markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A path as HTML, on one line ([`printable::path`]).
fn path(path: &Path) -> String {
    markup(&printable::path(path))
}

/// The report of one file as a complete HTML document.
pub fn to_html(page: &Page<'_>) -> String {
    let shown = path(page.path);
    let mut out = String::new();
    out.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    let _ = writeln!(out, "<title>{shown} - buzzard</title>");
    let _ = write!(out, "<style>\n{STYLE}</style>\n</head>\n");
    out.push_str("<body class=\"bz-page\">\n<main class=\"bz-main\">\n");
    let _ = writeln!(out, "<h1 class=\"bz-title\">{shown}</h1>");
    let _ = writeln!(
        out,
        "<p class=\"bz-hash\">SHA-256 <code class=\"bz-code\">{}</code></p>",
        escape(page.sha256)
    );

    metadata(&mut out, page.metadata);

    diagram(&mut out, page.diagram);
    findings(&mut out, page.findings);

    out.push_str("</main>\n</body>\n</html>\n");
    out
}

/// The section of the metadata: a table of the rows that the Markdown
/// summary prints ([`summary::metadata_rows`]), and the targets of the
/// file's links.
fn metadata(out: &mut String, meta: &ViMetadata) {
    out.push_str("<section class=\"bz-section\">\n<h2 class=\"bz-heading\">Metadata</h2>\n");
    out.push_str("<table class=\"bz-table\">\n<tbody>\n");
    for row in summary::metadata_rows(meta, None) {
        let code = row.code.map_or_else(String::new, |code| {
            format!(" (<code class=\"bz-code\">{}</code>)", escape(&code))
        });
        let _ = writeln!(
            out,
            "<tr><th scope=\"row\">{}</th><td>{}{code}</td></tr>",
            escape(row.label),
            escape(&row.value)
        );
    }
    out.push_str("</tbody>\n</table>\n");
    let targets = summary::dependencies(meta);
    let _ = writeln!(
        out,
        "<h3 class=\"bz-subheading\">Dependencies ({})</h3>",
        targets.len()
    );
    if targets.is_empty() {
        out.push_str("<p class=\"bz-note\">The file links to no other file.</p>\n");
    } else {
        out.push_str("<ul class=\"bz-list\">\n");
        for target in &targets {
            let _ = writeln!(
                out,
                "<li><code class=\"bz-code\">{}</code></li>",
                escape(target)
            );
        }
        out.push_str("</ul>\n");
    }
    out.push_str("</section>\n");
}

/// The section of the block diagram: whether it decoded completely, and
/// the picture.
fn diagram(out: &mut String, diagram: Option<&BlockDiagram>) {
    out.push_str("<section class=\"bz-section\">\n<h2 class=\"bz-heading\">Block diagram</h2>\n");
    let Some(diagram) = diagram else {
        out.push_str(
            "<p class=\"bz-note\">buzzard decoded no block diagram from this file.</p>\n\
             </section>\n",
        );
        return;
    };
    let outline = summary::outline(diagram);
    let _ = writeln!(
        out,
        "<p class=\"bz-note\">{}, {}, {}, {}.</p>",
        plural(outline.nodes, "node", "nodes"),
        plural(outline.terminals, "terminal", "terminals"),
        plural(outline.wires, "wire", "wires"),
        plural(outline.diagrams, "diagram", "diagrams"),
    );
    if outline.complete {
        out.push_str("<p class=\"bz-note\">The diagram decoded completely.</p>\n");
    } else {
        let losses: Vec<String> = outline
            .losses
            .iter()
            .map(|loss| match serde_json::to_value(loss) {
                Ok(serde_json::Value::String(name)) => name,
                _ => format!("{loss:?}"),
            })
            .collect();
        let _ = writeln!(
            out,
            "<p class=\"bz-warn\">The diagram did not decode completely, so no finding \
             about it is proven, and a missing node or wire proves nothing. Losses: {}.</p>",
            escape(&losses.join(", "))
        );
    }
    out.push_str("<div class=\"bz-diagram\">\n");
    out.push_str(&render::to_svg(diagram));
    out.push_str("</div>\n</section>\n");
}

/// The section of the findings of the file checked alone.
fn findings(out: &mut String, findings: &[Diagnostic]) {
    out.push_str(
        "<section class=\"bz-section\">\n<h2 class=\"bz-heading\">Problems found when this \
         file is checked alone</h2>\n",
    );
    out.push_str(
        "<p class=\"bz-note\">buzzard checked this file alone. The rules that compare files \
         (BZ002, BZ005, BZ006) can report something else when the file is checked with the \
         rest of its project.</p>\n",
    );
    if findings.is_empty() {
        out.push_str("<p class=\"bz-note\">No problems found.</p>\n</section>\n");
        return;
    }
    out.push_str("<ul class=\"bz-findings\">\n");
    for finding in findings {
        let severity = finding.severity.as_str();
        let _ = writeln!(
            out,
            "<li class=\"bz-finding\"><span class=\"bz-{severity}\">{severity}[{}]</span> \
             <code class=\"bz-code\">{}</code>: {}</li>",
            escape(finding.code),
            path(&finding.path),
            escape(&finding.message)
        );
    }
    out.push_str("</ul>\n</section>\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Inspection;
    use buzzard_rsrc::Loss;

    fn add_vi() -> Inspection {
        Inspection::from_path(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../buzzard-rsrc/tests/fixtures/pylavi/add.vi"),
        )
        .unwrap()
    }

    #[test]
    fn escape_writes_markup_as_text_and_controls_as_escapes() {
        assert_eq!(
            escape("<script>alert('x' & \"y\")</script>"),
            "&lt;script&gt;alert(&#39;x&#39; &amp; &quot;y&quot;)&lt;/script&gt;"
        );
        assert_eq!(
            escape("a.vi\n<b>\u{1b}[0m\u{202e}iv.exe"),
            "a.vi\\n&lt;b&gt;\\u{1b}[0m\\u{202e}iv.exe"
        );
        assert_eq!(escape("plain name.vi"), "plain name.vi");
        let forged = Path::new("dir").join("a.vi\n<script>x</script>");
        let shown = path(&forged);
        assert_eq!(shown.lines().count(), 1);
        assert!(!shown.contains('<'), "{shown}");
    }

    #[test]
    fn the_metadata_is_a_table_of_the_rows_of_the_markdown_summary() {
        let inspection = add_vi();
        let mut metadata = inspection.metadata.clone();
        metadata.libraries = vec!["<b>Lib</b>.lvlib".to_owned(), "a\nb".to_owned()];
        let mut out = String::new();
        super::metadata(&mut out, &metadata);
        let rows = summary::metadata_rows(&metadata, None);
        assert_eq!(out.matches("<tr>").count(), rows.len(), "{out}");
        for row in &rows {
            assert!(
                out.contains(&format!("<th scope=\"row\">{}</th>", row.label)),
                "{}",
                row.label
            );
        }
        assert!(out.contains(
            "<th scope=\"row\">Library</th><td>&lt;b&gt;Lib&lt;/b&gt;.lvlib &gt; a\\nb</td>"
        ));
        assert!(!out.contains("<b>"));
        assert!(out.contains("The file links to no other file."));
        // The Markdown summary prints the same rows.
        let markdown = summary::to_markdown("add.vi", &metadata, None);
        // A header line, a separator line, and one line per row.
        assert_eq!(markdown.matches("\n| ").count(), rows.len() + 2);
    }

    #[test]
    fn counts_are_written_with_the_noun_that_fits() {
        let inspection = add_vi();
        let page = to_html(&Page {
            path: Path::new("add.vi"),
            sha256: "00",
            metadata: &inspection.metadata,
            diagram: inspection.diagram.as_ref(),
            findings: &[],
        });
        assert!(
            page.contains("2 nodes, 6 terminals, 3 wires, 1 diagram.</p>"),
            "{page}"
        );
        assert!(!page.contains("1 diagrams"));
    }

    #[test]
    fn the_report_marks_an_incomplete_diagram_as_unproven() {
        let inspection = add_vi();
        let mut diagram = inspection.diagram.clone().unwrap();
        let page = |diagram: Option<&BlockDiagram>| {
            to_html(&Page {
                path: Path::new("add.vi"),
                sha256: "00",
                metadata: &inspection.metadata,
                diagram,
                findings: &[],
            })
        };
        let complete = page(Some(&diagram));
        assert!(complete.contains("The diagram decoded completely."));
        assert!(!complete.contains("did not decode completely"));
        assert_eq!(complete.matches("<svg ").count(), 1);

        diagram.root.losses.push(Loss::Wire);
        let damaged = page(Some(&diagram));
        assert!(
            damaged.contains(
                "The diagram did not decode completely, so no finding about it is proven"
            ),
            "{damaged}"
        );
        assert!(damaged.contains("Losses: wire.</p>"), "{damaged}");
        assert!(!damaged.contains("decoded completely."));

        let none = page(None);
        assert!(none.contains("buzzard decoded no block diagram from this file."));
        assert!(!none.contains("<svg"));
    }
}
