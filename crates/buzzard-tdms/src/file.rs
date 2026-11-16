//! The public view of an opened file: groups, channels, properties, reads.

use std::fmt;
use std::fs::File;
use std::marker::PhantomData;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use bytemuck::Zeroable;

use crate::error::{Anomaly, Error, Result};
use crate::index::{self, Index, IndexSource, ObjKind, Object};
use crate::io::{FileSource, ReadAt};
use crate::read::{self, Request, Span};
use crate::scaling::Scaling;
use crate::types::{Column, DataType, Property, PropertyValue, Sample, Timestamp};

/// How to open a file.
#[derive(Clone, Debug)]
pub struct OpenOptions {
    pub(crate) strict: bool,
    pub(crate) index_file: bool,
    pub(crate) max_layout_entries: u64,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            strict: false,
            index_file: true,
            max_layout_entries: 1 << 24,
        }
    }
}

impl OpenOptions {
    /// The defaults: recover what a truncated file still holds, and use the
    /// `.tdms_index` file when it agrees with the data file.
    pub fn new() -> Self {
        Self::default()
    }

    /// When set, any [`Anomaly`] makes opening fail with
    /// [`Error::Anomaly`].
    #[must_use]
    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Whether to read the metadata from the `.tdms_index` file beside the
    /// data file when there is one. It is checked against the data file
    /// first and ignored when it does not match.
    #[must_use]
    pub fn index_file(mut self, use_it: bool) -> Self {
        self.index_file = use_it;
        self
    }

    /// The largest number of channel positions the index may hold over all
    /// distinct chunk layouts (16,777,216 by default, about 1 GiB). Only a
    /// file that changes which channels it writes, again and again in ways
    /// it never repeats, comes near it; opening such a file fails with
    /// [`Error::IndexTooLarge`] and does not exhaust memory.
    #[must_use]
    pub fn max_layout_entries(mut self, limit: u64) -> Self {
        self.max_layout_entries = limit;
        self
    }

    /// Opens the file at `path`.
    ///
    /// # Errors
    ///
    /// When the file cannot be read, is not a TDMS file, contradicts the
    /// format, or uses a part of the format that is not supported.
    pub fn open(&self, path: impl AsRef<Path>) -> Result<TdmsFile> {
        let path = path.as_ref();
        let data = Arc::new(FileSource::new(File::open(path)?)?);
        let index_file = if self.index_file {
            let mut name = path.as_os_str().to_owned();
            name.push("_index");
            File::open(PathBuf::from(name))
                .and_then(FileSource::new)
                .ok()
        } else {
            None
        };
        let index_file: Option<&dyn ReadAt> = match &index_file {
            Some(file) => Some(file),
            None => None,
        };
        self.open_source(data, index_file)
    }

    /// Opens a file from any source of bytes, with the contents of its
    /// index file when there is one.
    ///
    /// # Errors
    ///
    /// As for [`Self::open`].
    pub fn open_source(
        &self,
        data: Arc<dyn ReadAt>,
        index_file: Option<&dyn ReadAt>,
    ) -> Result<TdmsFile> {
        let index = index::build(data.as_ref(), index_file, self)?;
        Ok(TdmsFile::new(data, index))
    }
}

struct GroupInfo {
    name: String,
    object: Option<u32>,
    channels: Vec<u32>,
}

/// An opened TDMS file: its index in memory, its data still on disk.
pub struct TdmsFile {
    io: Arc<dyn ReadAt>,
    index: Index,
    root: Option<u32>,
    groups: Vec<GroupInfo>,
    spans: Vec<OnceLock<Vec<Span>>>,
}

impl fmt::Debug for TdmsFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TdmsFile")
            .field("size", &self.io.size())
            .field("segments", &self.index.segments)
            .field("groups", &self.groups.len())
            .finish_non_exhaustive()
    }
}

impl TdmsFile {
    fn new(io: Arc<dyn ReadAt>, index: Index) -> Self {
        let mut root = None;
        let mut groups: Vec<GroupInfo> = Vec::new();
        let mut by_name = std::collections::HashMap::new();
        for (id, object) in index.objects.iter().enumerate() {
            // The index refuses more objects than fit.
            let id = u32::try_from(id).unwrap_or(u32::MAX);
            let (group, channel) = match &object.kind {
                ObjKind::Root => {
                    root = Some(id);
                    continue;
                }
                ObjKind::Group(group) => (group, false),
                ObjKind::Channel(group, _) => (group, true),
            };
            let at = *by_name.entry(group.clone()).or_insert_with(|| {
                groups.push(GroupInfo {
                    name: group.clone(),
                    object: None,
                    channels: Vec::new(),
                });
                groups.len() - 1
            });
            if channel {
                groups[at].channels.push(id);
            } else {
                groups[at].object = Some(id);
            }
        }
        let spans = index.objects.iter().map(|_| OnceLock::new()).collect();
        Self {
            io,
            index,
            root,
            groups,
            spans,
        }
    }

