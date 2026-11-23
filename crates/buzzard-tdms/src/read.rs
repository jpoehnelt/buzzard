//! Pass two copies the requested values of the requested channels out of
//! the file.
//!
//! Everything here works on bytes. A request names a channel, a range of
//! its values and a destination of exactly the right size. The typed
//! functions in `file.rs` cast their slices to bytes and back.
//!
//! Contiguous values are read straight into the destination with one
//! positional read per run, so the only copy is the kernel's. Values that
//! sit in rows (interleaved data and driver buffers) are read a block of
//! rows at a time into a scratch buffer, and every requested channel is
//! picked out of the block before the next one is read. A wide interleaved
//! file is therefore read once, however many of its channels are requested.

use rayon::prelude::*;

use crate::error::{Error, Result};
use crate::index::{Block, Index, Layout, Loc, Source};
use crate::io::ReadAt;
use crate::types::{DataType, Shape};

/// Bytes of rows read into the scratch buffer at once.
const GATHER_BYTES: u64 = 1 << 20;
/// Rows wider than this are not buffered. Their values are read one by one.
const WIDE_ROW: u64 = 1 << 16;
/// Bytes of one channel that a single task reads when nothing is in rows.
const DIRECT_BYTES: u64 = 4 << 20;
/// Below this many bytes in total a read stays on the calling thread.
const SERIAL_BYTES: u64 = 256 << 10;

/// The part of a block that belongs to one channel.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Span {
    pub(crate) block: usize,
    pub(crate) entry: u32,
    /// The channel's value number at the start of the block.
    pub(crate) first: u64,
    pub(crate) count: u64,
}

/// Every block the object has values in, in file order.
pub(crate) fn spans(index: &Index, obj: u32) -> Vec<Span> {
    let layouts = &index.objects[obj as usize].layouts;
    let mut out = Vec::new();
    let mut first = 0u64;
    for (at, block) in index.blocks.iter().enumerate() {
        let Ok(found) = layouts.binary_search_by_key(&block.layout, |&(layout, _)| layout) else {
            continue;
        };
        let entry = layouts[found].1;
        let n = index.layouts[block.layout as usize].entries[entry as usize].n;
        let count = n.saturating_mul(block.chunks).saturating_mul(block.repeat);
        out.push(Span {
            block: at,
            entry,
            first,
            count,
        });
        first = first.saturating_add(count);
    }
    out
}

/// Whether values stored as `stored` can be read into a buffer for `want`.
pub(crate) fn compatible(stored: DataType, want: DataType) -> bool {
    let class = |t: DataType| match t.without_unit() {
        DataType::Bool => DataType::U8,
        other => other,
    };
    class(stored) == class(want)
}

/// One channel's part of a read.
pub(crate) struct Request<'a> {
    pub(crate) spans: &'a [Span],
    /// Which scaler of a driver channel, or `None` for its only one.
    pub(crate) scaler: Option<u32>,
    pub(crate) want: DataType,
    /// Bytes per value in `dest`.
    pub(crate) size: usize,
    pub(crate) dest: &'a mut [u8],
}

/// A run of values that are evenly spaced in the file.
struct Piece {
    /// Offset of the first value.
    file: u64,
    /// Offset of the row that holds the first value.
    row: u64,
    stride: u64,
    count: u64,
    big_endian: bool,
    bit: Option<u8>,
    /// The stored type when it can differ from the channel's.
    dtype: Option<DataType>,
}

struct Pieces<'a> {
    index: &'a Index,
    spans: &'a [Span],
    scaler: Option<u32>,
    pos: u64,
    end: u64,
    at: usize,
}

impl<'a> Pieces<'a> {
    fn new(index: &'a Index, spans: &'a [Span], scaler: Option<u32>, pos: u64, end: u64) -> Self {
        let at = spans.partition_point(|s| s.first.saturating_add(s.count) <= pos);
        Self {
            index,
            spans,
            scaler,
            pos,
            end,
            at,
        }
    }

