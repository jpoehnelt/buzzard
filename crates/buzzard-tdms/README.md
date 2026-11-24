# buzzard-tdms

buzzard-tdms reads TDMS measurement files, the format that data-acquisition
software logs test data in, and exports their channels to Parquet and CSV.
The crate is built for test-stand logs. Such a log has hundreds of channels
at about 1 kHz and a few at 100 kHz and more. Its file is several gigabytes
of many small segments. The crate does not write TDMS files.

## Read a file

To read a file from Rust, open it and read a range of samples from a
channel.

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

To read a file from the command line, use the `buzzard` binary. The
`buzzard-cli` crate of the
[buzzard repository](https://github.com/jpoehnelt/buzzard) builds that
binary.

```sh
buzzard tdms info hotfire-0042.tdms                 # or --format json
buzzard tdms export hotfire-0042.tdms --out slow.parquet --channels Chamber
buzzard tdms export hotfire-0042.tdms --out pt101.csv --format csv --channels Chamber/PT-101
```

An export has as many rows as its longest channel. To export channels of
different sample rates, run one export for each rate.

## How the reader opens a file

`TdmsFile::open` reads the 28-byte lead-in and the metadata of every
segment, and no raw data. You read values per channel and per range of
samples. The reader splits a large read over the rayon thread pool.

When a `.tdms_index` file lies beside the data file, the reader compares
the lead-ins and metadata of the index file with the data file's, byte for
byte. It compares every segment of a file of up to 64 segments. Of a longer
file it compares the first, the last, and up to 64 evenly spaced segments,
so it does not notice a difference in a segment that it skips. If the
comparison passes, the reader takes the metadata from the index file. If
not, it ignores the index file. `TdmsFile::index_source` says which file
the reader used, and `OpenOptions::index_file(false)` turns the index file
off.

The crate has no `unsafe` code and does not map files into memory. The
reader uses positional reads through the `ReadAt` trait, so a file that
shrinks while the reader has it open gives an I/O error.

The reader follows the vendor's
[TDMS File Format Internal Structure](https://www.ni.com/en/support/documentation/supplemental/07/tdms-file-format-internal-structure.html).

## What the reader supports

The reader supports these parts of the format in full:

- Segments: all six table-of-contents flags, little-endian and big-endian
  segments (also mixed in one file), versions 4712 and 4713, padding,
  segments without metadata or without raw data, and any number of chunks
  after one lead-in.
- Metadata: incremental object lists, the "new object list" flag, the raw
  data index markers "none" and "same as before", and channels that are
  added, removed, reordered, and resized.
- Raw data, contiguous or interleaved: 8-bit to 64-bit integers, 32-bit
  and 64-bit floats, the "float with unit" types, booleans, timestamps,
  complex numbers, and strings (contiguous only).
- Timestamps with their 64 bits of whole seconds and 64 bits of fraction,
  and properties of the file, groups, and channels in each of those types.
- Driver raw data ("DAQmx"): format changing scalers of all ten integer
  and float types, digital line scalers, several scalers per channel
  (`Channel::read_scaler`), and chains of linear scales
  (`Channel::read_scaled`).

The reader supports these parts with limits:

- Scaling other than linear. `Channel::scaling` names the scale and does
  not apply it. The raw values are readable.
- Driver raw data. All driver channels of a segment must have the same
  number of values, or none. The reader takes a timestamp scaler to be a
  16-byte timestamp, and no available file confirms that layout.
- A channel of type void (type code 0). A void channel that declares no
  values has length 0. A void channel with values is `Error::Unsupported`.

The reader does not support these parts:

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
  while another program writes it. The reader takes the file size once, at
  open.

## What the reader does with a damaged file

A crashed writer leaves a last segment that is unfinished or cut short. The
reader recovers what is there and lists each anomaly in
`TdmsFile::anomalies`. It keeps at most 1,024 anomalies.
`OpenOptions::strict` turns any anomaly into an error.

In the table, a partial chunk is raw data that is not a whole number of
chunks.

| The file has | The reader |
| --- | --- |
| A last segment whose length field was never written (all bits set), or a segment that ends beyond the end of the file | reads to the end of the file |
| An end inside a lead-in or inside a segment's metadata | ignores that segment. The segments before it are intact |
| A partial chunk in a contiguous segment that was cut short | reads the channels in order until the bytes run out |
| A partial chunk in an interleaved segment that was cut short | keeps the whole rows |
| A partial chunk of driver buffers in a segment that was cut short | reads the buffers in order until the bytes run out. Each channel keeps the whole rows of the buffers that hold its scalers |
| A partial chunk in a finished contiguous segment | shortens every channel in proportion, as npTDMS does |
| A partial chunk in a contiguous segment with a string channel | skips the incomplete chunk |
| Bytes that no channel accounts for, or raw data in a segment whose table of contents says it has none | skips the bytes and reports them |
| A segment version other than 4712 or 4713 | reads the segment and reports the version where it first appears |
| Text that is not UTF-8 | replaces the bad bytes with U+FFFD. `Channel::read_string_bytes` returns the bytes |

Anything else that contradicts the format is an `Error` with the byte
offset and the segment number.

The reader allocates nothing from a count that a file declares, and a
channel is never longer than the bytes the file has for it. Some channel
positions are not spelled out in the metadata, and the reader derives them.
Each derived position counts against `OpenOptions::max_layout_entries`
(16,777,216 by default). Past the limit, `open` fails with
`Error::IndexTooLarge`.

## What an export holds

`export::parquet::write_parquet` (feature `parquet`) and `export::write_csv`
write the chosen channels as one table. Each channel is one column named
`group/channel`, or two for a complex channel (`.re` and `.im`).

- When every exported channel has the same `wf_start_time`,
  `wf_start_offset`, and `wf_increment`, a leading `time_s` column holds
  `wf_start_offset + row * wf_increment`.
- Parquet key-value metadata holds every property of the file, the groups,
  and the channels. The rustdoc of `export::parquet` lists the keys.
- The export writes a channel with linear scaling in engineering units, as
  64-bit floats, unless `ExportOptions::scaled` is off.
- The same input and options give the same bytes.

The `parquet` feature is off by default. Without it the crate depends on
`bytemuck`, `rayon`, and `thiserror` only. Compression is Snappy or none.

`info::to_text` is the report of `buzzard tdms info`. It prints every name
and value from the file on one line, with a control character as its escape.
The `json` feature,
also off by default, adds `info::report` and `info::to_json` for the same
report as JSON, and depends on `serde` and `serde_json`.

## How fast the reader is

The numbers come from `tools/tdms-bench.sh 400`, run from the repository on
a 4-core virtual machine (Xeon at 2.1 GHz). The script generates one file
of about 415 MB per shape and reads it from the page cache. Each number is
the best of three reads in one process, and the slower of two runs of the
script. MB/s is file bytes over wall time.

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

On an interleaved file, `Channel::read` and `Channel::read_into` walk every
row of the range for one channel. `TdmsFile::read_columns` walks the rows
once for all the channels it is given.

The script also reads each file from a copy in memory, as a memory-mapped
backend would. Positional reads were 6 to 23% slower than that on one
thread and at most 14% slower on four. No number here covers a file that is
not in the page cache.

## Stability

The crate is at 0.x. A minor release, such as 0.1 to 0.2, may change the
API. A patch release does not. `build::FileBuilder` writes synthetic files
for tests and is not a general TDMS writer.

The tests in the repository read generated files back against the
generator's own model, compare the reader with
[npTDMS](https://github.com/adamreeve/npTDMS), damage and truncate files at
every byte, and read one real measurement file.

## License

The license is Apache-2.0. The `LICENSE` and `NOTICE` files of this package
hold the texts. npTDMS is LGPL-3.0 and runs only as a separate test
program. None of its code, tests, or fixtures is in this repository. The
[README of the npTDMS oracle](https://github.com/jpoehnelt/buzzard/blob/main/crates/buzzard-tdms/tests/oracle/README.md)
records the boundary.

"TDMS", "DAQmx", "LabVIEW", and "NI" name the format and the software that
writes it. buzzard is not affiliated with, endorsed by, or sponsored by National
Instruments Corporation.
