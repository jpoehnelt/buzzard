# buzzard-tdms

A read-only reader for TDMS measurement files, the format that
data-acquisition software logs test data in, with export to Parquet and
CSV. It is built for test-stand logs: hundreds of channels at about 1 kHz,
a few at 100 kHz and more, in files of several gigabytes made of many small
segments.

```rust,no_run
use buzzard_tdms::TdmsFile;

fn main() -> Result<(), buzzard_tdms::Error> {
    let file = TdmsFile::open("hotfire-0042.tdms")?;
    for channel in file.channels() {
        println!("{} {} x {}", channel.path(), channel.len(), channel.dtype());
    }
    let pressure = file.channel("Chamber", "PT-101").expect("the channel exists");
    let last_second: Vec<f64> = pressure.read(pressure.len() - 1000..pressure.len())?;
    println!("{} values", last_second.len());
    Ok(())
}
```

The crate is at 0.x. A minor release (0.1 to 0.2) may change the API, and a
patch release does not. `build::FileBuilder` writes synthetic files for
tests and is not a general TDMS writer. The reader follows the vendor's
[TDMS File Format Internal Structure](https://www.ni.com/en/support/documentation/supplemental/07/tdms-file-format-internal-structure.html).

The `buzzard` binary of the `buzzard-cli` crate in the same
[repository](https://github.com/jpoehnelt/buzzard) runs the reader:

```sh
buzzard tdms info hotfire-0042.tdms                 # or --format json
buzzard tdms export hotfire-0042.tdms --out slow.parquet --channels Chamber
buzzard tdms export hotfire-0042.tdms --out pt101.csv --format csv --channels Chamber/PT-101
```

## How it reads

`TdmsFile::open` reads the 28-byte lead-in and the metadata of every
segment, and no raw data. Values are read per channel and per range of
samples, and large reads are split over the rayon thread pool.

When a `.tdms_index` file lies beside the data file, the reader takes the
metadata from it after comparing its lead-ins and metadata with the data
file's, byte for byte. It compares every segment of a file of up to 64
segments, and the first, the last and up to 64 evenly spaced segments of a
longer file. A difference in a segment that is not compared goes unnoticed.
The reader ignores an index file that fails the comparison.
`TdmsFile::index_source` says which file it used, and
`OpenOptions::index_file(false)` turns the index file off.

The crate has no `unsafe` code and does not map files into memory. It reads
with positional reads through the `ReadAt` trait, so a file that shrinks
while it is open gives an I/O error.

## What is supported

Supported:

- Segments: all six table-of-contents flags, little-endian and big-endian
  segments (also mixed in one file), versions 4712 and 4713, padding,
  segments without metadata or without raw data, and any number of chunks
  after one lead-in.
- Metadata: incremental object lists, the "new object list" flag, the raw
  data index markers "none" and "same as before", and channels that are
  added, removed, reordered and resized.
- Raw data, contiguous or interleaved: 8-bit to 64-bit integers, 32-bit
  and 64-bit floats, the "float with unit" types, booleans, timestamps,
  complex numbers, and strings (contiguous only).
- Timestamps with their 64 bits of whole seconds and 64 bits of fraction,
  and properties of the file, groups and channels in each of those types.
- Driver raw data ("DAQmx"): format changing scalers of all ten integer
  and float types, digital line scalers, several scalers per channel
  (`Channel::read_scaler`), and chains of linear scales
  (`Channel::read_scaled`).

Partly supported:

- Scaling other than linear. `Channel::scaling` names the scale and does
  not apply it. The raw values are readable.
- Driver raw data. All driver channels of a segment must have the same
  number of values, or none. A timestamp scaler is read as a 16-byte
  timestamp, which no available file confirms.
- A channel of type void (type code 0) that declares no values has length
  0. Void with values is `Error::Unsupported`.
- Text that is not UTF-8 is read with the bad bytes replaced by U+FFFD and
  reported as an anomaly. `Channel::read_string_bytes` returns the bytes.

Not supported:

- Extended-precision floats (type codes 11 and 0x1B) and fixed-point
  numbers (0x4F). Their layout is not published, and a file with values of
  these types fails to open with `Error::Unsupported`.
- Renaming an object through its `name` property (version 4713). The
  object keeps its path.
- A driver channel whose scalers change from one segment to the next, a
  segment that mixes driver buffers and plain raw data, array dimensions
  other than 1, string channels in interleaved segments, and interleaved
  channels of different lengths. All are errors.
- Writing files, the XML-based TDM and TDX formats, and following a file
  while it is being written (the reader takes the file size once, at open).

## Damaged and hostile files

A crashed writer leaves a last segment that is unfinished or cut short. The
reader recovers what is there and lists what it found in
`TdmsFile::anomalies` (at most 1024 are kept). `OpenOptions::strict` turns
any anomaly into an error.

| Found | Result |
| --- | --- |
| The length field of the last segment was never written (all bits set), or the segment ends beyond the end of the file | read to the end of the file |
| The file ends inside a lead-in or inside a segment's metadata | that segment is ignored, and the segments before it are intact |
| Raw data that is not a whole number of chunks, in a segment that was cut short and is contiguous | channels are complete in order until the bytes run out |
| The same, interleaved | the whole rows are kept |
| The same, in driver buffers | the buffers are complete in order until the bytes run out, and each channel keeps the whole rows of the buffers that hold its scalers |
| The same, in a finished contiguous segment | every channel is shortened in proportion (what npTDMS does) |
| The same, with a string channel | the incomplete chunk is skipped |
| Bytes that no channel accounts for, or raw data in a segment whose table of contents says it has none | skipped and reported |

Anything else that contradicts the format is an `Error` with the byte
offset and the segment number. The reader allocates nothing from a count
that a file declares, and a channel is never longer than the bytes the file
has for it. Channel positions that the reader derives beyond what the
metadata spells out count against `OpenOptions::max_layout_entries`
(16,777,216 by default). Past the limit, `open` fails with
`Error::IndexTooLarge`.

## Export

`export::parquet::write_parquet` (feature `parquet`) and `export::write_csv`
write the chosen channels as one table. Each channel is one column named
`group/channel`, or two for a complex channel (`.re` and `.im`). The table
has as many rows as the longest channel, so export channels of different
sample rates separately.

- When every exported channel has the same `wf_start_time`,
  `wf_start_offset` and `wf_increment`, a leading `time_s` column holds
  `wf_start_offset + row * wf_increment`.
- Parquet key-value metadata holds every property of the file, the groups
  and the channels. The rustdoc of `export::parquet` lists the keys.
- A channel with linear scaling is exported in engineering units as 64-bit
  floats unless `ExportOptions::scaled` is off.
- The same input and options give the same bytes.

The `parquet` feature is off by default. Without it the crate depends on
`bytemuck`, `rayon` and `thiserror` only. Compression is Snappy or none.

## Speed

Measured with `tools/tdms-bench.sh 400` from the repository on a 4-core
virtual machine (Xeon at 2.1 GHz). The script generates one file of about
415 MB per shape and reads it from the page cache. Each number is the best
of three reads in one process, and the slower of two runs of the script.
MB/s is file bytes over wall time.

| Shape | Measurement | 1 thread | 4 threads |
| --- | --- | --- | --- |
| Test stand: 300 x f64 at 1 kHz and 4 x f64 at 100 kHz, 74 segments | open | under 1 ms | under 1 ms |
| | read everything | 1,700 MB/s | 3,600 MB/s |
| | read one 100 kHz channel (59 MB) | 23 ms | 31 ms |
| | export to Parquet, Snappy | 440 MB/s | 780 MB/s |
| Many segments: 300 x f64, 174 segments of 1000 samples | read everything | 2,450 MB/s | 3,950 MB/s |
| | export to Parquet, Snappy | 595 MB/s | 1,200 MB/s |
| Interleaved: 300 x f64, one segment of 1000-row chunks | read everything | 1,870 MB/s | 3,070 MB/s |
| | read one channel (1.4 MB of 418 MB of rows) | 61 ms | 17 ms |
| | export to Parquet, Snappy | 585 MB/s | 1,070 MB/s |

`Channel::read` and `Channel::read_into` on an interleaved file walk every
row of the range for one channel. To read many channels of such a file, use
`TdmsFile::read_columns`, which walks the rows once for all of them.

The script also reads each file from a copy in memory, as a memory-mapped
backend would. Positional reads were 6 to 23% slower than that on one
thread and at most 14% slower on four. A file that is not in the page cache
was not measured.

## Tests and license

The tests in the repository read generated files back against the
generator's own model, compare the reader with
[npTDMS](https://github.com/adamreeve/npTDMS), damage and truncate files at
every byte, and read one real measurement file.

This crate is Apache-2.0 (see `LICENSE` and `NOTICE`). npTDMS is LGPL-3.0,
runs only as a separate test program, and none of its code, tests or
fixtures is in this repository. See the
[oracle README](https://github.com/jpoehnelt/buzzard/blob/main/crates/buzzard-tdms/tests/oracle/README.md).

"TDMS", "DAQmx", "LabVIEW" and "NI" name the format and the software that
writes it. buzzard is not affiliated with, endorsed by, or sponsored by National
Instruments Corporation.