    fn locate(&self, layout: &Layout, entry: u32) -> Result<(Loc, Option<u8>, Option<DataType>)> {
        match &layout.entries[entry as usize].source {
            Source::Fixed(loc) => Ok((*loc, None, None)),
            Source::Strings { .. } => Err(Error::TypeMismatch {
                stored: DataType::String,
                requested: "a fixed-size value",
            }),
            Source::Daqmx(scalers) => {
                let scaler = match self.scaler {
                    Some(id) => scalers.iter().find(|s| s.scale_id == id),
                    None if scalers.len() == 1 => scalers.first(),
                    None => None,
                };
                let scaler = scaler.ok_or_else(|| {
                    Error::NotFound(match self.scaler {
                        Some(id) => format!("scaler {id} in a part of the channel's data"),
                        None => "a single scaler in a part of the channel's data. \
                                 Name one of the channel's scalers"
                            .to_owned(),
                    })
                })?;
                Ok((scaler.loc, scaler.bit, Some(scaler.dtype)))
            }
        }
    }
}

impl Iterator for Pieces<'_> {
    type Item = Result<Piece>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.end {
            return None;
        }
        while self.at < self.spans.len()
            && self.spans[self.at]
                .first
                .saturating_add(self.spans[self.at].count)
                <= self.pos
        {
            self.at += 1;
        }
        let span = self.spans.get(self.at)?;
        let block: &Block = &self.index.blocks[span.block];
        let layout = &self.index.layouts[block.layout as usize];
        let (loc, bit, dtype) = match self.locate(layout, span.entry) {
            Ok(found) => found,
            Err(e) => {
                self.pos = self.end;
                return Some(Err(e));
            }
        };
        let n = layout.entries[span.entry as usize].n;
        let per_segment = n * block.chunks;
        let rel = self.pos - span.first;
        let segment = rel / per_segment;
        let within = rel % per_segment;
        let base = block.start + segment * block.stride;
        // When a channel's rows fill the chunk, chunks follow one another
        // without a gap and a run may cross them.
        let tiles = n.checked_mul(loc.stride) == Some(layout.chunk_size);
        let (file, run) = if tiles {
            (
                base + loc.offset + within * loc.stride,
                per_segment - within,
            )
        } else {
            let (chunk, k) = (within / n, within % n);
            (
                base + chunk * layout.chunk_size + loc.offset + k * loc.stride,
                n - k,
            )
        };
        let count = run.min(self.end - self.pos);
        self.pos += count;
        Some(Ok(Piece {
            file,
            row: file - loc.col,
            stride: loc.stride,
            count,
            big_endian: layout.big_endian,
            bit,
            dtype,
        }))
    }
}

fn reverse_groups(bytes: &mut [u8], group: usize) {
    fn fixed<const N: usize>(bytes: &mut [u8]) {
        for value in bytes.as_chunks_mut::<N>().0 {
            value.reverse();
        }
    }
    match group {
        0 | 1 => {}
        2 => fixed::<2>(bytes),
        4 => fixed::<4>(bytes),
        8 => fixed::<8>(bytes),
        16 => fixed::<16>(bytes),
        other => bytes.chunks_exact_mut(other).for_each(<[u8]>::reverse),
    }
}

/// Turns values in the file's byte order into the host's, in place.
pub(crate) fn fix_order(bytes: &mut [u8], size: usize, shape: Shape, big_endian: bool) {
    let host_big = cfg!(target_endian = "big");
    match shape {
        Shape::Scalar if big_endian != host_big => reverse_groups(bytes, size),
        Shape::Pair if big_endian != host_big => reverse_groups(bytes, size / 2),
        Shape::Scalar | Shape::Pair => {}
        // In memory the fractions come first, then the seconds. A
        // little-endian file stores them in that order. A big-endian file
        // stores the seconds first, so the whole value is reversed.
        Shape::Time => match (big_endian, host_big) {
            (false, false) => {}
            (true, false) => reverse_groups(bytes, 16),
            (false, true) => reverse_groups(bytes, 8),
            (true, true) => {
                for value in bytes.as_chunks_mut::<16>().0 {
                    value.rotate_left(8);
                }
            }
        },
    }
}

