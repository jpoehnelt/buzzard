//! Flattened path values (`PTH0`, `PTH1`, `PTH2`).

use std::fmt;

use serde::Serialize;

use crate::error::{malformed, Result};
use crate::reader::{decode_text, Reader};

/// Longest flattened path accepted. Real paths are a few hundred bytes.
pub const MAX_PATH_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathKind {
    Absolute,
    Relative,
    NotAPath,
    Unc,
}

/// A path as stored inside a file: a kind plus a list of components.
///
/// An empty component means "parent directory". Paths into the installation
/// start with a symbolic component such as `<vilib>` and are stored as
/// absolute even though they are location-independent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct LvPath {
    pub kind: PathKind,
    pub components: Vec<String>,
}

impl LvPath {
    /// Parse a path at the start of `data`, returning it and its byte length.
    pub fn parse(data: &[u8]) -> Result<(Self, usize)> {
        let mut r = Reader::new(data, "path");
        let format = r.fourcc()?;
        let wide = match &format {
            b"PTH0" => false,
            b"PTH1" | b"PTH2" => true,
            _ => return Err(malformed("path", "unknown path signature")),
        };
        let size = r.u32()? as usize;
        if size > MAX_PATH_BYTES {
            return Err(malformed(
                "path",
                format!("path of {size} bytes is too long"),
            ));
        }
        let body_start = r.pos();
        let mut body = Reader::new(r.take(size)?, "path");
        let total = body_start + size;

        if size == 0 {
            let path = Self {
                kind: PathKind::Absolute,
                components: Vec::new(),
            };
            return Ok((path, total));
        }

        let (kind, expected) = if wide {
            let kind = match &body.fourcc()? {
                b"abs " => PathKind::Absolute,
                b"rel " => PathKind::Relative,
                b"!pth" => PathKind::NotAPath,
                b"unc " => PathKind::Unc,
                _ => return Err(malformed("path", "unknown path type tag")),
            };
            (kind, None)
        } else {
            let kind = match body.u16()? {
                0 => PathKind::Absolute,
                1 => PathKind::Relative,
                2 => PathKind::NotAPath,
                3 => PathKind::Unc,
                other => return Err(malformed("path", format!("unknown path type {other}"))),
            };
            (kind, Some(body.u16()? as usize))
        };

        let mut components = Vec::new();
        while body.remaining() > 0 {
            // Stop as soon as the declared count is exceeded, so that a
            // stray signature in front of unrelated data is rejected cheaply.
            if expected.is_some_and(|n| components.len() >= n) {
                return Err(malformed("path", "component count does not match"));
            }
            let raw = if wide { body.pstr16()? } else { body.pstr()? };
            components.push(decode_text(raw));
        }
        if expected.is_some_and(|n| n != components.len()) {
            return Err(malformed("path", "component count does not match"));
        }
        Ok((Self { kind, components }, total))
    }

    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    /// The symbolic root such as `<vilib>` or `<userlib>`, if any.
    pub fn symbolic_root(&self) -> Option<&str> {
        let first = self.components.first()?;
        (first.len() > 2 && first.starts_with('<') && first.ends_with('>')).then_some(first)
    }

    /// True for a path pinned to one machine's file system layout.
    pub fn is_machine_absolute(&self) -> bool {
        matches!(self.kind, PathKind::Absolute | PathKind::Unc)
            && !self.is_empty()
            && self.symbolic_root().is_none()
    }

    /// Final component, normally the file name.
    pub fn file_name(&self) -> Option<&str> {
        self.components
            .last()
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }
}

impl fmt::Display for LvPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.kind == PathKind::NotAPath {
            return f.write_str("<Not A Path>");
        }
        let symbolic = self.symbolic_root().is_some();
        let drive = self.components.first().is_some_and(|c| c.len() == 1);
        match self.kind {
            PathKind::Absolute if !symbolic && !drive => f.write_str("/")?,
            PathKind::Unc => f.write_str("//")?,
            _ => {}
        }
        for (i, component) in self.components.iter().enumerate() {
            if i > 0 {
                f.write_str("/")?;
            }
            if component.is_empty() {
                f.write_str("..")?;
            } else if i == 0 && drive && self.kind == PathKind::Absolute {
                write!(f, "{component}:")?;
            } else {
                f.write_str(component)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pth0(kind: u16, components: &[&str]) -> Vec<u8> {
        let mut body = kind.to_be_bytes().to_vec();
        body.extend_from_slice(&(components.len() as u16).to_be_bytes());
        for c in components {
            body.push(c.len() as u8);
            body.extend_from_slice(c.as_bytes());
        }
        let mut out = b"PTH0".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend(body);
        out
    }

    #[test]
    fn parses_relative_pth0() {
        let bytes = pth0(1, &["", "sub", "a.vi"]);
        let (path, len) = LvPath::parse(&bytes).unwrap();
        assert_eq!(len, bytes.len());
        assert_eq!(path.kind, PathKind::Relative);
        assert_eq!(path.to_string(), "../sub/a.vi");
        assert!(!path.is_machine_absolute());
        assert_eq!(path.file_name(), Some("a.vi"));
    }

    #[test]
    fn displays_absolute_forms() {
        let (unix, _) = LvPath::parse(&pth0(0, &["Volumes", "x", "a.vi"])).unwrap();
        assert_eq!(unix.to_string(), "/Volumes/x/a.vi");
        assert!(unix.is_machine_absolute());

        let (win, _) = LvPath::parse(&pth0(0, &["C", "x", "a.vi"])).unwrap();
        assert_eq!(win.to_string(), "C:/x/a.vi");

        let (vilib, _) = LvPath::parse(&pth0(0, &["<vilib>", "Utility", "a.vi"])).unwrap();
        assert_eq!(vilib.to_string(), "<vilib>/Utility/a.vi");
        assert_eq!(vilib.symbolic_root(), Some("<vilib>"));
        assert!(!vilib.is_machine_absolute());
    }

    #[test]
    fn parses_empty_and_trailing_data() {
        let mut bytes = b"PTH0\x00\x00\x00\x00".to_vec();
        bytes.extend_from_slice(b"trailing");
        let (path, len) = LvPath::parse(&bytes).unwrap();
        assert_eq!(len, 8);
        assert!(path.is_empty());
    }

    #[test]
    fn parses_wide_pth1() {
        let mut body = b"rel ".to_vec();
        for c in ["", "a.vi"] {
            body.extend_from_slice(&(c.len() as u16).to_be_bytes());
            body.extend_from_slice(c.as_bytes());
        }
        let mut bytes = b"PTH1".to_vec();
        bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
        bytes.extend(body);
        let (path, len) = LvPath::parse(&bytes).unwrap();
        assert_eq!(len, bytes.len());
        assert_eq!(path.to_string(), "../a.vi");
    }

    #[test]
    fn rejects_malformed() {
        assert!(LvPath::parse(b"PTH9\x00\x00\x00\x00").is_err());
        assert!(LvPath::parse(b"PTH0\x00\x00\x00\x10\x00\x00").is_err());
        // Declared two components but only one present.
        let mut bytes = pth0(1, &["a.vi"]);
        bytes[11] = 2;
        assert!(LvPath::parse(&bytes).is_err());
        // Type out of range.
        assert!(LvPath::parse(&pth0(9, &["a.vi"])).is_err());
    }
}
