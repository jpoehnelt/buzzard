//! Benchmark of the TDMS reader on files shaped like test-stand logs.
//!
//! ```text
//! cargo run --release -p buzzard-tdms --features parquet --example tdms_bench -- \
//!     --dir target/tdms-bench --size-mb 400 --threads 4
//! ```
//!
//! It writes a file of about `--size-mb` megabytes into `--dir`, times
//! opening it, reading all of it, reading single channels and exporting it
//! to Parquet, prints the best of three runs of each, and deletes what it
//! wrote (`--keep` leaves it). `tools/tdms-bench.sh` runs it for several
//! thread counts.
//!
//! Shapes (`--shape`):
//!
//! - `stand` (default): 300 channels of 64-bit floats at 1 kHz and 4 at
//!   100 kHz, one segment per second of recording, contiguous, metadata in
//!   the first segment only.
//! - `segments`: 300 channels of 1000 samples per segment, contiguous.
//! - `interleaved`: 300 channels in one segment of many interleaved chunks.
//!
//! All times are with the file in the page cache; the reader's speed on a
//! cold file is the speed of the disk.

#![expect(
    clippy::print_stdout,
    reason = "an example that reports to the terminal"
)]
#![expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "sizes and rates for a report"
)]

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use buzzard_tdms::build::{FileBuilder, Object, Values};
use buzzard_tdms::export::parquet::{write_parquet, Compression};
use buzzard_tdms::export::ExportOptions;
use buzzard_tdms::{Channel, OpenOptions, PropertyValue, TdmsFile, Timestamp};

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Stand,
    Segments,
    Interleaved,
}

struct Args {
    dir: PathBuf,
    size_mb: u64,
    threads: Option<usize>,
    shape: Shape,
    keep: bool,
}

fn usage() -> ! {
    panic!(
        "usage: tdms_bench --dir DIR [--size-mb N] [--threads N] \
         [--shape stand|segments|interleaved] [--keep]"
    )
}

fn args() -> Args {
    let mut out = Args {
        dir: PathBuf::new(),
        size_mb: 400,
        threads: None,
        shape: Shape::Stand,
        keep: false,
    };
    let mut words = std::env::args().skip(1);
    while let Some(word) = words.next() {
        let mut value = || words.next().unwrap_or_else(|| usage());
        match word.as_str() {
            "--dir" => out.dir = PathBuf::from(value()),
            "--size-mb" => out.size_mb = value().parse().unwrap_or_else(|_| usage()),
            "--threads" => out.threads = Some(value().parse().unwrap_or_else(|_| usage())),
            "--shape" => {
                out.shape = match value().as_str() {
                    "stand" => Shape::Stand,
                    "segments" => Shape::Segments,
                    "interleaved" => Shape::Interleaved,
                    _ => usage(),
                }
            }
            "--keep" => out.keep = true,
            _ => usage(),
        }
    }
    if out.dir.as_os_str().is_empty() {
        usage();
    }
    out
}

/// Channels of a shape: (group, how many, samples per segment, rate in Hz).
fn groups(shape: Shape) -> Vec<(&'static str, usize, usize, f64)> {
    match shape {
        Shape::Stand => vec![("slow", 300, 1_000, 1e3), ("fast", 4, 100_000, 1e5)],
        Shape::Segments | Shape::Interleaved => vec![("slow", 300, 1_000, 1e3)],
    }
}

/// A sensor-like signal: a slow wave with a little noise.
fn signal(channel: usize, sample: usize, rate: f64, noise: &mut u64) -> f64 {
    *noise ^= *noise << 13;
    *noise ^= *noise >> 7;
    *noise ^= *noise << 17;
    let t = sample as f64 / rate;
    let wave = (t * (1.0 + channel as f64 * 0.01)).sin() * 50.0 + channel as f64;
    wave + (*noise >> 40) as f64 / f64::from(1u32 << 24) * 0.05
}

