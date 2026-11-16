//! `vers`: the version of the editor that saved the file.

use serde::Serialize;

use crate::error::Result;
use crate::reader::{decode_text, Reader};
use crate::version::Version;

/// Decoded `vers` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Vers {
    /// The packed version number.
    pub version: Version,
    /// Language code stored after the version number.
    pub language: u16,
    /// Human-readable version text, e.g. `21.0`.
    pub text: String,
    /// Pascal string after the version text; empty when the block omits it.
    pub name: String,
}

impl Vers {
    /// Parse a `vers` block.
    ///
    /// # Errors
    ///
    /// If the version number, language or version text is truncated, or a
    /// name string is present but runs past the end of the block.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data, "vers");
        let version = Version::from_u32(r.u32()?);
        let language = r.u16()?;
        let text = decode_text(r.pstr()?);
        // Some writers omit the trailing name string.
        let name = if r.remaining() > 0 {
            decode_text(r.pstr()?)
        } else {
            String::new()
        };
        Ok(Self {
            version,
            language,
            text,
            name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_block() {
        let data = [
            0x21, 0x00, 0x80, 0x00, 0x00, 0x00, 0x04, b'2', b'1', b'.', b'0', 0x00,
        ];
        let vers = Vers::parse(&data).unwrap();
        assert_eq!(vers.version.to_string(), "21.0.0");
        assert_eq!(vers.text, "21.0");
        assert_eq!(vers.name, "");
    }

    #[test]
    fn rejects_truncated_block() {
        assert!(Vers::parse(&[0x21, 0x00]).is_err());
    }
}
