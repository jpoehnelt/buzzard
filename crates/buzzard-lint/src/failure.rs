//! The errors that end a command, in a form a program can read.
//!
//! A command that cannot do what was asked exits with status 2 and prints
//! one line of prose on stderr. A caller that asked for a machine format
//! also gets the error as one JSON object, so that it can tell a path
//! that does not exist from an unknown rule code without reading prose:
//!
//! ```json
//! {"error":{"kind":"path_not_found","message":"path does not exist: a.vi","path":"a.vi"}}
//! ```
//!
//! [`ErrorKind`] is the closed set of kinds, and [`Failure`] is one error.
//! The command line and the Model Context Protocol server both report
//! their errors through these two types.

use std::fmt;
use std::path::Path;

use serde::Serialize;

use crate::printable;

/// What went wrong, as one word that a program can match on.
///
/// The names ([`ErrorKind::as_str`]) are a contract: a name is never
/// renamed or given another meaning. A new kind may be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorKind {
    /// A path argument names nothing: no file, no directory, and no member
    /// of an archive.
    PathNotFound,
    /// A rule code that is not in [`RULES`](crate::RULES) was given.
    UnknownRule,
    /// The file exists and cannot be read, or what it holds does not parse.
    UnreadableFile,
    /// An argument has a value that cannot be used: a path with a control
    /// character, a root that is not a directory, a channel that the file
    /// does not have.
    BadArgument,
    /// A path argument leads outside the directories that the server was
    /// confined to.
    OutsideRoot,
    /// The file was read, and what was asked for cannot be made from it: a
    /// picture of a file without a block diagram, the diagram of a whole
    /// archive.
    Unsupported,
    /// The output file exists and replacing it was not asked for.
    OutputExists,
    /// The report or the output file could not be written.
    WriteFailed,
    /// The command line or the call itself is malformed: an unknown flag
    /// or argument, a missing value, a value of the wrong type.
    Usage,
    /// buzzard failed for a reason that the request did not cause, such as
    /// a thread pool that cannot start.
    Internal,
}

impl ErrorKind {
    /// Every kind, in the order of the documentation.
    pub const ALL: [Self; 10] = [
        Self::PathNotFound,
        Self::UnknownRule,
        Self::UnreadableFile,
        Self::BadArgument,
        Self::OutsideRoot,
        Self::Unsupported,
        Self::OutputExists,
        Self::WriteFailed,
        Self::Usage,
        Self::Internal,
    ];

    /// The name in the JSON field `kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PathNotFound => "path_not_found",
            Self::UnknownRule => "unknown_rule",
            Self::UnreadableFile => "unreadable_file",
            Self::BadArgument => "bad_argument",
            Self::OutsideRoot => "outside_root",
            Self::Unsupported => "unsupported",
            Self::OutputExists => "output_exists",
            Self::WriteFailed => "write_failed",
            Self::Usage => "usage",
            Self::Internal => "internal",
        }
    }

    /// One sentence on when the kind is reported, for a listing.
    pub fn meaning(self) -> &'static str {
        match self {
            Self::PathNotFound => {
                "A path names no file, no directory, and no member of an archive."
            }
            Self::UnknownRule => "A rule code is not one that `buzzard rules` lists.",
            Self::UnreadableFile => "A file cannot be read, or what it holds does not parse.",
            Self::BadArgument => {
                "An argument has a value that cannot be used, such as a path with a control \
                 character."
            }
            Self::OutsideRoot => {
                "A path leads outside the directories that the server was confined to."
            }
            Self::Unsupported => {
                "The file was read, and what was asked for cannot be made from it."
            }
            Self::OutputExists => "The output file exists and `--force` was not given.",
            Self::WriteFailed => "The report or the output file could not be written.",
            Self::Usage => "The command line or the call itself is malformed.",
            Self::Internal => "buzzard failed for a reason that the request did not cause.",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One error that ends a command or a tool call.
///
/// [`Failure::message`] is the line of prose, without the `error: ` that
/// the command line puts before it. A path in the message is printed on
/// one line ([`printable::path`]). [`Failure::path`] is the path itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    /// What went wrong.
    pub kind: ErrorKind,
    /// The reason, for a person.
    pub message: String,
    /// The path the error is about, when it is about one. A path that is
    /// not Unicode is given as [`Path::to_string_lossy`] reads it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// The JSON shape of a [`Failure`]: an object with the one key `error`.
#[derive(Serialize)]
struct Envelope<'a> {
    error: &'a Failure,
}

