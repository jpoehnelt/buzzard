//! Names and paths as text for a person to read.
//!
//! A file name is data. On most systems it may hold a line break or the
//! start of a terminal escape sequence, and so may the name of a member of
//! an archive. Printed as it is, such a name would add a line to a report
//! that buzzard never wrote, or would drive the terminal. Every renderer
//! of human, text, or Markdown output therefore prints names and paths
//! through this module. JSON and SARIF output hold the name itself, which
//! a JSON string already escapes.

use std::path::Path;

/// `text` on one line. Each ASCII control character is written as its
/// Rust escape (`\n`, `\t`, `\u{1b}`), and everything else is as it was.
/// The bytes 0x80 to 0x9f of a name in a legacy code page are letters,
/// not controls, and are kept.
pub fn text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// A path on one line, as [`text`] prints it. A path that is not Unicode
/// is shown as [`Path::display`] shows it.
pub fn path(path: &Path) -> String {
    text(&path.display().to_string())
}

/// The last component of a path on one line, or the whole path when it
/// has none. This is the title of a report about one file.
pub fn file_name(path: &Path) -> String {
    text(
        &path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_printed_on_one_line_and_otherwise_as_it_is() {
        assert_eq!(text("a\\b `c` |d|.vi"), "a\\b `c` |d|.vi");
        assert_eq!(
            text("a\nb\r\t\0\u{1b}[0m\u{7f}"),
            "a\\nb\\r\\t\\u{0}\\u{1b}[0m\\u{7f}"
        );
        // Bytes of a legacy code page above 0x7f are letters.
        assert_eq!(text("\u{83}\u{65}\u{e9}"), "\u{83}\u{65}\u{e9}");
        // Characters that are legal in a file name and that buzzard never
        // decodes stay as they are.
        assert_eq!(text("50% done?#1.vi"), "50% done?#1.vi");
    }

    #[test]
    fn a_path_keeps_its_separators_and_loses_its_line_breaks() {
        let forged = Path::new("dir").join("a.vi\nforged.vi: warning[BZ001]: x");
        let shown = path(&forged);
        assert_eq!(shown.lines().count(), 1);
        assert!(shown.starts_with(&Path::new("dir").join("a.vi").display().to_string()));
        assert!(shown.ends_with("\\nforged.vi: warning[BZ001]: x"));
        assert_eq!(file_name(&forged), "a.vi\\nforged.vi: warning[BZ001]: x");
        assert_eq!(file_name(Path::new("..")), "..");
        let plain = Path::new("lib").join("util.llb").join("Init.vi");
        assert_eq!(path(&plain), plain.display().to_string());
    }
}