/// Writes the data file and its index file; returns the data file's size.
fn generate(path: &Path, shape: Shape, size_mb: u64) -> std::io::Result<u64> {
    let groups = groups(shape);
    let segment_bytes: usize = groups.iter().map(|g| g.1 * g.2 * 8).sum();
    let segments = ((size_mb << 20) as usize / segment_bytes).max(2);
    let start = Timestamp::new(3_877_000_000, 0);

    // The first segment's lead-in and metadata come from the builder; its
    // raw data and every later segment are streamed out here.
    let mut builder = FileBuilder::new();
    let first = builder.segment().interleaved(shape == Shape::Interleaved);
    first.object(Object::root().prop("name", PropertyValue::String("bench".into())));
    for &(group, count, samples, rate) in &groups {
        for channel in 0..count {
            first.object(
                Object::channel(group, &format!("ch{channel:03}"))
                    .data(Values::F64(vec![0.0; samples]))
                    .prop("unit_string", PropertyValue::String("bar".into()))
                    .prop("wf_start_time", PropertyValue::Timestamp(start))
                    .prop("wf_start_offset", PropertyValue::F64(0.0))
                    .prop("wf_increment", PropertyValue::F64(1.0 / rate)),
            );
        }
    }
    let built = builder.build();
    let head = usize::try_from(built.data_offsets[0]).expect("a small header");
    let mut lead = built.bytes[..28].to_vec();
    let toc = u32::from_le_bytes([lead[4], lead[5], lead[6], lead[7]]);

    let mut out = BufWriter::with_capacity(1 << 20, File::create(path)?);
    let mut index = Vec::new();
    let mut noise = 0x9E37_79B9_7F4A_7C15_u64;
    let mut data = Vec::with_capacity(segment_bytes);
    let mut written = 0u64;
    for segment in 0..segments {
        data.clear();
        if shape == Shape::Interleaved {
            let (_, count, samples, rate) = groups[0];
            for sample in 0..samples {
                for channel in 0..count {
                    let v = signal(channel, segment * samples + sample, rate, &mut noise);
                    data.extend(v.to_le_bytes());
                }
            }
        } else {
            for &(_, count, samples, rate) in &groups {
                for channel in 0..count {
                    for sample in 0..samples {
                        let v = signal(channel, segment * samples + sample, rate, &mut noise);
                        data.extend(v.to_le_bytes());
                    }
                }
            }
        }
        if shape == Shape::Interleaved {
            // One segment: the lead-in once, then chunk after chunk.
            if segment == 0 {
                let total = (head - 28 + segment_bytes * segments) as u64;
                lead[12..20].copy_from_slice(&total.to_le_bytes());
                out.write_all(&lead)?;
                out.write_all(&built.bytes[28..head])?;
                index.extend(b"TDSh");
                index.extend(&lead[4..]);
                index.extend(&built.bytes[28..head]);
                written += head as u64;
            }
        } else if segment == 0 {
            out.write_all(&built.bytes[..head])?;
            index.extend(&built.index_bytes);
            written += head as u64;
        } else {
            // Raw data only: no metadata, no new object list.
            let mut next = Vec::with_capacity(28);
            next.extend(b"TDSm");
            next.extend((toc & !0b110).to_le_bytes());
            next.extend(4713u32.to_le_bytes());
            next.extend((segment_bytes as u64).to_le_bytes());
            next.extend(0u64.to_le_bytes());
            out.write_all(&next)?;
            index.extend(b"TDSh");
            index.extend(&next[4..]);
            written += 28;
        }
        out.write_all(&data)?;
        written += data.len() as u64;
    }
    out.flush()?;
    let mut index_path = path.as_os_str().to_owned();
    index_path.push("_index");
    std::fs::write(index_path, index)?;
    Ok(written)
}

/// The shortest of three runs.
fn best<T>(mut run: impl FnMut() -> T) -> (Duration, T) {
    let mut out = None;
    for _ in 0..3 {
        let started = Instant::now();
        let value = run();
        let took = started.elapsed();
        if out.as_ref().is_none_or(|(least, _)| took < *least) {
            out = Some((took, value));
        }
    }
    out.expect("three runs")
}

fn report(what: &str, took: Duration, bytes: u64, note: &str) {
    let seconds = took.as_secs_f64();
    let rate = if bytes == 0 {
        String::new()
    } else {
        format!("{:.0} MB/s", bytes as f64 / 1e6 / seconds)
    };
    println!("{what:<36} {:>10.3} ms {rate:>12}  {note}", seconds * 1e3);
}

