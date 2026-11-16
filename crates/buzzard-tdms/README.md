# buzzard-tdms

A fast, safe, read-only reader for TDMS measurement files, the format that
data-acquisition software logs test data in, with export to Parquet and
CSV. It is built for test-stand logs: hundreds of channels at about 1 kHz,
a few at 100 kHz and more, files of several gigabytes made of many small
segments.

```rust
use buzzard_tdms::TdmsFile;

let file = TdmsFile::open("hotfire-0042.tdms")?;
for channel in file.channels() {
    println!("{} {} x {}", channel.path(), channel.len(), channel.dtype());
}
let pressure = file.channel("Chamber", "PT-101").expect("the channel exists");
let last_second: Vec<f64> = pressure.read(pressure.len() - 1000..pressure.len())?;
```

From the command line, through the `buzzard` binary:

```sh
buzzard tdms info hotfire-0042.tdms                 # or --format json
buzzard tdms export hotfire-0042.tdms --out slow.parquet --channels Chamber
buzzard tdms export hotfire-0042.tdms --out pt101.csv --format csv --channels Chamber/PT-101
```

The format is described by its vendor in
[TDMS File Format Internal Structure](https://www.ni.com/en/support/documentation/supplemental/07/tdms-file-format-internal-structure.html).
That page is the specification this crate follows; where it is silent, the
behaviour of [npTDMS](https://github.com/adamreeve/npTDMS) is the reference
(see [The oracle](#the-oracle)).

## How it reads

Opening a file is **pass one**: every segment's 28-byte lead-in and its
metadata are read, and no raw data. The result is an index of *layouts*
(which channel sits where in a chunk of raw data) and *runs* (consecutive
segments with the same layout and spacing). A log that appends the same
channels once a second for an hour is one run and one layout, whatever its
size, so opening takes milliseconds and the index a few kilobytes. When a
`.tdms_index` file lies beside the data file, the metadata is read from it
after it has been checked against the data file (the total length, and
the lead-ins of the first and last segments and of up to 64 spread evenly
between them); a
stale, damaged or foreign index file is ignored without an error.
`TdmsFile::index_source` says which was used.

Reading values is **pass two**, per channel and per range of samples.
Contiguous values are read straight into the destination with one
positional read per run. Interleaved rows and driver buffers are read a
megabyte of rows at a time, and every requested channel is picked out of a
block before the next is read: `TdmsFile::read_columns` reads a wide
interleaved file once, not once per channel. Large reads are split over
the rayon thread pool.

There is no `unsafe` code, which the workspace forbids, and so no memory
mapping: files are read with positional reads (`pread`, or `seek_read` on
Windows) into buffers the reader owns. A file that shrinks while it is
open gives an I/O error, not a bus fault. The I/O sits behind the two
methods of the `ReadAt` trait, so a mapped backend can be added by a crate
that allows `unsafe`. What not mapping costs is measured below.

## What is supported

Supported:

- Segments: the lead-in, all six table-of-contents flags, little- and
  big-endian segments (also mixed in one file), versions 4712 and 4713
  (any other number is read and reported), padding between metadata and
  raw data, segments without metadata, segments with metadata only, any
  number of chunks after one lead-in.
- Metadata: incremental object lists, the "new object list" flag, the raw
  data index markers "none" and "same as before" (including for an object
  that an intervening segment dropped), channels that are added, removed,
  reordered and resized, files without a file object or group objects.
- Raw data, contiguous or interleaved (channels of different widths in one
  row): 8- to 64-bit signed and unsigned integers, 32- and 64-bit floats,
  the "float with unit" types, booleans, timestamps, complex numbers of
  32- and 64-bit floats, and strings (contiguous only, as the format has
  them).
- Timestamps keep their stored 64 bits of whole seconds and 64 bits of
  fraction; `Timestamp` converts to Unix nanoseconds, to seconds as a
  float, and to RFC 3339 text, each rounding down.
- Properties of the file, groups and channels in every one of those types,
  with their type codes, in the order they were first set; a later value
  replaces an earlier one. `Channel::waveform` gathers `wf_start_time`,
  `wf_start_offset`, `wf_increment` and `wf_samples`; `Channel::unit`
  reads `unit_string`.
- Driver raw data ("DAQmx"): format changing scalers of the integer and
  float types in any number of raw buffers, digital line scalers, several
  scalers per channel (`Channel::read_scaler`), and linear scaling,
  including chains of linear scales (`Channel::read_scaled`).
- Truncated and unfinished files, as below.

Partly supported:

- Scaling other than linear (polynomial, thermocouple, RTD, strain, table,
  thermistor, add, subtract): recognized and named by `Channel::scaling`,
  not applied. The raw values are readable.
- Driver raw data: all driver channels of a segment must have the same
  number of values; a timestamp scaler is read as a 16-byte timestamp,
  which no available file confirms; an incomplete last chunk is recovered
  only when there is a single raw buffer.
- Text that is not UTF-8 (names, string properties, string channels) is
  read with the bad bytes replaced by U+FFFD and reported as an anomaly;
  `Channel::read_string_bytes` returns string values as stored.
- The two format versions are read the same way. The format description
  names one difference (below) and no others.

Not supported:

- Extended-precision floats (type codes 11 and 0x1B) and fixed-point
  numbers (0x4F). Their layout is not published. A file with values of
  these types fails to open with `Error::Unsupported`.
- Renaming an object by writing its `name` property (version 4713): the
  property is stored like any other.
- Array dimensions other than 1, string channels in interleaved segments,
  interleaved channels of different lengths: errors, as the format has
  them.
- Writing or defragmenting files (`build` writes test files only), the
  XML-based TDM/TDX formats, following a file while it is being written
  (its size is taken once, at open), memory mapping.

## Damaged and hostile files

A crashed writer leaves a last segment that is unfinished or cut short.
By default the reader recovers what is there and lists what it found in
`TdmsFile::anomalies` (at most 1024 are kept); `OpenOptions::strict` turns
any anomaly into an error.

| Found | Result |
| --- | --- |
| The length field of the last segment was never written (all bits set), or the segment ends beyond the end of the file | read to the end of the file |
| The file ends inside a lead-in or inside a segment's metadata | that segment is ignored; the ones before it are intact |
| Raw data that is not a whole number of chunks, in a segment that was cut short and is contiguous | channels are complete in order until the bytes run out |
| The same, interleaved | the whole rows are kept |
| The same, in a finished segment | every channel is shortened in proportion (what npTDMS does) |
| The same, with a string channel | the incomplete chunk is skipped |
| Bytes that no channel accounts for, or raw data in a segment whose table of contents says it has none | skipped and reported |

Everything else that contradicts the format is an error with the byte
offset and the segment number: a missing segment tag, metadata longer than
its segment, counts and lengths that do not fit, a raw data index whose
length disagrees with its type, an unknown type code, a channel that
changes type, a scaler outside its buffer.

Sizes in a file are never trusted. No allocation is made from a declared
count: a channel cannot be longer than the bytes the file has for it, and
buffers are allocated fallibly. The index is bounded as well. Layouts
that a file spells out in its metadata cost memory in proportion to that
metadata; layouts beyond that (a file can make the reader derive a new
layout of thousands of channels from a few bytes) count against
`OpenOptions::max_layout_entries`, 16,777,216 channel positions by default
or under 1 GiB, and against sixteen times that in work, after which
opening fails with `Error::IndexTooLarge`.

## Export

`export::parquet::write_parquet` (feature `parquet`) and
`export::write_csv` write the chosen channels as one table: one column per
channel named `group/channel` (two for a complex channel, `.re` and
`.im`), as many rows as the longest channel, missing values where a
shorter channel has ended. A table is meant for channels of one sample
rate; export the 1 kHz and the 100 kHz channels separately.

- **Timing is explicit.** When every exported channel has the same
  `wf_start_time`, `wf_start_offset` and `wf_increment`, a leading
  `time_s` column holds `wf_start_offset + row * wf_increment` and the
  metadata holds the start time.
- **Metadata is carried.** Parquet key-value metadata holds every property
  of the file (`tdms.file.<name>`), of the groups (`tdms.group.<group>.<name>`)
  and of the channels (`tdms.channel.<column>.<name>`), and for each column
  its path, stored type, length, scaling and timing
  (`buzzard.channel.<column>.…`). CSV has the table only.
- **Scaling.** A channel with linear scaling is exported in engineering
  units as 64-bit floats unless `ExportOptions::scaled` is off.
- **Types.** Integers and floats keep their width; times become UTC
  timestamps in nanoseconds (missing outside the years 1677 to 2262).
- **Deterministic.** The same input and options give the same bytes: keys
  are in a fixed order, no creation time is written, and the Arrow schema
  is not embedded.

The table is written a batch of rows at a time, one Parquet row group per
batch, decoded and encoded in parallel, so memory use does not grow with
the file.

The `parquet` feature is **off by default** in this crate: the reader
alone builds with three small dependencies (`bytemuck`, `rayon`,
`thiserror`), and the Arrow and Parquet crates add about forty more. The
`buzzard` binary turns it on by default, because Parquet is its default
export format. Compression is Snappy or none. zstd is not offered: the
`zstd-sys` crate is under `BSD-3-Clause AND BSD-2-Clause`, and BSD-2-Clause
is not on the licence allow-list in `deny.toml`.

## Speed

Measured with `tools/tdms-bench.sh 400` on a 4-core virtual machine
(Xeon at 2.1 GHz), files of about 415 MB in the page cache, best of three.
Megabytes per second are file bytes over wall time.

| | 1 thread | 4 threads |
| --- | --- | --- |
| **Test stand**: 300 x f64 at 1 kHz and 4 x f64 at 100 kHz, 74 one-second segments | | |
| open | 0.6 ms | 0.6 ms |
| read everything | 1,490 MB/s | 2,430 MB/s |
| read one 100 kHz channel (59 MB) | 18 ms | 9 ms |
| read one 1 kHz channel (0.6 MB) | 0.12 ms | 0.08 ms |
| export to Parquet, Snappy | 400 MB/s | 685 MB/s |
| **Many segments**: 300 x f64, 174 segments of 1000 samples | | |
| read everything | 2,360 MB/s | 4,750 MB/s |
| export to Parquet, Snappy | 530 MB/s | 970 MB/s |
| **Interleaved**: 300 x f64, one segment of 1000-row chunks | | |
| read everything | 1,610 MB/s | 3,700 MB/s |
| read one channel (1.4 MB out of 418 MB of rows) | 72 ms | 19 ms |
| export to Parquet, Snappy | 520 MB/s | 970 MB/s |

For comparison, npTDMS 1.14 on the same machine reads files of the last
two shapes at 470 to 750 MB/s and 170 MB/s, and takes 1.9 s for one
channel of the interleaved file.

**The cost of not mapping.** The benchmark reads each file a second time
from a copy in memory, which is what a mapped backend would do: the same
copies into the caller's buffers, without the system calls. On one thread
positional reads are 9 to 16 % slower (1,490 against 1,630 MB/s on the
test-stand file, 2,360 against 2,800 MB/s on the many-segments file). On
four threads the difference is within the noise for two of the three
shapes and 16 % for the third. What mapping could add beyond that is
reads that copy nothing, for contiguous little-endian data; this crate
does not offer borrowed views, so that is not measured.

These are warm-cache numbers. On a cold file the reader is as fast as the
storage.

## Tests

- **A generator with its own model** (`build`). It writes files from the
  format description and keeps its own account of what each channel
  holds. `tests/generated.rs` reads back the cross product of byte order,
  layout, every type, chunking, format version and padding over a
  sequence of nine segments that exercises the metadata rules, with and
  without an index file. `tests/property.rs` does the same for random
  sequences of segments, and checks that any prefix of a file gives a
  prefix of every channel.
- **The oracle** (`tests/oracle.rs`), below.
- **Hostile input** (`tests/hostile.rs`): every file cut at every byte,
  every single byte of data files and index files damaged five ways,
  seeded random damage, directed cases for each kind of absurd count,
  length and offset, a million empty segments, 100,000 one-byte segments,
  50,000 objects, and files built to blow up the index.
- **A real file** (`tests/real.rs`), from a public data set under
  CC BY 4.0; `tests/fixtures/README.md` has its source and checksums.
  `BUZZARD_TDMS_SAMPLES=<dir> cargo test -p buzzard-tdms --release --test real -- --ignored`
  reads every file of a directory of your own.

## The oracle

`tests/oracle/dump.py` asks npTDMS what it makes of a set of files, and
`tests/oracle.rs` requires this reader to agree: groups, channels, types,
lengths, every property, a hash of every channel's raw values, raw
timestamps, and scaled values. It runs when `BUZZARD_TDMS_ORACLE` names a
Python interpreter with npTDMS installed, and in CI on every change.
`tests/oracle/README.md` has the details.

615 files are compared: this crate's generated files, files written by
npTDMS's own writer, the real file in `tests/fixtures`, and, because the
npTDMS wheel ships its test package, the 31 scenario files and 4 data
files that come with it, run as a black box where pip installed them.

The two readers disagree in eight ways. Each is an entry in
`KNOWN_DIFFERENCES` in `tests/oracle.rs`:

| Case | npTDMS | This reader |
| --- | --- | --- |
| A property of a complex type | refuses the file | reads it |
| Interleaved channels of complex numbers | cannot read them | reads them |
| Order of groups | groups with an object of their own first | order of first mention in the file |
| A finished interleaved segment that ends in a partial row, with segments after it | raises an error | keeps the whole rows, reports an anomaly |
| A first segment without metadata | refuses the file | reports its bytes as an anomaly, reads on |
| A raw data index whose length field is wrong | ignores the field | refuses the file |
| Raw data in a segment whose table of contents says it has none | reads it as values | skips it, reports an anomaly |
| Linear scaling of 32-bit float raw values | computes in single precision | computes in double precision |

One limit of the oracle: npTDMS returns every integer property as a
Python integer, so the width of integer properties is checked by the
generator's model only.

## Licence boundary

This crate is MIT. npTDMS is LGPL-3.0, and none of it is here: no code,
no test, no fixture was copied or ported. The reader was written from the
vendor's format description, from research notes that state format facts
in prose (some of them first established by reading npTDMS, and recorded
as facts, not as code), and from npTDMS's behaviour observed through the
oracle. npTDMS runs only as a separate program in a test, installed from
PyPI; it is not a dependency of the crate. The vendor's document is
linked, not reproduced.

"TDMS", "DAQmx", "LabVIEW" and "NI" are used here only to name the format
and the software that writes it. This project is not affiliated with their
owner.