    /// Opens the file at `path` with the default [`OpenOptions`].
    ///
    /// Only lead-ins and metadata are read, so this is quick for a large
    /// file; raw data is read when a channel is.
    ///
    /// # Errors
    ///
    /// As for [`OpenOptions::open`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        OpenOptions::new().open(path)
    }

    /// Opens a file held in memory.
    ///
    /// # Errors
    ///
    /// As for [`OpenOptions::open`].
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        OpenOptions::new().open_source(Arc::new(bytes), None)
    }

    /// The properties of the file itself.
    pub fn properties(&self) -> &[Property] {
        self.root
            .map_or(&[], |id| &self.index.objects[id as usize].props)
    }

    /// The groups, in the order the file first mentions them.
    pub fn groups(&self) -> impl ExactSizeIterator<Item = Group<'_>> + '_ {
        (0..self.groups.len()).map(move |at| Group { file: self, at })
    }

    /// The group with this name.
    pub fn group(&self, name: &str) -> Option<Group<'_>> {
        self.groups().find(|g| g.name() == name)
    }

    /// Every channel of every group, in file order within each group.
    pub fn channels(&self) -> impl Iterator<Item = Channel<'_>> + '_ {
        self.groups().flat_map(|g| g.channels())
    }

    /// The channel with this group and name.
    pub fn channel(&self, group: &str, name: &str) -> Option<Channel<'_>> {
        let id = *self
            .index
            .by_path
            .get(&index::object_path(group, Some(name)))?;
        Some(Channel { file: self, id })
    }

    /// Number of segments in the file, including a truncated last one.
    pub fn segment_count(&self) -> u64 {
        self.index.segments
    }

    /// What is wrong with the file, when it could be opened all the same.
    pub fn anomalies(&self) -> &[Anomaly] {
        &self.index.anomalies
    }

    /// Where the metadata was read from.
    pub fn index_source(&self) -> IndexSource {
        self.index.source
    }

    /// The version numbers the segments carry (4712 or 4713), in order of
    /// first appearance; at most eight are kept.
    pub fn versions(&self) -> &[u32] {
        &self.index.versions
    }

    /// Size of the data file in bytes when it was opened.
    pub fn size(&self) -> u64 {
        self.io.size()
    }

    /// How many runs of identically laid out segments the index holds, and
    /// how many distinct chunk layouts. Both stay small for a file whose
    /// channel list rarely changes, however many segments it has.
    pub fn index_shape(&self) -> (usize, usize) {
        (self.index.blocks.len(), self.index.layouts.len())
    }

    fn spans(&self, id: u32) -> &[Span] {
        self.spans[id as usize].get_or_init(|| read::spans(&self.index, id))
    }

    /// Reads the same range of values from several channels at once, each
    /// in the type it is stored in.
    ///
    /// This is the fast way to read many channels: the work is spread over
    /// the current rayon thread pool, and interleaved data is read once for
    /// all of them, not once per channel.
    ///
    /// # Errors
    ///
    /// When the range is not inside every channel, a channel's type cannot
    /// be read, or reading fails.
    pub fn read_columns(&self, channels: &[Channel<'_>], range: Range<u64>) -> Result<Vec<Column>> {
        let plain: Vec<_> = channels.iter().map(|c| (*c, None)).collect();
        self.read_columns_of(&plain, range)
    }

    fn read_columns_of(
        &self,
        channels: &[(Channel<'_>, Option<u32>)],
        range: Range<u64>,
    ) -> Result<Vec<Column>> {
        let rows = range.end.saturating_sub(range.start);
        let mut columns = Vec::with_capacity(channels.len());
        let mut plans = Vec::with_capacity(channels.len());
        for (channel, scaler) in channels {
            channel.check(&range)?;
            let (want, scaler) = match scaler {
                Some(id) => (channel.scaler_type(*id)?, Some(*id)),
                None => channel.storage()?,
            };
            let mut column = Column::empty(want).ok_or(Error::TypeMismatch {
                stored: want,
                requested: "a column",
            })?;
            if let Column::String(strings) = &mut column {
                *strings = channel.read_strings(range.clone())?;
            } else {
                resize_column(&mut column, rows)?;
            }
            columns.push(column);
            plans.push((channel.id, want, scaler));
        }
        let mut requests = Vec::with_capacity(channels.len());
        for (column, &(id, want, scaler)) in columns.iter_mut().zip(&plans) {
            if let Some((size, dest)) = column_bytes(column) {
                requests.push(Request {
                    spans: self.spans(id),
                    scaler,
                    want,
                    size,
                    dest,
                });
            }
        }
        read::read_many(self.io.as_ref(), &self.index, requests, range.start, rows)?;
        Ok(columns)
    }
}

fn zeroed<T: Zeroable + Clone>(len: u64) -> Result<Vec<T>> {
    let bytes = len.saturating_mul(size_of::<T>() as u64);
    let n = usize::try_from(len).map_err(|_| Error::Alloc { bytes })?;
    let mut out = Vec::new();
    out.try_reserve_exact(n)
        .map_err(|_| Error::Alloc { bytes })?;
    out.resize(n, T::zeroed());
    Ok(out)
}

fn resize_column(column: &mut Column, len: u64) -> Result<()> {
    match column {
        Column::I8(v) => *v = zeroed(len)?,
        Column::I16(v) => *v = zeroed(len)?,
        Column::I32(v) => *v = zeroed(len)?,
        Column::I64(v) => *v = zeroed(len)?,
        Column::U8(v) | Column::Bool(v) => *v = zeroed(len)?,
        Column::U16(v) => *v = zeroed(len)?,
        Column::U32(v) => *v = zeroed(len)?,
        Column::U64(v) => *v = zeroed(len)?,
        Column::F32(v) => *v = zeroed(len)?,
        Column::F64(v) => *v = zeroed(len)?,
        Column::Timestamp(v) => *v = zeroed(len)?,
        Column::ComplexF32(v) => *v = zeroed(len)?,
        Column::ComplexF64(v) => *v = zeroed(len)?,
        Column::String(_) => {}
    }
    Ok(())
}

/// The column's values as bytes, with the size of one value.
fn column_bytes(column: &mut Column) -> Option<(usize, &mut [u8])> {
    fn cast<T: bytemuck::Pod>(values: &mut [T]) -> (usize, &mut [u8]) {
        (size_of::<T>(), bytemuck::cast_slice_mut(values))
    }
    Some(match column {
        Column::I8(v) => cast(v),
        Column::I16(v) => cast(v),
        Column::I32(v) => cast(v),
        Column::I64(v) => cast(v),
        Column::U8(v) | Column::Bool(v) => cast(v),
        Column::U16(v) => cast(v),
        Column::U32(v) => cast(v),
        Column::U64(v) => cast(v),
        Column::F32(v) => cast(v),
        Column::F64(v) => cast(v),
        Column::Timestamp(v) => cast(v),
        Column::ComplexF32(v) => cast(v),
        Column::ComplexF64(v) => cast(v),
        Column::String(_) => return None,
    })
}

/// A group of channels.
#[derive(Clone, Copy)]
pub struct Group<'a> {
    file: &'a TdmsFile,
    at: usize,
}

impl fmt::Debug for Group<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Group").field("name", &self.name()).finish()
    }
}

