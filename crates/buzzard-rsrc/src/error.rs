//! The error type shared by every parser in this crate.

use thiserror::Error;

/// Errors produced while parsing an RSRC container or one of its blocks.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RsrcError {
    /// The input is shorter than the 32-byte header. Holds the input's length.
    #[error("file is too small to be an RSRC container ({0} bytes)")]
    TooSmall(usize),
    /// The file is larger than
    /// [`MAX_FILE_BYTES`](crate::metadata::MAX_FILE_BYTES). Holds the file's
    /// size and that limit.
    #[error("file is {0} bytes, over the limit of {1}")]
    TooLarge(u64, u64),
    /// The input does not start with `RSRC\r\n`.
    #[error("missing RSRC signature")]
    BadSignature,
    /// The header names a container format other than
    /// [`FORMAT_VERSION`](crate::header::FORMAT_VERSION).
    #[error("unsupported RSRC format version {0}")]
    UnsupportedVersion(u16),
    /// The copy of the header at the start of the info section is missing
    /// or differs from the one at offset 0.
    #[error("the two copies of the RSRC header disagree")]
    HeaderMismatch,
    /// An offset or length points outside the buffer being read.
    #[error("{what} at offset {offset} (+{len} bytes) is outside the {size}-byte buffer")]
    OutOfBounds {
        /// What was being read, such as `type list` or `LIBN`.
        what: &'static str,
        /// Where the read started, counted from the start of the buffer.
        offset: u64,
        /// Bytes requested, or the entry count for a directory list.
        len: u64,
        /// Size of the buffer in bytes.
        size: usize,
    },
    /// A block's contents do not match its expected layout.
    #[error("malformed `{block}` block: {reason}")]
    MalformedBlock {
        /// The block or structure being parsed, such as `LIBN` or `heap`.
        block: &'static str,
        /// What was wrong with it.
        reason: String,
    },
    /// The zlib stream of a compressed block is corrupt.
    #[error("zlib decompression failed: {0}")]
    Decompress(#[source] std::io::Error),
    /// Reading the file from disk failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Result type of this crate, with [`RsrcError`] as the default error.
pub type Result<T, E = RsrcError> = std::result::Result<T, E>;

pub(crate) fn malformed(block: &'static str, reason: impl Into<String>) -> RsrcError {
    RsrcError::MalformedBlock {
        block,
        reason: reason.into(),
    }
}
