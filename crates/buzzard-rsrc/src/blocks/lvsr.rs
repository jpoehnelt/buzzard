//! `LVSR`: the save record holding VI-level execution and editing flags.
//!
//! The block is a packed version number followed by an array of big-endian
//! 32-bit flag words. Older files have fewer words, so every accessor returns
//! `None` when its word is absent.

use serde::Serialize;

use crate::error::Result;
use crate::reader::Reader;
use crate::version::Version;

type Flag = (usize, u32);

const SUSPEND_WHEN_CALLED: Flag = (0, 0x0000_1000);
const LOCKED: Flag = (0, 0x0000_2000);
const RUN_ON_OPEN: Flag = (0, 0x0000_4000);
const REENTRANT: Flag = (0, 0x0100_0000);
const SHARED_CLONES: Flag = (0, 0x0000_0080);
const PREALLOCATE_CLONES: Flag = (0, 0x0000_0020);
const SAVED_FOR_PREVIOUS: Flag = (1, 0x0000_0004);
const SEPARATE_CODE: Flag = (1, 0x0000_0400);
const INLINE: Flag = (1, 0x0002_0000);
const CLEAR_INDICATORS: Flag = (1, 0x0100_0000);
const AUTO_ERROR_HANDLING: Flag = (1, 0x2000_0000);
const BREAKPOINTS_SET: Flag = (5, 0x2000_0000);
const DEBUGGABLE: Flag = (5, 0x4000_0200);
const BREAKPOINT_COUNT_WORD: usize = 28;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lvsr {
    pub version: Version,
    words: Vec<u32>,
}

/// How a VI behaves when called from several places at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reentrancy {
    NonReentrant,
    SharedClones,
    PreallocatedClones,
}

/// Decoded view of the flags most relevant to static analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViFlags {
    pub separate_compiled_code: Option<bool>,
    pub auto_error_handling: Option<bool>,
    pub debuggable: Option<bool>,
    pub has_breakpoints: Option<bool>,
    pub breakpoint_count: Option<u32>,
    pub suspend_when_called: Option<bool>,
    pub run_on_open: Option<bool>,
    pub locked: Option<bool>,
    pub clear_indicators: Option<bool>,
    pub saved_for_previous: Option<bool>,
    pub reentrancy: Option<Reentrancy>,
    pub inline: Option<bool>,
}

impl Lvsr {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data, "LVSR");
        let version = Version::from_u32(r.u32()?);
        let mut words = Vec::with_capacity(r.remaining() / 4);
        while r.remaining() >= 4 {
            words.push(r.u32()?);
        }
        Ok(Self { version, words })
    }

    /// Raw flag words following the version number.
    pub fn words(&self) -> &[u32] {
        &self.words
    }

    fn flag(&self, (word, mask): Flag) -> Option<bool> {
        self.words.get(word).map(|w| w & mask != 0)
    }

    pub fn reentrancy(&self) -> Option<Reentrancy> {
        let reentrant = self.flag(REENTRANT)? || self.flag(PREALLOCATE_CLONES)?;
        Some(if !reentrant {
            Reentrancy::NonReentrant
        } else if self.flag(SHARED_CLONES)? {
            Reentrancy::SharedClones
        } else {
            Reentrancy::PreallocatedClones
        })
    }

    pub fn flags(&self) -> ViFlags {
        ViFlags {
            separate_compiled_code: self.flag(SEPARATE_CODE),
            auto_error_handling: self.flag(AUTO_ERROR_HANDLING),
            debuggable: self.flag(DEBUGGABLE),
            has_breakpoints: self.flag(BREAKPOINTS_SET),
            breakpoint_count: self.words.get(BREAKPOINT_COUNT_WORD).copied(),
            suspend_when_called: self.flag(SUSPEND_WHEN_CALLED),
            run_on_open: self.flag(RUN_ON_OPEN),
            locked: self.flag(LOCKED),
            clear_indicators: self.flag(CLEAR_INDICATORS),
            saved_for_previous: self.flag(SAVED_FOR_PREVIOUS),
            reentrancy: self.reentrancy(),
            inline: self.flag(INLINE),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(words: &[u32]) -> Vec<u8> {
        let mut out = 0x2100_8000u32.to_be_bytes().to_vec();
        for w in words {
            out.extend_from_slice(&w.to_be_bytes());
        }
        out
    }

    #[test]
    fn decodes_flag_words() {
        let lvsr = Lvsr::parse(&block(&[0x0000_1000, 0x2000_0400, 0, 0, 0, 0x4000_0200])).unwrap();
        let flags = lvsr.flags();
        assert_eq!(flags.suspend_when_called, Some(true));
        assert_eq!(flags.locked, Some(false));
        assert_eq!(flags.separate_compiled_code, Some(true));
        assert_eq!(flags.auto_error_handling, Some(true));
        assert_eq!(flags.debuggable, Some(true));
        assert_eq!(flags.has_breakpoints, Some(false));
        assert_eq!(flags.breakpoint_count, None);
        assert_eq!(flags.reentrancy, Some(Reentrancy::NonReentrant));
    }

    #[test]
    fn missing_words_are_unknown() {
        let lvsr = Lvsr::parse(&block(&[0])).unwrap();
        let flags = lvsr.flags();
        assert_eq!(flags.locked, Some(false));
        assert_eq!(flags.separate_compiled_code, None);
        assert_eq!(flags.debuggable, None);
    }

    #[test]
    fn decodes_reentrancy() {
        let of = |w0| Lvsr::parse(&block(&[w0])).unwrap().reentrancy();
        assert_eq!(of(0), Some(Reentrancy::NonReentrant));
        assert_eq!(of(0x0000_0080), Some(Reentrancy::NonReentrant));
        assert_eq!(of(0x0100_0080), Some(Reentrancy::SharedClones));
        assert_eq!(of(0x0100_0000), Some(Reentrancy::PreallocatedClones));
        assert_eq!(of(0x0000_0020), Some(Reentrancy::PreallocatedClones));
    }
}