impl Failure {
    /// An error of `kind` that is about no path.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            path: None,
        }
    }

    /// The same error, about `path`.
    #[must_use]
    pub fn with_path(mut self, path: &Path) -> Self {
        self.path = Some(path.to_string_lossy().into_owned());
        self
    }

    /// The same error, about a path that is shown to the caller as
    /// `shown`, for a server that reports paths in its own spelling.
    #[must_use]
    pub fn with_shown_path(mut self, shown: impl Into<String>) -> Self {
        self.path = Some(shown.into());
        self
    }

    /// `path` names nothing that can be analyzed
    /// ([`archive::exists`](crate::archive::exists)).
    pub fn path_not_found(path: &Path) -> Self {
        Self::new(
            ErrorKind::PathNotFound,
            format!("path does not exist: {}", printable::path(path)),
        )
        .with_path(path)
    }

    /// One or more rule codes are not in [`RULES`](crate::RULES).
    pub fn unknown_rules<S: AsRef<str>>(codes: &[S]) -> Self {
        let codes: Vec<&str> = codes.iter().map(AsRef::as_ref).collect();
        Self::new(
            ErrorKind::UnknownRule,
            format!("unknown rule code(s): {codes:?}"),
        )
    }

    /// The file at `path` cannot be read or does not parse, for `reason`.
    pub fn unreadable(path: &Path, reason: impl fmt::Display) -> Self {
        Self::new(
            ErrorKind::UnreadableFile,
            format!("{}: {reason}", printable::path(path)),
        )
        .with_path(path)
    }

    /// What was asked for cannot be made from the file at `path`, for
    /// `reason`.
    pub fn unsupported(path: &Path, reason: impl fmt::Display) -> Self {
        Self::new(
            ErrorKind::Unsupported,
            format!("{}: {reason}", printable::path(path)),
        )
        .with_path(path)
    }

    /// An argument has a value that cannot be used.
    pub fn bad_argument(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::BadArgument, message)
    }

    /// `path` is a directory where one file is expected. The caller can
    /// correct the argument, so the kind is `bad_argument` on every
    /// surface.
    pub fn not_a_file(path: &Path) -> Self {
        Self::bad_argument(format!(
            "{} is a directory; pass one file",
            printable::path(path)
        ))
        .with_path(path)
    }

    /// The command line or the call is malformed.
    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Usage, message)
    }

    /// The output file at `path` exists and may not be replaced.
    pub fn output_exists(path: &Path) -> Self {
        Self::new(
            ErrorKind::OutputExists,
            format!(
                "{} exists; pass --force to replace it",
                printable::path(path)
            ),
        )
        .with_path(path)
    }

    /// The report could not be written to stdout.
    pub fn write_failed(err: &std::io::Error) -> Self {
        Self::new(
            ErrorKind::WriteFailed,
            format!("cannot write output: {err}"),
        )
    }

    /// The output file at `path` could not be created or written.
    pub fn cannot_create(path: &Path, err: &std::io::Error) -> Self {
        Self::new(
            ErrorKind::WriteFailed,
            format!("cannot create {}: {err}", printable::path(path)),
        )
        .with_path(path)
    }

    /// The error as a JSON value: `{"error": {"kind", "message", "path"}}`,
    /// without `path` when the error is about none.
    pub fn to_value(&self) -> serde_json::Value {
        let mut error = serde_json::Map::new();
        error.insert("kind".to_owned(), self.kind.as_str().into());
        error.insert("message".to_owned(), self.message.as_str().into());
        if let Some(path) = &self.path {
            error.insert("path".to_owned(), path.as_str().into());
        }
        serde_json::json!({ "error": error })
    }

    /// The error as one line of JSON that ends in a line break, with the
    /// keys in the order `kind`, `message`, `path`.
    #[expect(
        clippy::missing_panics_doc,
        reason = "three strings in a struct always serialize"
    )]
    pub fn to_json(&self) -> String {
        serde_json::to_string(&Envelope { error: self }).expect("a failure always serializes")
            + "\n"
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

impl From<Failure> for String {
    fn from(failure: Failure) -> Self {
        failure.message
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::io;

    use super::*;

    #[test]
    fn every_kind_has_one_name_and_serializes_as_it() {
        let names: BTreeSet<&str> = ErrorKind::ALL.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(names.len(), ErrorKind::ALL.len());
        for kind in ErrorKind::ALL {
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::Value::from(kind.as_str())
            );
            assert_eq!(kind.to_string(), kind.as_str());
            assert!(kind.meaning().ends_with('.'));
            assert!(kind
                .as_str()
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_'));
        }
        // The names are a contract.
        assert_eq!(
            ErrorKind::ALL.map(ErrorKind::as_str),
            [
                "path_not_found",
                "unknown_rule",
                "unreadable_file",
                "bad_argument",
                "outside_root",
                "unsupported",
                "output_exists",
                "write_failed",
                "usage",
                "internal",
            ]
        );
    }

    #[test]
    fn the_existing_error_lines_keep_their_words() {
        let missing = Failure::path_not_found(Path::new("no/such.vi"));
        assert_eq!(missing.kind, ErrorKind::PathNotFound);
        assert_eq!(
            missing.to_string(),
            format!("path does not exist: {}", Path::new("no/such.vi").display())
        );
        assert_eq!(missing.path.as_deref(), Some("no/such.vi"));

        let codes = ["BZ999".to_owned(), "x".to_owned()];
        let unknown: Vec<&String> = codes.iter().collect();
        let failure = Failure::unknown_rules(&unknown);
        assert_eq!(failure.kind, ErrorKind::UnknownRule);
        // As the command line printed a `Vec<&String>` before.
        assert_eq!(
            failure.message,
            format!("unknown rule code(s): {unknown:?}")
        );
        assert_eq!(failure.message, r#"unknown rule code(s): ["BZ999", "x"]"#);
        assert_eq!(failure.path, None);

        let unreadable = Failure::unreadable(Path::new("a.vi"), "file is truncated");
        assert_eq!(unreadable.kind, ErrorKind::UnreadableFile);
        assert_eq!(unreadable.message, "a.vi: file is truncated");

        let full = io::Error::other("disk full");
        let failure = Failure::write_failed(&full);
        assert_eq!(failure.kind, ErrorKind::WriteFailed);
        assert_eq!(failure.message, "cannot write output: disk full");
        let failure = Failure::cannot_create(Path::new("out.csv"), &full);
        assert_eq!(failure.message, "cannot create out.csv: disk full");
        assert_eq!(
            Failure::output_exists(Path::new("out.csv")).kind,
            ErrorKind::OutputExists
        );
        assert_eq!(String::from(unreadable), "a.vi: file is truncated");
    }

    #[test]
    fn a_directory_in_place_of_a_file_is_an_argument_to_correct() {
        let failure = Failure::not_a_file(Path::new("lib\nx"));
        assert_eq!(failure.kind, ErrorKind::BadArgument);
        assert_eq!(failure.message, "lib\\nx is a directory; pass one file");
        assert_eq!(failure.path.as_deref(), Some("lib\nx"));
    }

    #[test]
    fn the_json_is_one_line_with_one_object() {
        let failure = Failure::path_not_found(Path::new("a.vi"));
        assert_eq!(
            failure.to_json(),
            "{\"error\":{\"kind\":\"path_not_found\",\"message\":\"path does not exist: a.vi\",\
             \"path\":\"a.vi\"}}\n"
        );
        let usage = Failure::usage("unexpected argument '--x'");
        assert_eq!(
            usage.to_json(),
            "{\"error\":{\"kind\":\"usage\",\"message\":\"unexpected argument '--x'\"}}\n"
        );
        for failure in [failure, usage] {
            let parsed: serde_json::Value = serde_json::from_str(&failure.to_json()).unwrap();
            assert_eq!(parsed, failure.to_value());
            assert_eq!(parsed["error"]["kind"], failure.kind.as_str());
        }
    }

    #[test]
    fn a_path_with_a_line_break_stays_on_one_line_and_whole_in_json() {
        let forged = Path::new("a.vi\nerror: forged");
        let failure = Failure::path_not_found(forged);
        assert_eq!(failure.message, "path does not exist: a.vi\\nerror: forged");
        assert_eq!(failure.to_json().lines().count(), 1);
        let parsed: serde_json::Value = serde_json::from_str(&failure.to_json()).unwrap();
        // The JSON string holds the name itself.
        assert_eq!(parsed["error"]["path"], "a.vi\nerror: forged");
        let shown = Failure::bad_argument("x").with_shown_path("lib/a.vi");
        assert_eq!(shown.path.as_deref(), Some("lib/a.vi"));
    }
}
