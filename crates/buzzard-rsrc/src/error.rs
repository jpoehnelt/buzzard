use thiserror::Error;

/// Errors produced while parsing an RSRC container or one of its blocks.
#[derive(Debug, Error)]
pub enum RsrcError {
    #[error("file is too small to be an RSRC container ({0} bytes)")]
    TooSmall(usize),
    #[error("file is {0} bytes, over the limit of {1}")]
    TooLarge(u64, u64),
    #[error("missing RSRC signature")]
    BadSignature,
    #[error("unsupported RSRC format version {0}")]
    UnsupportedVersion(u16),
    #[error("the two copies of the RSRC header disagree")]
    HeaderMismatch,
    #[error("{what} at offset {offset} (+{len} bytes) is outside the {size}-byte buffer")]
    OutOfBounds {
        what: &'static str,
        offset: u64,
        len: u64,
        size: usize,
    },
    #[error("malformed `{block}` block: {reason}")]
    MalformedBlock { block: &'static str, reason: String },
    #[error("zlib decompression failed: {0}")]
    Decompress(#[source] std::io::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = RsrcError> = std::result::Result<T, E>;

pub(crate) fn malformed(block: &'static str, reason: impl Into<String>) -> RsrcError {
    RsrcError::MalformedBlock {
        block,
        reason: reason.into(),
    }
}
