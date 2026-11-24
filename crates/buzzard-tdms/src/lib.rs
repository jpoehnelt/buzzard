//! A fast, safe, read-only reader for TDMS measurement files.
//!
//! TDMS is the format that data-acquisition software logs test data in. A
//! file holds groups and channels, and is written as a sequence of
//! segments. Each segment holds some metadata and a block of raw values.
//! The format is described at
//! <https://www.ni.com/en/support/documentation/supplemental/07/tdms-file-format-internal-structure.html>.
//!
//! Opening a file reads the lead-in and the metadata of every segment and
//! none of the raw data, so it is quick for a file of many gigabytes. Raw
//! values are read when a channel is read. A read takes any range of a
//! channel, and a large read runs on several threads.
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
//! The crate has no `unsafe` code. The reader uses positional reads into
//! buffers that it owns ([`ReadAt`]) and never memory-maps a file, so a
//! file that is truncated while it is open gives an error and cannot cause
//! a fault. Hostile input gives an [`Error`], never a panic, and never an
//! allocation larger than a small multiple of the file.
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
pub mod info;

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
