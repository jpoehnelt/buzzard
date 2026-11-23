//! Four-character codes, which name file types and resource block types.

use std::fmt;

use serde::{Serialize, Serializer};

/// Four-character code identifying a file type or resource block type.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FourCc(pub [u8; 4]);

impl FourCc {
    /// Make a code from its four bytes. The function works in `const` items.
    pub const fn new(code: &[u8; 4]) -> Self {
        Self(*code)
    }

    /// The four bytes of the code, in file order.
    pub fn as_bytes(&self) -> &[u8; 4] {
        &self.0
    }
}

impl From<[u8; 4]> for FourCc {
    fn from(value: [u8; 4]) -> Self {
        Self(value)
    }
}

impl From<&[u8; 4]> for FourCc {
    fn from(value: &[u8; 4]) -> Self {
        Self(*value)
    }
}

impl fmt::Display for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &b in &self.0 {
            if b.is_ascii_graphic() || b == b' ' {
                write!(f, "{}", b as char)?;
            } else {
                write!(f, "\\x{b:02x}")?;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FourCc({self})")
    }
}

impl Serialize for FourCc {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}