impl<'a> Group<'a> {
    fn info(&self) -> &'a GroupInfo {
        &self.file.groups[self.at]
    }

    /// The group's name.
    pub fn name(&self) -> &'a str {
        &self.info().name
    }

    /// The group's properties. Empty when the file names the group only in
    /// the paths of its channels.
    pub fn properties(&self) -> &'a [Property] {
        self.info()
            .object
            .map_or(&[], |id| &self.file.index.objects[id as usize].props)
    }

    /// The group's channels, in the order the file first mentions them.
    pub fn channels(&self) -> impl ExactSizeIterator<Item = Channel<'a>> + 'a {
        let file = self.file;
        self.info()
            .channels
            .iter()
            .map(move |&id| Channel { file, id })
    }

    /// The channel with this name.
    pub fn channel(&self, name: &str) -> Option<Channel<'a>> {
        self.file.channel(self.name(), name)
    }
}

/// One scaler of a channel whose data is in a driver's raw buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DaqmxScaler {
    /// The number scaling properties refer to the scaler by.
    pub scale_id: u32,
    /// The type of the scaler's raw values.
    pub dtype: DataType,
    /// Whether the scaler is one line of a digital port, read as 0 or 1.
    pub digital_line: bool,
}

/// The waveform properties of a channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Waveform {
    /// `wf_start_time`: when the first sample was taken, if recorded.
    pub start_time: Option<Timestamp>,
    /// `wf_start_offset`: the x value of the first sample, relative to the
    /// start time; zero when absent.
    pub start_offset: f64,
    /// `wf_increment`: the x distance between two samples (seconds, for a
    /// waveform in the time domain).
    pub increment: f64,
    /// `wf_samples`: the number of samples the writer recorded per
    /// waveform, if any. It is not the channel's length.
    pub samples: Option<u64>,
}

