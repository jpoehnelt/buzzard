//! The fixed 32-byte RSRC file header.
//!
//! The header appears twice in every file: once at offset 0 and once at the
//! start of the info (directory) section. All integers are big-endian.

use std::io::Cursor;

use binrw::{BinRead, BinWrite};

use crate::error::{Result, RsrcError};
use crate::fourcc::FourCc;

/// Size of the serialized header in bytes.
pub const HEADER_SIZE: usize = 32;
/// The only container format version known to exist in real files.
pub const FORMAT_VERSION: u16 = 3;

const SIGNATURE: &[u8; 6] = b"RSRC\r\n";

/// Well-known values of [`RsrcHeader::file_type`].
pub mod file_type {
    use crate::fourcc::FourCc;
    /// A VI (`.vi`, `.vit`, `.vim`).
    pub const VI: FourCc = FourCc::new(b"LVIN");
    /// A custom control or typedef (`.ctl`, `.ctt`).
    pub const CONTROL: FourCc = FourCc::new(b"LVCC");
    /// An LLB archive (`.llb`).
    pub const LIBRARY: FourCc = FourCc::new(b"LVAR");
}

/// The header found at offset 0 and again at the start of the info section.
#[derive(BinRead, BinWrite, Debug, Clone, PartialEq, Eq)]
#[brw(big, magic = b"RSRC\r\n")]
pub struct RsrcHeader {
    /// Container format version, always [`FORMAT_VERSION`].
    pub format_version: u16,
    /// What kind of file this is, such as `LVIN` for a VI.
    #[br(map = FourCc)]
    #[bw(map = |v: &FourCc| v.0)]
    pub file_type: FourCc,
    /// Creator code, normally `LBVW`.
    #[br(map = FourCc)]
    #[bw(map = |v: &FourCc| v.0)]
    pub creator: FourCc,
    /// Offset of the info section (the second header and the resource
    /// directory).
    pub info_offset: u32,
    /// Size of the info section in bytes.
    pub info_size: u32,
    /// Offset of the data section holding the resource payloads.
    pub data_offset: u32,
    /// Size of the data section in bytes.
    pub data_size: u32,
}

impl RsrcHeader {
    /// Parse a header from the first [`HEADER_SIZE`] bytes of `bytes`.
    ///
    /// # Errors
    ///
    /// Returns [`RsrcError::TooSmall`] if `bytes` is shorter than
    /// [`HEADER_SIZE`], [`RsrcError::BadSignature`] if it does not start with
    /// `RSRC\r\n`, and [`RsrcError::UnsupportedVersion`] if the format version
    /// is not [`FORMAT_VERSION`].
    #[expect(
        clippy::missing_panics_doc,
        reason = "the `expect` runs after the length and signature checks, so the fixed-size read cannot fail"
    )]
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_SIZE {
            return Err(RsrcError::TooSmall(bytes.len()));
        }
        if &bytes[..SIGNATURE.len()] != SIGNATURE {
            return Err(RsrcError::BadSignature);
        }
        let header = Self::read(&mut Cursor::new(&bytes[..HEADER_SIZE]))
            .expect("fixed-size header read cannot fail after length and magic checks");
        if header.format_version != FORMAT_VERSION {
            return Err(RsrcError::UnsupportedVersion(header.format_version));
        }
        Ok(header)
    }

    /// Check that both sections lie within a file of `file_len` bytes.
    ///
    /// # Errors
    ///
    /// Returns [`RsrcError::OutOfBounds`] if either section ends past
    /// `file_len`.
    pub fn validate(&self, file_len: usize) -> Result<()> {
        check_range("info section", self.info_offset, self.info_size, file_len)?;
        check_range("data section", self.data_offset, self.data_size, file_len)
    }

    /// Serialize the header to its 32-byte on-disk form.
    #[expect(
        clippy::missing_panics_doc,
        reason = "writing a fixed-size header into a `Vec` cannot fail and always yields `HEADER_SIZE` bytes"
    )]
    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut cursor = Cursor::new(Vec::with_capacity(HEADER_SIZE));
        self.write(&mut cursor)
            .expect("writing a header to memory cannot fail");
        cursor
            .into_inner()
            .try_into()
            .expect("header serializes to exactly HEADER_SIZE bytes")
    }
}

fn check_range(what: &'static str, offset: u32, len: u32, size: usize) -> Result<()> {
    if u64::from(offset) + u64::from(len) > size as u64 {
        return Err(RsrcError::OutOfBounds {
            what,
            offset: offset.into(),
            len: len.into(),
            size,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RsrcHeader {
        RsrcHeader {
            format_version: FORMAT_VERSION,
            file_type: file_type::VI,
            creator: FourCc::new(b"LBVW"),
            info_offset: 0x0e30,
            info_size: 0x0531,
            data_offset: 0x20,
            data_size: 0x0e10,
        }
    }

    #[test]
    fn parses_real_header_bytes() {
        // First 32 bytes of a real VI.
        let bytes: [u8; 32] = [
            0x52, 0x53, 0x52, 0x43, 0x0d, 0x0a, 0x00, 0x03, 0x4c, 0x56, 0x49, 0x4e, 0x4c, 0x42,
            0x56, 0x57, 0x00, 0x00, 0x0e, 0x30, 0x00, 0x00, 0x05, 0x31, 0x00, 0x00, 0x00, 0x20,
            0x00, 0x00, 0x0e, 0x10,
        ];
        let header = RsrcHeader::parse(&bytes).unwrap();
        assert_eq!(header, sample());
        assert_eq!(header.to_bytes(), bytes);
    }

    #[test]
    fn rejects_short_input() {
        assert!(matches!(
            RsrcHeader::parse(b"RSRC\r\n"),
            Err(RsrcError::TooSmall(6))
        ));
    }

    #[test]
    fn rejects_bad_signature() {
        let mut bytes = sample().to_bytes();
        bytes[0] = b'X';
        assert!(matches!(
            RsrcHeader::parse(&bytes),
            Err(RsrcError::BadSignature)
        ));
        // A text-mode transfer that changed the CR to an LF is rejected too.
        let mut bytes = sample().to_bytes();
        bytes[4] = b'\n';
        assert!(matches!(
            RsrcHeader::parse(&bytes),
            Err(RsrcError::BadSignature)
        ));
    }

    #[test]
    fn rejects_unknown_format_version() {
        let mut header = sample();
        header.format_version = 4;
        assert!(matches!(
            RsrcHeader::parse(&header.to_bytes()),
            Err(RsrcError::UnsupportedVersion(4))
        ));
    }

    #[test]
    fn validates_section_bounds() {
        let header = sample();
        assert!(header.validate(0x0e30 + 0x0531).is_ok());
        assert!(matches!(
            header.validate(0x0e30 + 0x0530),
            Err(RsrcError::OutOfBounds {
                what: "info section",
                ..
            })
        ));
    }
}
