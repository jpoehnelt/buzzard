//! The packed 32-bit version number stored in `vers` and `LVSR` blocks.

use std::fmt;

use serde::Serialize;

/// Release stage encoded in a [`Version`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Unknown,
    Development,
    Alpha,
    Beta,
    Release,
}

impl Stage {
    fn from_bits(bits: u32) -> Self {
        match bits {
            1 => Stage::Development,
            2 => Stage::Alpha,
            3 => Stage::Beta,
            4 => Stage::Release,
            _ => Stage::Unknown,
        }
    }

    fn letter(self) -> char {
        match self {
            Stage::Unknown => '?',
            Stage::Development => 'd',
            Stage::Alpha => 'a',
            Stage::Beta => 'b',
            Stage::Release => 'f',
        }
    }
}

/// Version of the editor that saved a file.
///
/// The on-disk form is binary-coded decimal: `MMmp sbbb` in hex nibbles, with
/// the stage in the top three bits of the `s` nibble.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct Version {
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
    pub stage: Stage,
    pub build: u16,
}

impl Version {
    pub fn from_u32(raw: u32) -> Self {
        let nibble = |shift: u32| (raw >> shift) & 0xF;
        Self {
            major: (nibble(28) * 10 + nibble(24)) as u8,
            minor: nibble(20) as u8,
            patch: nibble(16) as u8,
            stage: Stage::from_bits((raw >> 13) & 0x7),
            build: (((raw >> 12) & 1) * 1000 + nibble(8) * 100 + nibble(4) * 10 + nibble(0)) as u16,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if self.stage != Stage::Release || self.build != 0 {
            write!(f, "{}{}", self.stage.letter(), self.build)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_release() {
        let v = Version::from_u32(0x2100_8000);
        assert_eq!((v.major, v.minor, v.patch, v.build), (21, 0, 0, 0));
        assert_eq!(v.stage, Stage::Release);
        assert_eq!(v.to_string(), "21.0.0");
    }

    #[test]
    fn decodes_patch_and_build() {
        let v = Version::from_u32(0x1401_8003);
        assert_eq!(v.to_string(), "14.0.1f3");
        let v = Version::from_u32(0x2312_6123);
        assert_eq!((v.major, v.minor, v.patch), (23, 1, 2));
        assert_eq!(v.stage, Stage::Beta);
        assert_eq!(v.build, 123);
        assert_eq!(v.to_string(), "23.1.2b123");
    }

    #[test]
    fn orders_by_release() {
        assert!(Version::from_u32(0x2100_8000) < Version::from_u32(0x2300_8000));
        assert!(Version::from_u32(0x2300_6000) < Version::from_u32(0x2300_8000));
    }
}
