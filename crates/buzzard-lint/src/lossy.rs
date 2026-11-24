//! Paths as JSON strings.
//!
//! A path on disk need not be Unicode, and the serializer of the standard
//! path type refuses one that is not. A report must not fail on the name
//! of a file, so every path in a report is written with these functions:
//! a Unicode path as it is, and any other as [`Path::to_string_lossy`]
//! reads it.

use std::path::{Path, PathBuf};

use serde::ser::{SerializeSeq, SerializeTuple};
use serde::Serializer;

/// Serializes one path.
pub(crate) fn path<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}

/// Serializes a list of paths.
pub(crate) fn paths<S: Serializer>(paths: &[PathBuf], serializer: S) -> Result<S::Ok, S::Error> {
    let mut seq = serializer.serialize_seq(Some(paths.len()))?;
    for path in paths {
        seq.serialize_element(&path.to_string_lossy())?;
    }
    seq.end()
}

/// Serializes a path that may be absent.
#[expect(
    clippy::ref_option,
    reason = "serde calls a `serialize_with` function with a reference to the field"
)]
pub(crate) fn option<S: Serializer>(
    path: &Option<PathBuf>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match path {
        Some(path) => serializer.serialize_some(&path.to_string_lossy()),
        None => serializer.serialize_none(),
    }
}

/// A path and a text as the pair they are in a list.
struct Pair<'a>(&'a Path, &'a str);

impl serde::Serialize for Pair<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut pair = serializer.serialize_tuple(2)?;
        pair.serialize_element(&self.0.to_string_lossy())?;
        pair.serialize_element(self.1)?;
        pair.end()
    }
}

/// Serializes a list of paths that each come with a text.
pub(crate) fn with_text<S: Serializer>(
    pairs: &[(PathBuf, String)],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut seq = serializer.serialize_seq(Some(pairs.len()))?;
    for (path, text) in pairs {
        seq.serialize_element(&Pair(path, text))?;
    }
    seq.end()
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;

    #[derive(Serialize)]
    struct All {
        #[serde(serialize_with = "path")]
        one: PathBuf,
        #[serde(serialize_with = "paths")]
        many: Vec<PathBuf>,
        #[serde(serialize_with = "option")]
        maybe: Option<PathBuf>,
        #[serde(serialize_with = "option")]
        absent: Option<PathBuf>,
        #[serde(serialize_with = "with_text")]
        pairs: Vec<(PathBuf, String)>,
    }

    /// What the derived serializer writes for the same fields.
    #[derive(Serialize)]
    struct Derived {
        one: PathBuf,
        many: Vec<PathBuf>,
        maybe: Option<PathBuf>,
        absent: Option<PathBuf>,
        pairs: Vec<(PathBuf, String)>,
    }

    #[test]
    fn a_unicode_path_is_written_as_the_derived_serializer_writes_it() {
        let path = PathBuf::from("dir").join("テスト \n.vi");
        let all = All {
            one: path.clone(),
            many: vec![path.clone(), PathBuf::from("b.vi")],
            maybe: Some(path.clone()),
            absent: None,
            pairs: vec![(path.clone(), "reason".to_owned())],
        };
        let derived = Derived {
            one: path.clone(),
            many: vec![path.clone(), PathBuf::from("b.vi")],
            maybe: Some(path.clone()),
            absent: None,
            pairs: vec![(path, "reason".to_owned())],
        };
        assert_eq!(
            serde_json::to_string_pretty(&all).unwrap(),
            serde_json::to_string_pretty(&derived).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_unicode_serializes() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = PathBuf::from(OsStr::from_bytes(b"a\xff.vi"));
        assert!(serde_json::to_string(&path).is_err());
        let all = All {
            one: path.clone(),
            many: vec![path.clone()],
            maybe: Some(path.clone()),
            absent: None,
            pairs: vec![(path, "reason".to_owned())],
        };
        let json = serde_json::to_string(&all).unwrap();
        assert_eq!(json.matches("a\u{fffd}.vi").count(), 4);
    }
}