impl Waveform {
    /// The x value of sample `i`: `start_offset + i * increment`.
    #[expect(
        clippy::cast_precision_loss,
        reason = "sample numbers beyond 2^53 do not occur"
    )]
    pub fn x(&self, i: u64) -> f64 {
        self.start_offset + i as f64 * self.increment
    }
}

/// A channel: a typed, one-dimensional array of values with properties.
#[derive(Clone, Copy)]
pub struct Channel<'a> {
    file: &'a TdmsFile,
    id: u32,
}

impl fmt::Debug for Channel<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Channel")
            .field("path", &self.path())
            .field("dtype", &self.dtype())
            .field("len", &self.len())
            .finish()
    }
}

impl<'a> Channel<'a> {
    fn object(&self) -> &'a Object {
        &self.file.index.objects[self.id as usize]
    }

    /// The channel's name.
    pub fn name(&self) -> &'a str {
        match &self.object().kind {
            ObjKind::Channel(_, name) => name,
            _ => "",
        }
    }

    /// The name of the channel's group.
    pub fn group_name(&self) -> &'a str {
        match &self.object().kind {
            ObjKind::Channel(group, _) => group,
            _ => "",
        }
    }

    /// The channel's path as the file spells it: `/'group'/'channel'`.
    pub fn path(&self) -> &'a str {
        &self.object().path
    }

    /// The type of the channel's values; [`DataType::Void`] for a channel
    /// that never had data.
    pub fn dtype(&self) -> DataType {
        self.object().dtype.unwrap_or(DataType::Void)
    }

    /// Number of values.
    pub fn len(&self) -> u64 {
        self.object().len
    }

    /// Whether the channel has no values.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The channel's properties, in the order the file first sets them.
    pub fn properties(&self) -> &'a [Property] {
        &self.object().props
    }

    /// The value of the property with this name.
    pub fn property(&self, name: &str) -> Option<&'a PropertyValue> {
        self.properties()
            .iter()
            .find(|p| p.name == name)
            .map(|p| &p.value)
    }

    /// The `unit_string` property.
    pub fn unit(&self) -> Option<&'a str> {
        self.property("unit_string")?.as_str()
    }

    /// The waveform timing, when the channel has a numeric `wf_increment`.
    pub fn waveform(&self) -> Option<Waveform> {
        let increment = self.property("wf_increment")?.as_f64()?;
        Some(Waveform {
            start_time: self
                .property("wf_start_time")
                .and_then(PropertyValue::as_timestamp),
            start_offset: self
                .property("wf_start_offset")
                .and_then(PropertyValue::as_f64)
                .unwrap_or(0.0),
            increment,
            samples: self.property("wf_samples").and_then(PropertyValue::as_u64),
        })
    }

    /// The scalers of a channel stored in a driver's raw buffers, as its
    /// most recent raw data index declares them; empty for other channels.
    pub fn scalers(&self) -> Vec<DaqmxScaler> {
        self.object()
            .scalers
            .iter()
            .map(|s| DaqmxScaler {
                scale_id: s.scale_id,
                dtype: s.dtype,
                digital_line: s.digital,
            })
            .collect()
    }

    /// The scaling that the channel's properties describe.
    pub fn scaling(&self) -> Scaling {
        Scaling::of(self.properties(), !self.object().scalers.is_empty())
    }

    fn check(&self, range: &Range<u64>) -> Result<()> {
        if range.start <= range.end && range.end <= self.len() {
            Ok(())
        } else {
            Err(Error::OutOfRange {
                start: range.start,
                end: range.end,
                len: self.len(),
            })
        }
    }

    /// The type values are read as, and the scaler to name when reading.
    fn storage(&self) -> Result<(DataType, Option<u32>)> {
        let object = self.object();
        match self.dtype() {
            DataType::DaqmxRaw => match object.scalers.as_slice() {
                [only] => Ok((only.dtype, None)),
                _ => Err(Error::NotFound(format!(
                    "a single scaler: {} has {}, read one with read_scaler",
                    object.path,
                    object.scalers.len()
                ))),
            },
            dtype => Ok((dtype, None)),
        }
    }

    fn scaler_type(&self, id: u32) -> Result<DataType> {
        self.object()
            .scalers
            .iter()
            .find(|s| s.scale_id == id)
            .map(|s| s.dtype)
            .ok_or_else(|| Error::NotFound(format!("scaler {id} of {}", self.path())))
    }

    /// Reads a range of the raw values of one scaler in the type it
    /// stores.
    ///
    /// # Errors
    ///
    /// As for [`Self::read_scaler`].
    pub fn read_scaler_column(&self, scale_id: u32, range: Range<u64>) -> Result<Column> {
        let mut columns = self
            .file
            .read_columns_of(&[(*self, Some(scale_id))], range)?;
        columns.pop().ok_or(Error::Alloc { bytes: 0 })
    }

    fn read_raw<T: Sample>(&self, scaler: Option<u32>, start: u64, dest: &mut [T]) -> Result<()> {
        let end = start.saturating_add(dest.len() as u64);
        self.check(&(start..end))?;
        let want = match scaler {
            Some(id) => self.scaler_type(id)?,
            None => self.storage()?.0,
        };
        if !T::accepts(want) {
            return Err(Error::TypeMismatch {
                stored: want,
                requested: T::NAME,
            });
        }
        let request = Request {
            spans: self.file.spans(self.id),
            scaler,
            want,
            size: size_of::<T>(),
            dest: bytemuck::cast_slice_mut(dest),
        };
        read::read_many(
            self.file.io.as_ref(),
            &self.file.index,
            vec![request],
            start,
            end - start,
        )
    }

    /// Reads values into `dest`, starting at value number `start`.
    ///
    /// # Errors
    ///
    /// [`Error::TypeMismatch`] when the channel does not store `T`,
    /// [`Error::OutOfRange`] when `dest` reaches past the channel's end, or
    /// an I/O error.
    pub fn read_into<T: Sample>(&self, start: u64, dest: &mut [T]) -> Result<()> {
        self.read_raw(None, start, dest)
    }

    /// Reads a range of values.
    ///
    /// # Errors
    ///
    /// As for [`Self::read_into`], and [`Error::Alloc`] when the result
    /// does not fit in memory.
    pub fn read<T: Sample>(&self, range: Range<u64>) -> Result<Vec<T>> {
        self.check(&range)?;
        let mut out = zeroed::<T>(range.end - range.start)?;
        self.read_raw(None, range.start, &mut out)?;
        Ok(out)
    }

    /// Reads every value.
    ///
    /// # Errors
    ///
    /// As for [`Self::read`].
    pub fn read_all<T: Sample>(&self) -> Result<Vec<T>> {
        self.read(0..self.len())
    }

    /// Reads a range of the raw values of one scaler of a channel stored in
    /// a driver's raw buffers.
    ///
    /// # Errors
    ///
    /// As for [`Self::read`], and [`Error::NotFound`] for an unknown
    /// scaler.
    pub fn read_scaler<T: Sample>(&self, scale_id: u32, range: Range<u64>) -> Result<Vec<T>> {
        self.check(&range)?;
        let mut out = zeroed::<T>(range.end - range.start)?;
        self.read_raw(Some(scale_id), range.start, &mut out)?;
        Ok(out)
    }

    /// Reads a range of a string channel as the bytes the file holds.
    ///
    /// # Errors
    ///
    /// As for [`Self::read`], and [`Error::CorruptData`] when the string
    /// offsets in the file are inconsistent.
    pub fn read_string_bytes(&self, range: Range<u64>) -> Result<Vec<Vec<u8>>> {
        self.check(&range)?;
        if self.dtype() != DataType::String {
            return Err(Error::TypeMismatch {
                stored: self.dtype(),
                requested: "string",
            });
        }
        read::read_strings(
            self.file.io.as_ref(),
            &self.file.index,
            self.file.spans(self.id),
            range.start,
            range.end,
        )
    }

    /// Reads a range of a string channel. Bytes that are not valid UTF-8
    /// are replaced with U+FFFD; [`Self::read_string_bytes`] keeps them.
    ///
    /// # Errors
    ///
    /// As for [`Self::read_string_bytes`].
    pub fn read_strings(&self, range: Range<u64>) -> Result<Vec<String>> {
        Ok(self
            .read_string_bytes(range)?
            .into_iter()
            .map(|bytes| match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
            })
            .collect())
    }

    /// Reads a range of values in the type the channel stores.
    ///
    /// # Errors
    ///
    /// As for [`Self::read`].
    pub fn read_column(&self, range: Range<u64>) -> Result<Column> {
        let mut columns = self.file.read_columns(&[*self], range)?;
        columns.pop().ok_or(Error::Alloc { bytes: 0 })
    }

    fn read_f64_from(&self, scaler: Option<u32>, range: Range<u64>) -> Result<Vec<f64>> {
        macro_rules! convert {
            ($t:ty, $f:expr) => {{
                let mut raw = zeroed::<$t>(range.end - range.start)?;
                self.read_raw(scaler, range.start, &mut raw)?;
                Ok(raw.into_iter().map($f).collect())
            }};
        }
        self.check(&range)?;
        let stored = match scaler {
            Some(id) => self.scaler_type(id)?,
            None => self.storage()?.0,
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "documented on read_f64: 64-bit integers lose their low bits"
        )]
        match stored.without_unit() {
            DataType::I8 => convert!(i8, f64::from),
            DataType::I16 => convert!(i16, f64::from),
            DataType::I32 => convert!(i32, f64::from),
            DataType::I64 => convert!(i64, |v| v as f64),
            DataType::U8 => convert!(u8, f64::from),
            DataType::Bool => convert!(u8, |v| f64::from(u8::from(v != 0))),
            DataType::U16 => convert!(u16, f64::from),
            DataType::U32 => convert!(u32, f64::from),
            DataType::U64 => convert!(u64, |v| v as f64),
            DataType::F32 => convert!(f32, f64::from),
            DataType::F64 => {
                let mut raw = zeroed::<f64>(range.end - range.start)?;
                self.read_raw(scaler, range.start, &mut raw)?;
                Ok(raw)
            }
            other => Err(Error::TypeMismatch {
                stored: other,
                requested: "f64",
            }),
        }
    }

    /// Reads a range of a numeric or boolean channel converted to `f64`,
    /// without applying any scaling. Integers of 64 bits beyond 2^53 lose
    /// their low bits.
    ///
    /// # Errors
    ///
    /// As for [`Self::read`]; [`Error::TypeMismatch`] for strings, times
    /// and complex numbers.
    pub fn read_f64(&self, range: Range<u64>) -> Result<Vec<f64>> {
        self.read_f64_from(None, range)
    }

    /// Reads a range of values in engineering units: the raw values with
    /// the channel's [`Scaling`] applied. A channel without scaling
    /// properties gives the same as [`Self::read_f64`].
    ///
    /// # Errors
    ///
    /// As for [`Self::read_f64`], and [`Error::Scaling`] when the channel
    /// uses a kind of scale that is not supported.
    pub fn read_scaled(&self, range: Range<u64>) -> Result<Vec<f64>> {
        match self.scaling() {
            Scaling::None => self.read_f64(range),
            Scaling::Linear {
                slope,
                intercept,
                scaler,
            } => {
                let mut values = self.read_f64_from(scaler, range)?;
                for v in &mut values {
                    *v = *v * slope + intercept;
                }
                Ok(values)
            }
            Scaling::Unsupported(kind) => Err(Error::Scaling(format!(
                "{} uses a scale of type {kind}",
                self.path()
            ))),
        }
    }

    /// An iterator over the channel's values in pieces of `chunk_len`
    /// values (the last may be shorter), for reading a long channel without
    /// holding all of it.
    pub fn chunks<T: Sample>(&self, chunk_len: usize) -> Chunks<'a, T> {
        Chunks {
            channel: *self,
            pos: 0,
            chunk_len: chunk_len.max(1) as u64,
            marker: PhantomData,
        }
    }
}

/// The iterator of [`Channel::chunks`].
#[derive(Debug)]
pub struct Chunks<'a, T> {
    channel: Channel<'a>,
    pos: u64,
    chunk_len: u64,
    marker: PhantomData<T>,
}

impl<T: Sample> Iterator for Chunks<'_, T> {
    type Item = Result<Vec<T>>;

    fn next(&mut self) -> Option<Self::Item> {
        let len = self.channel.len();
        if self.pos >= len {
            return None;
        }
        let end = len.min(self.pos.saturating_add(self.chunk_len));
        let out = self.channel.read(self.pos..end);
        // An error ends the iteration after it has been reported.
        self.pos = if out.is_ok() { end } else { len };
        Some(out)
    }
}