/// Reads every value of every channel, group by group, in batches.
fn read_all(file: &TdmsFile) -> u64 {
    let mut bytes = 0;
    for group in file.groups() {
        let channels: Vec<Channel<'_>> = group.channels().collect();
        let len = channels.iter().map(Channel::len).min().unwrap_or(0);
        let batch = ((256u64 << 20) / (channels.len() as u64 * 8)).max(1);
        let mut at = 0;
        while at < len {
            let end = len.min(at + batch);
            let columns = file.read_columns(&channels, at..end).expect("read a batch");
            bytes += columns.iter().map(|c| c.len() as u64 * 8).sum::<u64>();
            at = end;
        }
    }
    bytes
}

/// Times the export of every group to a Parquet file of its own.
fn export(args: &Args, file: &TdmsFile, size: u64) {
    for (compression, name) in [(Compression::Snappy, "snappy"), (Compression::None, "none")] {
        let mut outputs = Vec::new();
        let (took, written) = best(|| {
            let mut written = 0;
            for group in file.groups() {
                let out = args.dir.join(format!("bench-{}.parquet", group.name()));
                let channels: Vec<Channel<'_>> = group.channels().collect();
                let target = BufWriter::new(File::create(&out).expect("create the export"));
                write_parquet(
                    file,
                    &channels,
                    target,
                    &ExportOptions::default(),
                    compression,
                )
                .expect("export");
                written += std::fs::metadata(&out).map_or(0, |m| m.len());
                outputs.push(out);
            }
            written
        });
        report(
            &format!("export to Parquet, {name}"),
            took,
            size,
            &format!("{:.1} MB written", written as f64 / 1e6),
        );
        if !args.keep {
            for out in outputs {
                let _ = std::fs::remove_file(out);
            }
        }
    }
}

fn bench(args: &Args) {
    std::fs::create_dir_all(&args.dir).expect("create the directory");
    let path = args.dir.join("bench.tdms");
    let mut index_path = path.as_os_str().to_owned();
    index_path.push("_index");
    let started = Instant::now();
    let size = generate(&path, args.shape, args.size_mb).expect("write the file");
    let shape = match args.shape {
        Shape::Stand => "stand: 300 x f64 at 1 kHz + 4 x f64 at 100 kHz, one segment per second",
        Shape::Segments => "segments: 300 x f64, 1000 samples per segment",
        Shape::Interleaved => "interleaved: 300 x f64, one segment of 1000-row chunks",
    };
    println!("shape {shape}");
    println!(
        "file {:.1} MB, written in {:.2} s; {} threads",
        size as f64 / 1e6,
        started.elapsed().as_secs_f64(),
        rayon::current_num_threads()
    );
    // Once through, so that every run below finds the file in the cache.
    let warm = std::fs::read(&path).expect("read the file back");

    let no_index = OpenOptions::new().index_file(false);
    let (took, file) = best(|| no_index.open(&path).expect("open"));
    let (blocks, layouts) = file.index_shape();
    report(
        "open, from the data file",
        took,
        0,
        &format!(
            "{} segments, {blocks} runs, {layouts} layouts",
            file.segment_count()
        ),
    );
    let (took, indexed) = best(|| TdmsFile::open(&path).expect("open"));
    report(
        "open, from the index file",
        took,
        0,
        &format!("{:?}", indexed.index_source()),
    );

    let (took, bytes) = best(|| read_all(&file));
    report("read everything, positional reads", took, size, "");
    assert!(bytes > size * 9 / 10);
    // The same reads from memory: what a mapped file would cost, since the
    // values are copied out either way and only the system calls differ.
    let in_memory = OpenOptions::new()
        .open_source(Arc::new(warm), None)
        .expect("open from memory");
    let (took, _) = best(|| read_all(&in_memory));
    report(
        "read everything, from memory",
        took,
        size,
        "(as a mapped backend would)",
    );
    drop(in_memory);

    for group in file.groups() {
        let channel = group.channels().nth(1).expect("a second channel");
        let (took, values) = best(|| channel.read_all::<f64>().expect("read a channel"));
        report(
            &format!("read one channel of '{}'", group.name()),
            took,
            values.len() as u64 * 8,
            &format!("{} values", values.len()),
        );
        let (took, _) = best(|| channel.read::<f64>(channel.len() / 2..channel.len() / 2 + 1000));
        report("  1000 values from its middle", took, 8000, "");
    }

    export(args, &file, size);
    if !args.keep {
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(index_path);
    }
}

fn main() {
    let args = args();
    match args.threads {
        Some(threads) => rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("start the thread pool")
            .install(|| bench(&args)),
        None => bench(&args),
    }
}