fn gather_fixed<const N: usize>(src: &[u8], stride: usize, col: usize, dest: &mut [u8]) {
    for (row, out) in src.chunks_exact(stride).zip(dest.as_chunks_mut::<N>().0) {
        out.copy_from_slice(&row[col..col + N]);
    }
}

/// Copies the `size` bytes at `col` of every row of `src` to `dest`.
fn gather(src: &[u8], stride: usize, col: usize, size: usize, dest: &mut [u8]) {
    match size {
        1 => gather_fixed::<1>(src, stride, col, dest),
        2 => gather_fixed::<2>(src, stride, col, dest),
        4 => gather_fixed::<4>(src, stride, col, dest),
        8 => gather_fixed::<8>(src, stride, col, dest),
        16 => gather_fixed::<16>(src, stride, col, dest),
        _ => {
            for (row, out) in src.chunks_exact(stride).zip(dest.chunks_exact_mut(size)) {
                out.copy_from_slice(&row[col..col + size]);
            }
        }
    }
}

/// Turns each value into the chosen bit of its lowest byte.
fn extract_bit(dest: &mut [u8], size: usize, bit: u8) {
    let low = if cfg!(target_endian = "big") {
        size - 1
    } else {
        0
    };
    for value in dest.chunks_exact_mut(size) {
        let b = (value[low] >> bit) & 1;
        value.fill(0);
        value[low] = b;
    }
}

/// The most recently read block of rows.
#[derive(Default)]
struct Scratch {
    start: u64,
    buf: Vec<u8>,
}

impl Scratch {
    fn rows(&mut self, io: &dyn ReadAt, start: u64, len: usize) -> Result<&[u8]> {
        let inside = start >= self.start
            && usize::try_from(start - self.start)
                .ok()
                .and_then(|at| at.checked_add(len))
                .is_some_and(|end| end <= self.buf.len());
        if !inside {
            self.buf.clear();
            self.buf
                .try_reserve(len)
                .map_err(|_| Error::Alloc { bytes: len as u64 })?;
            self.buf.resize(len, 0);
            self.start = start;
            if let Err(e) = io.read_exact_at(start, &mut self.buf) {
                self.buf.clear();
                return Err(e.into());
            }
        }
        let at = usize::try_from(start - self.start).unwrap_or(0);
        Ok(&self.buf[at..at + len])
    }
}

fn to_usize(value: u64) -> Result<usize> {
    usize::try_from(value).map_err(|_| Error::Alloc { bytes: value })
}

fn read_piece(
    io: &dyn ReadAt,
    piece: &Piece,
    size: usize,
    dest: &mut [u8],
    scratch: &mut Scratch,
) -> Result<()> {
    // A digital line is one byte per row, whatever the scaler's type.
    let taken = if piece.bit.is_some() { 1 } else { size };
    if piece.stride == size as u64 && piece.bit.is_none() {
        io.read_exact_at(piece.file, dest)?;
    } else if piece.stride > WIDE_ROW {
        let low = if cfg!(target_endian = "big") {
            size - taken
        } else {
            0
        };
        let mut at = piece.file;
        for value in dest.chunks_exact_mut(size) {
            io.read_exact_at(at, &mut value[low..low + taken])?;
            at += piece.stride;
        }
    } else {
        let stride = to_usize(piece.stride)?;
        let col = to_usize(piece.file - piece.row)?;
        let rows_per_block = to_usize((GATHER_BYTES / piece.stride).max(1))?;
        let mut start = piece.row;
        for out in dest.chunks_mut(rows_per_block * size) {
            let len = out.len() / size * stride;
            let src = scratch.rows(io, start, len)?;
            if taken == size {
                gather(src, stride, col, size, out);
            } else {
                let low = if cfg!(target_endian = "big") {
                    size - 1
                } else {
                    0
                };
                for (row, value) in src.chunks_exact(stride).zip(out.chunks_exact_mut(size)) {
                    value[low] = row[col];
                }
            }
            start += len as u64;
        }
    }
    Ok(())
}

