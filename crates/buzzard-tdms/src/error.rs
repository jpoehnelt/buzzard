//! The error type, and the anomalies a file can be opened in spite of.

use std::fmt;
use std::io;

use crate::types::DataType;

/// A specialized result for this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Why a file could not be opened or a read could not be done.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The operating system reported an error, or the file is shorter than
    /// it was when it was opened.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// The file does not start with a TDMS segment.
    #[error("not a TDMS file: the first four bytes are not the segment tag")]
    NotTdms,
    /// The file contradicts the format.
    #[error("corrupt file at byte {offset} (segment {segment}): {reason}")]
    Corrupt {
        /// Byte offset in the file (or the index file) of the bad field.
        offset: u64,
        /// Zero-based number of the segment.
        segment: u64,
        /// What is wrong.
        reason: &'static str,
    },
    /// Raw data contradicts the format; found while reading values.
    #[error("corrupt raw data at byte {offset}: {reason}")]
    CorruptData {
        /// Byte offset in the file of the channel's data in the chunk.
        offset: u64,
        /// What is wrong.
        reason: &'static str,
    },
    /// The file uses a part of the format that this crate does not read.
    #[error("unsupported at byte {offset} (segment {segment}): {what}")]
    Unsupported {
        /// Byte offset in the file of the field.
        offset: u64,
        /// Zero-based number of the segment.
        segment: u64,
        /// What is not supported.
        what: String,
    },
    /// The channel does not store the requested type.
    #[error("the channel stores {stored}, which cannot be read as {requested}")]
    TypeMismatch {
        /// The channel's type.
        stored: DataType,
        /// The requested type.
        requested: &'static str,
    },
    /// The requested values are not all in the channel.
    #[error("values {start}..{end} were requested from a channel of {len}")]
    OutOfRange {
        /// First requested value.
        start: u64,
        /// One past the last requested value.
        end: u64,
        /// The channel's length.
        len: u64,
    },
    /// A buffer of this size could not be allocated.
    #[error("cannot allocate {bytes} bytes")]
    Alloc {
        /// The size asked for.
        bytes: u64,
    },
    /// The file changes its layout so often that the index would exceed
    /// [`crate::OpenOptions::max_layout_entries`].
    #[error("the index needs more than {limit} layout entries")]
    IndexTooLarge {
        /// The limit that was exceeded.
        limit: u64,
    },
    /// The file was opened with [`crate::OpenOptions::strict`] and has an
    /// anomaly.
    #[error("{0}")]
    Anomaly(Anomaly),
    /// A named group or channel, or a scaler, does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// The channel's scaling cannot be applied.
    #[error("unsupported scaling: {0}")]
    Scaling(String),
    /// Writing an export failed.
    #[error("export failed: {0}")]
    Export(String),
}

/// Something wrong with a file that did not stop it from being opened.
///
/// The usual cause is a writer that crashed or a copy that was cut short:
/// the last segment is then incomplete, and everything before it is intact.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Anomaly {
    /// Fewer than 28 bytes follow the last segment.
    TruncatedLeadIn {
        /// Where the incomplete lead-in starts.
        offset: u64,
        /// How many bytes of it are present.
        bytes: u64,
    },
    /// The last segment's length field was never written (all bits set), so
    /// the segment is taken to extend to the end of the file.
    UnfinishedSegment {
        /// Zero-based number of the segment.
        segment: u64,
        /// Where the segment starts.
        offset: u64,
    },
    /// A segment claims to end beyond the end of the file.
    TruncatedSegment {
        /// Zero-based number of the segment.
        segment: u64,
        /// Where the segment starts.
        offset: u64,
        /// How many bytes are missing.
        missing: u64,
    },
    /// The file ends inside a segment's metadata; the segment is ignored.
    TruncatedMetadata {
        /// Zero-based number of the segment.
        segment: u64,
        /// Where the segment starts.
        offset: u64,
    },
    /// A segment's raw data is not a whole number of chunks.
    PartialChunk {
        /// Zero-based number of the segment.
        segment: u64,
        /// Where the incomplete chunk starts.
        offset: u64,
        /// Bytes present of the incomplete chunk.
        bytes: u64,
        /// Whether values were recovered from it. Otherwise it was skipped.
        recovered: bool,
    },
    /// A segment has raw data bytes that no channel accounts for.
    UnindexedData {
        /// Zero-based number of the segment.
        segment: u64,
        /// Where the bytes start.
        offset: u64,
        /// How many bytes.
        bytes: u64,
    },
    /// A segment's version number is neither 4712 nor 4713.
    UnknownVersion {
        /// Zero-based number of the segment.
        segment: u64,
        /// The number found.
        version: u32,
    },
    /// A name or a text property is not valid UTF-8; the bad bytes were
    /// replaced with U+FFFD.
    InvalidUtf8 {
        /// Zero-based number of the segment.
        segment: u64,
        /// Where the text starts.
        offset: u64,
    },
}

impl Anomaly {
    /// A stable identifier for the kind of anomaly, as the command line's
    /// JSON output names it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::TruncatedLeadIn { .. } => "truncated_lead_in",
            Self::UnfinishedSegment { .. } => "unfinished_segment",
            Self::TruncatedSegment { .. } => "truncated_segment",
            Self::TruncatedMetadata { .. } => "truncated_metadata",
            Self::PartialChunk { .. } => "partial_chunk",
            Self::UnindexedData { .. } => "unindexed_data",
            Self::UnknownVersion { .. } => "unknown_version",
            Self::InvalidUtf8 { .. } => "invalid_utf8",
        }
    }
}

impl fmt::Display for Anomaly {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::TruncatedLeadIn { offset, bytes } => write!(
                f,
                "the file ends {bytes} bytes into a segment header at byte {offset}"
            ),
            Self::UnfinishedSegment { segment, offset } => write!(
                f,
                "segment {segment} at byte {offset} was never finished; it is read to the end of the file"
            ),
            Self::TruncatedSegment {
                segment,
                offset,
                missing,
            } => write!(
                f,
                "segment {segment} at byte {offset} is truncated: {missing} bytes are missing"
            ),
            Self::TruncatedMetadata { segment, offset } => write!(
                f,
                "the file ends inside the metadata of segment {segment} at byte {offset}; the segment is ignored"
            ),
            Self::PartialChunk {
                segment,
                offset,
                bytes,
                recovered,
            } => write!(
                f,
                "segment {segment} ends with an incomplete chunk of {bytes} bytes at byte {offset}; {}",
                if recovered {
                    "the values present were kept"
                } else {
                    "it was skipped"
                }
            ),
            Self::UnindexedData {
                segment,
                offset,
                bytes,
            } => write!(
                f,
                "segment {segment} has {bytes} bytes of raw data at byte {offset} that no channel describes"
            ),
            Self::UnknownVersion { segment, version } => {
                write!(f, "segment {segment} has the unknown version number {version}")
            }
            Self::InvalidUtf8 { segment, offset } => write!(
                f,
                "text at byte {offset} in segment {segment} is not valid UTF-8"
            ),
        }
    }
}
