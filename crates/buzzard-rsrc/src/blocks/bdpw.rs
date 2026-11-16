//! `BDPW`: block diagram password hashes.

use crate::error::{malformed, Result};

/// MD5 of the empty string, stored when no password is set.
const EMPTY_PASSWORD_MD5: [u8; 16] = [
    0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04, 0xe9, 0x80, 0x09, 0x98, 0xec, 0xf8, 0x42, 0x7e,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bdpw {
    /// MD5 of the password; the remaining hashes in the block are salted
    /// integrity hashes and are not interpreted.
    pub password_md5: [u8; 16],
}

impl Bdpw {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let hash = data
            .get(..16)
            .ok_or_else(|| malformed("BDPW", format!("{} bytes, need at least 16", data.len())))?;
        Ok(Self {
            password_md5: hash.try_into().expect("slice is 16 bytes"),
        })
    }

    pub fn has_password(&self) -> bool {
        self.password_md5 != EMPTY_PASSWORD_MD5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_password_is_not_protected() {
        let mut data = EMPTY_PASSWORD_MD5.to_vec();
        data.extend_from_slice(&[0xAA; 32]);
        assert!(!Bdpw::parse(&data).unwrap().has_password());
    }

    #[test]
    fn other_hash_is_protected() {
        assert!(Bdpw::parse(&[0x11; 48]).unwrap().has_password());
    }

    #[test]
    fn rejects_short_block() {
        assert!(Bdpw::parse(&[0; 8]).is_err());
    }
}