/// Fills one request with the values `start..start + dest.len() / size`.
fn run(
    io: &dyn ReadAt,
    index: &Index,
    request: &mut Request<'_>,
    start: u64,
    scratch: &mut Scratch,
) -> Result<()> {
    let size = request.size;
    let shape = crate::types::shape_of(request.want);
    let rows = (request.dest.len() / size) as u64;
    let mut done = 0usize;
    for piece in Pieces::new(index, request.spans, request.scaler, start, start + rows) {
        let piece = piece?;
        if let Some(stored) = piece.dtype {
            if !compatible(stored, request.want) {
                return Err(Error::TypeMismatch {
                    stored,
                    requested: request.want.name(),
                });
            }
        }
        let bytes = to_usize(piece.count)? * size;
        let dest = &mut request.dest[done..done + bytes];
        read_piece(io, &piece, size, dest, scratch)?;
        match piece.bit {
            Some(bit) => extract_bit(dest, size, bit),
            None => fix_order(dest, size, shape, piece.big_endian),
        }
        done += bytes;
    }
    if done == request.dest.len() {
        Ok(())
    } else {
        // The caller checked the range against the channel's length.
        Err(Error::OutOfRange {
            start,
            end: start + rows,
            len: start + (done / size) as u64,
        })
    }
}

struct Plan<'a> {
    io: &'a dyn ReadAt,
    index: &'a Index,
    /// Whether any requested value sits in a row.
    rows: bool,
    leaf: u64,
}

fn split(requests: Vec<Request<'_>>, rows: u64) -> Result<(Vec<Request<'_>>, Vec<Request<'_>>)> {
    let rows = to_usize(rows)?;
    let mut left = Vec::with_capacity(requests.len());
    let mut right = Vec::with_capacity(requests.len());
    for request in requests {
        let Request {
            spans,
            scaler,
            want,
            size,
            dest,
        } = request;
        let (a, b) = dest.split_at_mut(rows * size);
        left.push(Request {
            spans,
            scaler,
            want,
            size,
            dest: a,
        });
        right.push(Request {
            spans,
            scaler,
            want,
            size,
            dest: b,
        });
    }
    Ok((left, right))
}

fn go(plan: &Plan<'_>, mut requests: Vec<Request<'_>>, start: u64, rows: u64) -> Result<()> {
    if rows > plan.leaf {
        let half = (rows / 2).div_ceil(plan.leaf) * plan.leaf;
        let (left, right) = split(requests, half)?;
        let (a, b) = rayon::join(
            || go(plan, left, start, half),
            || go(plan, right, start + half, rows - half),
        );
        return a.and(b);
    }
    let bytes: u64 = requests.iter().map(|r| r.dest.len() as u64).sum();
    if plan.rows || requests.len() == 1 || bytes < SERIAL_BYTES {
        let mut scratch = Scratch::default();
        requests
            .iter_mut()
            .try_for_each(|r| run(plan.io, plan.index, r, start, &mut scratch))
    } else {
        requests
            .par_iter_mut()
            .try_for_each(|r| run(plan.io, plan.index, r, start, &mut Scratch::default()))
    }
}

