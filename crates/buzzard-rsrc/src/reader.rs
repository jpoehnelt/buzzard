//! Minimal bounds-checked big-endian reader used by the block parsers.

use crate::error::{Result, RsrcError};

#[derive(Debug, Clone)]
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    what: &'static str,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8], what: &'static str) -> Self {
        Self {
            bytes,
            pos: 0,
            what,
        }
    }

    pub fn at(bytes: &'a [u8], pos: usize, what: &'static str) -> Self {
        Self { bytes, pos, what }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    pub fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(len).filter(|&e| e <= self.bytes.len());
        match end {
            Some(end) => {
                let out = &self.bytes[self.pos..end];
                self.pos = end;
                Ok(out)
            }
            None => Err(RsrcError::OutOfBounds {
                what: self.what,
                offset: self.pos as u64,
                len: len as u64,
                size: self.bytes.len(),
            }),
        }
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn fourcc(&mut self) -> Result<[u8; 4]> {
        let b = self.take(4)?;
        Ok([b[0], b[1], b[2], b[3]])
    }

    /// Pascal string with a one-byte length prefix.
    pub fn pstr(&mut self) -> Result<&'a [u8]> {
        let len = self.u8()? as usize;
        self.take(len)
    }

    /// Pascal string with a two-byte length prefix.
    pub fn pstr16(&mut self) -> Result<&'a [u8]> {
        let len = self.u16()? as usize;
        self.take(len)
    }
}

/// Decode text stored in the platform code page. Non-UTF-8 bytes are mapped
/// through Latin-1 so the result is always lossless for ASCII and stable.
pub(crate) fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}
