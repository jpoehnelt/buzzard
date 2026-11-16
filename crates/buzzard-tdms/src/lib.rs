//! A fast, safe, read-only reader for TDMS measurement files.
//!
//! TDMS is the format data-acquisition software logs test data in: a file
//! of groups and channels, written as a sequence of segments that each hold
//! some metadata and a block of raw values. The format is described at
//! <https://www.ni.com/en/support/documentation/supplemental/07/tdms-file-format-internal-structure.html>.
//!
//! Opening a file reads the lead-in and the metadata of every segment and
//! none of the raw data, so it is quick for a file of many gigabytes.
//! Values are read when a channel is, for any range, in parallel.
//!
//! ```
//! use buzzard_tdms::build::{FileBuilder, Object, Values};
//! use buzzard_tdms::TdmsFile;
//!
//! let mut builder = FileBuilder::new();
//! builder
//!     .segment()
//!     .object(Object::channel("run", "pressure").data(Values::F64(vec![1.0, 2.5, 4.0])));
//! let file = TdmsFile::from_bytes(builder.build().bytes)?;
//!
//! let channel = file.channel("run", "pressure").expect("the channel exists");
//! assert_eq!(channel.len(), 3);
//! assert_eq!(channel.read::<f64>(1..3)?, [2.5, 4.0]);
//! # Ok::<(), buzzard_tdms::Error>(())
//! ```
//!
//! The crate has no `unsafe` code. A file is read with positional reads
//! into buffers the reader owns ([`ReadAt`]); it is never memory-mapped, so
//! a file that is truncated while open gives an error, not a fault.
//! Hostile input gives an [`Error`], never a panic, and never an allocation
//! larger than a small multiple of the file.
//!
//! See the crate's `README.md` for what is supported and what is not.

mod error;
mod file;
mod index;
mod io;
mod read;
mod scaling;
mod types;

pub mod build;
pub mod export;

pub use error::{Anomaly, Error, Result};
pub use file::{Channel, Chunks, DaqmxScaler, Group, OpenOptions, TdmsFile, Waveform};
pub use index::IndexSource;
pub use io::{FileSource, ReadAt};
pub use scaling::Scaling;
pub use types::{
    Column, Complex32, Complex64, DataType, Property, PropertyValue, Sample, Timestamp,
    EPOCH_OFFSET_SECONDS,
};

/// Compiles the examples of the crate's `README.md` as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