/// Reads the values `start..start + rows` of every request. Each
/// destination holds exactly `rows` values, and the caller has checked the
/// range against the length of every channel.
pub(crate) fn read_many(
    io: &dyn ReadAt,
    index: &Index,
    requests: Vec<Request<'_>>,
    start: u64,
    rows: u64,
) -> Result<()> {
    if rows == 0 || requests.is_empty() {
        return Ok(());
    }
    let mut widest = 1u64;
    let mut in_rows = false;
    let mut largest = 1u64;
    for request in &requests {
        let size = request.size as u64;
        largest = largest.max(size);
        let mut seen = u32::MAX;
        for span in request.spans {
            let block = &index.blocks[span.block];
            if block.layout == seen {
                continue;
            }
            seen = block.layout;
            let entry = &index.layouts[block.layout as usize].entries[span.entry as usize];
            let stride = match &entry.source {
                Source::Fixed(loc) => loc.stride,
                Source::Daqmx(scalers) => scalers.iter().map(|s| s.loc.stride).max().unwrap_or(1),
                Source::Strings { .. } => size,
            };
            if stride != size && stride <= WIDE_ROW {
                in_rows = true;
                widest = widest.max(stride);
            }
        }
    }
    let leaf = if in_rows {
        (GATHER_BYTES / widest).max(1)
    } else {
        (DIRECT_BYTES / largest).max(1)
    };
    let plan = Plan {
        io,
        index,
        rows: in_rows,
        leaf,
    };
    go(&plan, requests, start, rows)
}

/// Reads the strings `start..end` of a string channel as raw bytes.
pub(crate) fn read_strings(
    io: &dyn ReadAt,
    index: &Index,
    spans: &[Span],
    start: u64,
    end: u64,
) -> Result<Vec<Vec<u8>>> {
    const BATCH: u64 = 1 << 16;
    let mut out = Vec::new();
    let mut pos = start;
    let mut at = spans.partition_point(|s| s.first.saturating_add(s.count) <= pos);
    while pos < end {
        while at < spans.len() && spans[at].first.saturating_add(spans[at].count) <= pos {
            at += 1;
        }
        let Some(span) = spans.get(at) else {
            break;
        };
        let block = &index.blocks[span.block];
        let layout = &index.layouts[block.layout as usize];
        let entry = &layout.entries[span.entry as usize];
        let Source::Strings { offset, total } = entry.source else {
            return Err(Error::TypeMismatch {
                stored: DataType::Void,
                requested: "string",
            });
        };
        let n = entry.n;
        let rel = pos - span.first;
        let per_segment = n * block.chunks;
        let within = rel % per_segment;
        let (chunk, first) = (within / n, within % n);
        let base =
            block.start + (rel / per_segment) * block.stride + chunk * layout.chunk_size + offset;
        let last = n.min(first + (end - pos)).min(first + BATCH);
        let corrupt = |reason| Error::CorruptData {
            offset: base,
            reason,
        };
        // End offsets of the strings first - 1 .. last, relative to the
        // start of the characters.
        let skip = first.saturating_sub(1);
        let mut table = Vec::new();
        let table_len = to_usize((last - skip) * 4)?;
        table.try_reserve(table_len).map_err(|_| Error::Alloc {
            bytes: table_len as u64,
        })?;
        table.resize(table_len, 0u8);
        io.read_exact_at(base + skip * 4, &mut table)?;
        let mut ends = table.as_chunks::<4>().0.iter().map(|&b| {
            u64::from(if layout.big_endian {
                u32::from_be_bytes(b)
            } else {
                u32::from_le_bytes(b)
            })
        });
        let chars_len = total - n * 4;
        let begin = if first == 0 {
            0
        } else {
            ends.next().unwrap_or(0)
        };
        let ends: Vec<u64> = ends.collect();
        let mut prev = begin;
        for &e in &ends {
            if e < prev || e > chars_len {
                return Err(corrupt(
                    "string offsets are not in order or leave the block",
                ));
            }
            prev = e;
        }
        let chars_bytes = to_usize(prev - begin)?;
        let mut chars = Vec::new();
        chars.try_reserve(chars_bytes).map_err(|_| Error::Alloc {
            bytes: chars_bytes as u64,
        })?;
        chars.resize(chars_bytes, 0u8);
        io.read_exact_at(base + n * 4 + begin, &mut chars)?;
        let mut from = 0usize;
        for &e in &ends {
            let to = to_usize(e - begin)?;
            out.push(chars[from..to].to_vec());
            from = to;
        }
        pos += last - first;
    }
    if pos < end {
        return Err(Error::OutOfRange {
            start,
            end,
            len: pos,
        });
    }
    Ok(out)
}
