# buzzard-tdms: measured read performance against npTDMS, tedium, tdms-rs and tdms

Measured 2026-11-16 on the tree of commit `ca0e2e4` (branch `worktree-wf_1f9cfc69-681-3`), copied without `target/` and
built in release mode. Nothing in the worktree or in `/home/user/buzzard` was changed (`git status` clean, copy identical
to the source).

## Verdict

- **buzzard-tdms returned the generated values in every measurement (5 shapes, all operations, 1 and 4 threads), and
  its Parquet exports read back equal with pyarrow.** Two of the other Rust crates returned wrong data without an
  error; see "Correctness problems".
- **Full read into new memory: buzzard-tdms is the fastest reader only with 4 threads, and only by 1.3 to 1.8 times
  over tdms-rs.** With 1 thread it is level with tdms-rs on the many-segment log (A) and **slower than tdms-rs on the
  one-segment file (B, 1.24 s against 1.09 s) and on the int32 file (D, 3.27 s against 2.67 s)**.
- **Every reader's full read is bound by this VM's cost of providing new memory, not by TDMS parsing.** Reading the
  1.44 GB file B with plain `pread` on one thread takes 0.81 s into a new buffer and 0.29 s into a buffer that was
  written to before. buzzard-tdms zero-fills its result columns on one thread before its threads start (0.69 s of a
  0.79 s read of B), which is why 4 threads gain so little and why it loses to tdms-rs on 1 thread. Into buffers
  that already exist (`Channel::read_into`) it equals plain `pread` on B and D, on 1 thread and on 4.
- **Where buzzard-tdms clearly wins:** against npTDMS everywhere (3 to 18 times on full reads in the main runs, 22
  to 240 times on one channel of an interleaved or driver file; npTDMS's own times varied 3 to 4 times between
  sessions, see "Limits"); opening a file of many segments (20 ms against 84 to 401 ms for 600
  segments of 300 channels); interleaved data (C), where tedium is 1.6 to 3 times slower, tdms-rs returns the first chunk
  only and tdms runs out of memory; and driver (DAQmx) raw data (E), which only npTDMS also reads.
- **Small reads (one channel, a one-second window) of contiguous files: no measurable difference between
  buzzard-tdms and tdms-rs** once the file is open; on B tdms-rs reads one channel about 1 ms faster.
- **Parquet export** costs two to three times the 4-thread read (1.6 s for 1.44 GB, 5.7 s for 3.84 GB on 4 threads), and
  the output is never smaller than the TDMS file on this data.

## Addendum: result columns are now filled on every thread

The verdict above says the reader zero-fills its result columns on one thread before the parallel part starts. That
is fixed: a result of 2^20 values or more has its columns filled in parallel. Measured on the same machine with a
1,466 MB file of 300 f64 channels in 611 segments, warm page cache, five alternating rounds.

| Measurement | Before | After |
|---|---|---|
| One full read in a fresh process, 4 threads | 0.82-0.88 s | 0.34-0.40 s |
| One full read in a fresh process, 1 thread | 1.09-1.17 s | 1.09-1.28 s |
| Third full read in one process (`tdms_bench`, best of three), 4 threads | 0.31 s | 0.36-0.38 s |
| Third full read in one process, 1 thread | 0.56-0.60 s | 0.57-0.59 s |

The first read in a process is 2.4 times faster on four threads and is now close to the plain `pread` ceiling measured
above (0.26-0.31 s). A repeated full read in the same process, where the allocator hands back memory that was
already touched, is about 20% slower on four threads than before. The tables above were measured before this change
and have not been re-run.

## Machine

- Firecracker VM, 4 virtual cores of an Intel Xeon at 2.10 GHz (1 thread per core), 16 GB RAM, no swap, Linux 6.18.44.
  One virtio disk, ext4. The memory limit is shared by every process of the session.
- rustc 1.97.0, release profile of the buzzard workspace (`lto = "thin"`, `codegen-units = 1`); the comparison
  binaries use the same profile. parquet 60.1.0, rayon 1.12.0 (as in the workspace `Cargo.lock`).
- Python 3.13.18, numpy 2.5.5, npTDMS 1.14.0, pyarrow 26.0.1 (installed for this run into the research venv).
- tdms 0.3.0, tedium 0.2.0, tdms-rs 2.0.0 from crates.io.
- Transparent huge pages: `madvise`. numpy asks for them, the Rust allocator does not (`AnonHugePages` stayed 0).

**Load.** Before every measurement the runner sampled `/proc/stat` for one second and waited (up to a minute) until
less than 0.25 cores were busy. All 1,269 measurements started with at most 0.19 cores busy; the 1-minute load
average was between 0.1 and 1.9, which is the benchmark's own previous run. **No other heavy process ran during the measurements.** The VM is
still noisy in a way these counters do not show: the cost of first-touch memory varies between sessions (see
"Limits of these numbers").

**Page cache.** Warm for every table entry unless marked cold: `mincore` showed 100% of the file cached before and
after each shape's runs, and after every repetition for C, D and E (not recorded for A and B).

## Method

- One process per measurement, timed inside the process with a monotonic clock; the checksum is computed after the
  timer stops. Python and Rust startup are not counted.
- 5 repetitions of each measurement, readers taking turns (round-robin), so drift favours none. Tables give
  **median (min-max)**. The runner would cut a measurement that takes over 150 s to 2 repetitions; none did.
- Every result is checked: per channel, the wrapping 64-bit sum of the values' bit patterns, combined over channels
  with the channel number as weight, against what the generator wrote, plus the number of values.
- Files: A, B and D are written by npTDMS's `TdmsWriter` (with its index file), C by hand (npTDMS cannot write
  interleaved data), E by buzzard-tdms's own builder (`buzzard_tdms::build`), because no other writer of driver raw
  data was available. Before timing, each file was read with npTDMS from the data file alone and every channel
  compared with the generated values. Values are sensor-like: a sine per channel plus noise (A to C: f64 with
  full-mantissa noise; D: int32, about 22 bits of signal and 12 of noise; E: int16).
- Sizes are as requested: nothing was scaled down. Disk: 14 GB free at the start; at most one data file plus one
  Parquet export existed at a time (9.8 GB free with D on disk; about 5.5 GB during its uncompressed export, by
  arithmetic).
- Readers were pointed at a hard link of the data file without an index file beside it, so that none reads the
  index; "open through the index file" is measured separately.
- **Full read** = open + every value of every channel in memory at once, with each reader's API for it:
  buzzard-tdms `TdmsFile::read_columns(all channels, 0..len)`; npTDMS `TdmsFile.read`; tedium `read_channels_from`
  into `vec![0; n]` buffers; tdms-rs `TdmsChannel::read` per channel into `vec![0; n]`; tdms `collect()` of each
  channel's iterator. MB/s is file size / (open + read).
- **One channel** = the middle channel, whole. **1 s window** = one second of samples starting half a segment after
  the middle of the file, so it spans two segments (A to C: 1,000 samples; D and E: 100,000). These exclude the
  open time, which has its own column. tdms has no random access; its window is `skip(n).take(len)`.
- **Ceiling** = the whole file into one new `Vec<u8>` with `pread` in 8 MiB pieces, on 1 thread and split over 4.
- On shape E the full read, one channel and the windows are the raw 16-bit values (npTDMS: `raw_data`,
  `read_data(scaled=False)`); "engineering units" is the scaled read (`read_scaled`; npTDMS `channel[:]`).
- **Supplementary tables** (a later session, files regenerated): the full read again, and the same into buffers
  that were allocated and written to before the timer started (buzzard-tdms `Channel::read_into` per channel with
  the channels spread over rayon; tedium and tdms-rs as above; `pread` into a written buffer). On C this is not
  buzzard-tdms's fast path; see "Reading the numbers".
- buzzard-tdms "1t" and "4t" are the size of the rayon pool.
- Each measured process had a 10 GB address-space limit (added after the first attempt at C; see below).

## Commands

All paths are under `tdms-bench`.

```sh
# the tree and the binaries
tar --exclude=./target --exclude=./.git -C <worktree> -cf - . | tar -C buzzard -xf -
(cd bzbench && cargo build --release)     # buzzard-tdms from ../buzzard, feature parquet; Cargo.lock copied from buzzard
(cd others  && cargo build --release)     # tdms =0.3.0, tedium =0.2.0, tdms-rs =2.0.0

# everything: generate, verify with npTDMS, time, verify the Parquet export, cold runs, delete
sh scripts/run_all.sh "A B C D E"         # one shape: ../tdms-research/venv/bin/python -I scripts/run.py A 1.0 5
# full read with and without page faults for the destination (shapes A to D)
../tdms-research/venv/bin/python -I scripts/sup.py A 5
# tables and results.json
python3 scripts/make_report.py > tables.md
cat scripts/head.md tables.md scripts/tail.md > RESULTS.md      # head and tail are written by hand

# one measurement by hand: FILE OP DTYPE NCH CHAN WIN_START WIN_LEN [THREADS [OUT]]
bzbench/target/release/bzbench data/a_noindex.tdms all f64 300 150 300500 1000 4
others/target/release/b_tdmsrs data/a_noindex.tdms all f64 300 150 300500 1000
../tdms-research/venv/bin/python -I scripts/np_bench.py data/a_noindex.tdms all f64 300 150 300500 1000
```

Files: `scripts/gen.py` (generator), `bzbench/src/bin/gen_daqmx.rs` (shape E), `scripts/run.py`, `scripts/sup.py`,
`scripts/np_bench.py`, `scripts/pq_check.py`, `bzbench/src/main.rs`, `others/src/bin/b_{tedium,tdmsrs,tdms}.rs`,
`results/*.json` (every run, with load, CPU time and peak memory), `results.json` (merged), `logs/`.

## Results

Times are median (min-max) of 5 runs, warm page cache.

### A. 300 x f64 x 600,000, contiguous, 600 segments of 1,000 samples

File: 1447.2 MB, 300 channels x 600,000 samples, written by npTDMS TdmsWriter; npTDMS reads it back equal to the generated values: yes. Page cache: 1.0 of the file cached before the runs, 1.0 after; after each repetition: not recorded. Runs that started with more than 0.25 cores busy elsewhere: 0 of 214.

| Reader | Open, ms | Full read (open + all channels), s | One channel, ms | 1 s window, 1 channel, ms | 1 s window, all channels, ms | Peak memory of full read, MB |
|---|---|---|---|---|---|---|
| pread-1t (ceiling) | - | 0.838 (0.817-0.909), 1728 MB/s | - | - | - | 1384 |
| pread-4t (ceiling) | - | 0.285 (0.259-0.309), 5070 MB/s | - | - | - | 1384 |
| buzzard-1t | 19.7 (18.4-32.6) | 1.36 (1.30-1.41), 1062 MB/s | 4.60 (4.21-5.18) | 0.012 (0.009-0.017) | 2.66 (2.17-3.11) | 1378 |
| buzzard-4t | 21.3 (18.5-31.9) | 0.809 (0.782-0.836), 1789 MB/s | 4.10 (3.89-4.51) | 0.018 (0.009-0.056) | 1.72 (1.56-1.95) | 1378 |
| npTDMS | 401 (396-463) | 4.33 (4.14-4.87), 334 MB/s | 26.9 (25.3-37.4) | 0.948 (0.875-1.10) | 103 (95.3-132) | 1438 |
| tedium | 111 (103-112) | 2.01 (1.90-2.10), 720 MB/s | 30.4 (29.5-33.7) | 0.168 (0.138-0.204) | 7.62 (7.58-9.76) | 1392 |
| tdms-rs | 83.7 (81.1-90.6) | 1.44 (1.32-1.52), 1006 MB/s | 3.97 (3.62-4.75) | 0.031 (0.024-0.049) | 3.29 (2.93-4.07) | 1380 |
| tdms | 143 (136-180) | 23.9 (23.8-24.7), 61 MB/s | 78.4 (77.1-89.4) | 36.3 (35.7-38.2) | 11222 (11084-11411) | 1504 |

| Further measurements | Median (min-max) |
|---|---|
| buzzard-1t: open through the index file, ms | 17.0 (16.7-18.6) |
| buzzard-1t: full read in batches of 256 MiB, s | 1.38 (1.27-1.45), 1051 MB/s |
| buzzard-1t: Parquet export, Snappy, s | 3.21 (3.20-3.41), 451 MB/s; output 1443.4 MB (100% of the TDMS file) |
| buzzard-1t: Parquet export, uncompressed, s | 2.43 (2.35-2.71), 595 MB/s; output 1445.6 MB (100% of the TDMS file) |
| buzzard-4t: open through the index file, ms | 17.3 (16.5-29.7) |
| buzzard-4t: full read in batches of 256 MiB, s | 1.04 (1.01-1.05), 1393 MB/s |
| buzzard-4t: Parquet export, Snappy, s | 1.58 (1.51-1.62), 915 MB/s; output 1443.4 MB (100% of the TDMS file) |
| buzzard-4t: Parquet export, uncompressed, s | 1.37 (1.31-1.65), 1054 MB/s; output 1445.6 MB (100% of the TDMS file) |
| npTDMS: open with the index file beside the data file, ms | 428 (398-439) |
| npTDMS: `TdmsFile.open` + `channel[:]` for every channel, s | 8.61 (8.36-8.73), 168 MB/s |
| cold cache, one run: pread-1t whole file, s | 3.41, 424 MB/s |
| cold cache, one run: buzzard-4t full read, s | 1.74, 830 MB/s |
| cold cache, one run: buzzard-1t full read, s | 2.17, 667 MB/s |
| cold cache, one run: npTDMS full read, s | 4.15, 348 MB/s |
| Parquet export read back with pyarrow and compared | equal: 600000 rows, 301 columns (double), 11 row groups |

CPU seconds of the whole process for the full read, total/in the kernel (median): pread-1t 1.62/0.88; pread-4t 1.73/1.10; buzzard-1t 1.56/1.29; buzzard-4t 1.42/1.19; npTDMS 5.01/2.85; tedium 2.24/1.04; tdms-rs 1.64/1.27; tdms 23.96/11.68.

Supplementary run (2026-11-16 11:45:51, separate session, file regenerated; page cache 1.0 before, 1.0 after): the full read with and without page faults for the destination memory.

| Reader | New memory inside the timer, s | Memory written to before the timer, s |
|---|---|---|
| pread-1t (ceiling) | 0.831 (0.803-0.883), 1741 MB/s | 0.339 (0.292-0.366), 4271 MB/s |
| pread-4t (ceiling) | 0.263 (0.250-0.284), 5494 MB/s | 0.092 (0.089-0.097), 15673 MB/s |
| buzzard-1t | 1.18 (1.16-1.29), 1232 MB/s | 0.531 (0.508-0.558), 2725 MB/s |
| buzzard-4t | 0.830 (0.786-0.898), 1743 MB/s | 0.166 (0.159-0.167), 8715 MB/s |
| npTDMS | 6.06 (4.95-7.85), 239 MB/s | - |
| tedium | - | 1.37 (1.31-1.51), 1057 MB/s |
| tdms-rs | 1.33 (1.24-1.39), 1086 MB/s | 0.694 (0.688-0.755), 2086 MB/s |

### B. The same values in one segment

File: 1440.0 MB, 300 channels x 600,000 samples, written by npTDMS TdmsWriter; npTDMS reads it back equal to the generated values: yes. Page cache: 1.0 of the file cached before the runs, 1.0 after; after each repetition: not recorded. Runs that started with more than 0.25 cores busy elsewhere: 0 of 214.

| Reader | Open, ms | Full read (open + all channels), s | One channel, ms | 1 s window, 1 channel, ms | 1 s window, all channels, ms | Peak memory of full read, MB |
|---|---|---|---|---|---|---|
| pread-1t (ceiling) | - | 0.814 (0.773-1.07), 1768 MB/s | - | - | - | 1377 |
| pread-4t (ceiling) | - | 0.300 (0.274-0.330), 4802 MB/s | - | - | - | 1377 |
| buzzard-1t | 0.634 (0.539-0.810) | 1.24 (1.18-1.28), 1160 MB/s | 4.19 (3.48-6.82) | 0.009 (0.008-0.061) | 2.11 (2.02-11.6) | 1378 |
| buzzard-4t | 0.570 (0.481-0.677) | 0.794 (0.767-0.857), 1813 MB/s | 3.45 (3.10-3.72) | 0.011 (0.008-0.064) | 1.88 (1.55-4.41) | 1378 |
| npTDMS | 4.71 (3.61-5.72) | 5.34 (4.38-10.4), 270 MB/s | 20.3 (5.23-29.6) | 5.42 (2.84-8.33) | 364 (335-397) | 2781 |
| tedium | 0.628 (0.580-0.874) | 1.84 (1.74-1.97), 783 MB/s | 7.00 (4.98-8.04) | 0.050 (0.049-0.050) | 3.08 (2.87-3.76) | 1376 |
| tdms-rs | 0.495 (0.469-0.790) | 1.09 (1.01-1.25), 1323 MB/s | 2.89 (2.65-3.13) | 0.008 (0.007-0.013) | 2.10 (1.98-2.27) | 1376 |
| tdms | 0.505 (0.483-0.755) | 23.3 (22.9-23.7), 62 MB/s | 79.1 (74.5-84.3) | 37.5 (35.5-46.3) | 10818 (10600-10996) | 1376 |

| Further measurements | Median (min-max) |
|---|---|
| buzzard-1t: open through the index file, ms | 0.727 (0.593-0.939) |
| buzzard-1t: full read in batches of 256 MiB, s | 1.29 (1.17-1.41), 1118 MB/s |
| buzzard-1t: Parquet export, Snappy, s | 3.25 (3.16-3.52), 443 MB/s; output 1443.4 MB (100% of the TDMS file) |
| buzzard-1t: Parquet export, uncompressed, s | 2.38 (2.36-2.40), 604 MB/s; output 1445.6 MB (100% of the TDMS file) |
| buzzard-4t: open through the index file, ms | 0.614 (0.594-0.797) |
| buzzard-4t: full read in batches of 256 MiB, s | 1.01 (0.939-1.12), 1423 MB/s |
| buzzard-4t: Parquet export, Snappy, s | 1.57 (1.51-1.66), 916 MB/s; output 1443.4 MB (100% of the TDMS file) |
| buzzard-4t: Parquet export, uncompressed, s | 1.69 (1.27-1.81), 850 MB/s; output 1445.6 MB (100% of the TDMS file) |
| npTDMS: open with the index file beside the data file, ms | 3.90 (3.84-4.19) |
| npTDMS: `TdmsFile.open` + `channel[:]` for every channel, s | 3.11 (2.73-3.47), 464 MB/s |
| cold cache, one run: pread-1t whole file, s | 4.74, 304 MB/s |
| cold cache, one run: buzzard-4t full read, s | 1.33, 1079 MB/s |
| cold cache, one run: buzzard-1t full read, s | 1.57, 918 MB/s |
| cold cache, one run: npTDMS full read, s | 4.28, 337 MB/s |
| Parquet export read back with pyarrow and compared | equal: 600000 rows, 301 columns (double), 11 row groups |

CPU seconds of the whole process for the full read, total/in the kernel (median): pread-1t 1.68/0.90; pread-4t 1.95/1.21; buzzard-1t 1.48/1.23; buzzard-4t 1.26/1.00; npTDMS 5.96/5.12; tedium 2.08/1.04; tdms-rs 1.31/1.15; tdms 23.40/11.46.

Supplementary run (2026-11-16 11:48:56, separate session, file regenerated; page cache 1.0 before, 1.0 after): the full read with and without page faults for the destination memory.

| Reader | New memory inside the timer, s | Memory written to before the timer, s |
|---|---|---|
| pread-1t (ceiling) | 0.815 (0.763-0.942), 1768 MB/s | 0.287 (0.271-0.359), 5012 MB/s |
| pread-4t (ceiling) | 0.576 (0.407-0.705), 2499 MB/s | 0.077 (0.074-0.090), 18795 MB/s |
| buzzard-1t | 1.04 (0.969-1.13), 1384 MB/s | 0.275 (0.266-0.278), 5228 MB/s |
| buzzard-4t | 0.824 (0.774-0.914), 1747 MB/s | 0.075 (0.071-0.087), 19202 MB/s |
| npTDMS | 7.04 (6.83-7.59), 205 MB/s | - |
| tedium | - | 1.09 (1.03-1.34), 1317 MB/s |
| tdms-rs | 0.854 (0.791-0.951), 1686 MB/s | 0.276 (0.255-0.304), 5218 MB/s |

### C. The same values interleaved (one lead-in, 600 chunks of 1,000 rows)

File: 1440.0 MB, 300 channels x 600,000 samples, written by hand (gen.py); npTDMS reads it back equal to the generated values: yes. Page cache: 1.0 of the file cached before the runs, 1.0 after; after each repetition: [1.0, 1.0, 1.0, 1.0]. Runs that started with more than 0.25 cores busy elsewhere: 0 of 188.

| Reader | Open, ms | Full read (open + all channels), s | One channel, ms | 1 s window, 1 channel, ms | 1 s window, all channels, ms | Peak memory of full read, MB |
|---|---|---|---|---|---|---|
| pread-1t (ceiling) | - | 1.03 (1.01-1.27), 1392 MB/s | - | - | - | 1377 |
| pread-4t (ceiling) | - | 0.324 (0.246-0.414), 4446 MB/s | - | - | - | 1377 |
| buzzard-1t | 0.411 (0.389-0.480) | 1.71 (1.36-1.78), 841 MB/s | 272 (237-327) | 1.37 (1.33-1.86) | 3.55 (3.20-4.49) | 1379 |
| buzzard-4t | 0.423 (0.404-0.457) | 0.882 (0.821-0.922), 1632 MB/s | 64.0 (60.4-72.9) | 1.33 (1.21-2.41) | 2.80 (2.48-3.37) | 1383 |
| npTDMS | 3.89 (3.31-4.84) | 16.0 (9.73-18.6), 90 MB/s | 15153 (10481-21710) | 19.0 (14.4-23.9) | 3681 (3658-4343) | 2790 |
| tedium | 0.708 (0.489-0.805) | 2.77 (2.65-3.20), 520 MB/s | 4146 (4087-4346) | 6.30 (6.26-7.93) | 7.60 (6.69-9.64) | 1376 |
| tdms-rs | 0.465 (0.397-0.585) | **WRONG DATA**: 300,000 of 180,000,000 values, no error | **WRONG DATA**: 1,000 of 600,000 values, no error | FAILS: read: InvalidRange(300500, 301500, 1000) | FAILS: read: InvalidRange(300500, 301500, 1000) | 11 |
| tdms | FAILS: aborts: allocation fails at the 10 GB limit (growing a Vec of ChannelPositions) | cannot open the file | - | - | - | - |

| Further measurements | Median (min-max) |
|---|---|
| buzzard-1t: open through the index file, ms | 0.524 (0.472-0.591) |
| buzzard-1t: full read in batches of 256 MiB, s | 1.73 (1.63-1.89), 833 MB/s |
| buzzard-1t: Parquet export, Snappy, s | 3.88 (3.70-4.05), 371 MB/s; output 1443.4 MB (100% of the TDMS file) |
| buzzard-1t: Parquet export, uncompressed, s | 3.00 (2.86-3.12), 480 MB/s; output 1445.6 MB (100% of the TDMS file) |
| buzzard-4t: open through the index file, ms | 0.483 (0.437-0.565) |
| buzzard-4t: full read in batches of 256 MiB, s | 1.10 (1.06-1.15), 1311 MB/s |
| buzzard-4t: Parquet export, Snappy, s | 2.05 (1.75-2.15), 703 MB/s; output 1443.4 MB (100% of the TDMS file) |
| buzzard-4t: Parquet export, uncompressed, s | 1.75 (1.65-1.78), 822 MB/s; output 1445.6 MB (100% of the TDMS file) |
| npTDMS: open with the index file beside the data file, ms | 4.75 (3.47-5.28) |
| npTDMS: `TdmsFile.open` + `channel[:]` for every channel | not run: one channel takes as long as the whole file (see the table), so about an hour per repetition |
| cold cache, one run: pread-1t whole file, s | 9.04, 159 MB/s |
| cold cache, one run: buzzard-4t full read, s | 1.33, 1085 MB/s |
| cold cache, one run: buzzard-1t full read, s | 1.72, 836 MB/s |
| cold cache, one run: npTDMS full read, s | 15.0, 96 MB/s |
| Parquet export read back with pyarrow and compared | equal: 600000 rows, 301 columns (double), 11 row groups |

CPU seconds of the whole process for the full read, total/in the kernel (median): pread-1t 1.83/1.13; pread-4t 1.94/1.15; buzzard-1t 1.93/1.14; buzzard-4t 1.64/0.93; npTDMS 16.65/6.83; tedium 3.13/1.30; tdms-rs 0.01/0.00.

Supplementary run (2026-11-16 11:51:54, separate session, file regenerated; page cache 1.0 before, 1.0 after): the full read with and without page faults for the destination memory.

| Reader | New memory inside the timer, s | Memory written to before the timer, s |
|---|---|---|
| pread-1t (ceiling) | 1.16 (1.08-1.23), 1244 MB/s | 0.512 (0.263-0.553), 2811 MB/s |
| pread-4t (ceiling) | 0.410 (0.339-0.467), 3515 MB/s | 0.089 (0.082-0.116), 16259 MB/s |
| buzzard-1t | 1.45 (1.36-1.51), 994 MB/s | 74.9 (68.0-78.0), 19 MB/s |
| buzzard-4t | 0.944 (0.886-1.00), 1525 MB/s | 18.8 (17.6-19.5), 77 MB/s |
| npTDMS | 25.3 (22.4-27.7), 57 MB/s | - |
| tedium | - | 2.31 (2.01-2.43), 624 MB/s |
| tdms-rs | **WRONG DATA**: 300,000 of 180,000,000 values, no error | **WRONG DATA**: 300,000 of 180,000,000 values, no error |

### D. 16 x i32 x 60,000,000, contiguous, 600 segments of 100,000 samples

File: 3840.4 MB, 16 channels x 60,000,000 samples, written by npTDMS TdmsWriter; npTDMS reads it back equal to the generated values: yes. Page cache: 1.0 of the file cached before the runs, 1.0 after; after each repetition: [1.0, 1.0, 1.0, 1.0]. Runs that started with more than 0.25 cores busy elsewhere: 0 of 214.

| Reader | Open, ms | Full read (open + all channels), s | One channel, ms | 1 s window, 1 channel, ms | 1 s window, all channels, ms | Peak memory of full read, MB |
|---|---|---|---|---|---|---|
| pread-1t (ceiling) | - | 2.56 (2.52-2.76), 1499 MB/s | - | - | - | 3666 |
| pread-4t (ceiling) | - | 0.795 (0.739-0.917), 4829 MB/s | - | - | - | 3666 |
| buzzard-1t | 5.85 (3.58-9.58) | 3.27 (3.12-3.41), 1176 MB/s | 174 (159-189) | 0.277 (0.248-0.282) | 4.58 (4.21-22.2) | 3666 |
| buzzard-4t | 2.52 (2.38-5.07) | 2.04 (1.94-2.10), 1887 MB/s | 127 (120-149) | 0.279 (0.251-0.294) | 4.16 (3.61-4.50) | 3666 |
| npTDMS | 31.2 (30.5-49.1) | 11.0 (9.36-13.5), 349 MB/s | 779 (498-999) | 1.34 (1.31-1.90) | 18.8 (16.4-20.0) | 4154 |
| tedium | 4.55 (4.42-6.19) | 7.05 (6.83-7.23), 545 MB/s | 433 (368-464) | 0.753 (0.670-1.01) | 11.6 (10.1-14.0) | 3665 |
| tdms-rs | 7.64 (6.16-8.79) | 2.67 (2.46-2.95), 1437 MB/s | 153 (139-180) | 0.277 (0.241-0.636) | 4.98 (4.26-5.02) | 3665 |
| tdms | 11.3 (10.3-13.8) | 118 (116-122), 33 MB/s | 7525 (7275-7655) | 3628 (3542-3776) | 57920 (57207-59578) | 3674 |

| Further measurements | Median (min-max) |
|---|---|
| buzzard-1t: open through the index file, ms | 1.43 (1.36-2.28) |
| buzzard-1t: full read in batches of 256 MiB, s | 2.03 (1.98-2.04), 1896 MB/s |
| buzzard-1t: Parquet export, Snappy, s | 11.1 (10.9-11.6), 347 MB/s; output 4225.7 MB (110% of the TDMS file) |
| buzzard-1t: Parquet export, uncompressed, s | 9.43 (8.69-10.3), 407 MB/s; output 4322.0 MB (113% of the TDMS file) |
| buzzard-4t: open through the index file, ms | 2.08 (1.44-2.36) |
| buzzard-4t: full read in batches of 256 MiB, s | 1.57 (1.48-1.59), 2454 MB/s |
| buzzard-4t: Parquet export, Snappy, s | 5.69 (4.84-6.55), 675 MB/s; output 4225.7 MB (110% of the TDMS file) |
| buzzard-4t: Parquet export, uncompressed, s | 5.47 (5.35-6.33), 703 MB/s; output 4322.0 MB (113% of the TDMS file) |
| npTDMS: open with the index file beside the data file, ms | 33.3 (28.0-37.5) |
| npTDMS: `TdmsFile.open` + `channel[:]` for every channel, s | 11.3 (9.99-13.3), 340 MB/s |
| cold cache, one run: pread-1t whole file, s | 20.1, 191 MB/s |
| cold cache, one run: buzzard-4t full read, s | 6.01, 639 MB/s |
| cold cache, one run: buzzard-1t full read, s | 6.15, 624 MB/s |
| cold cache, one run: npTDMS full read, s | 15.2, 253 MB/s |
| Parquet export read back with pyarrow and compared | equal: 60000000 rows, 17 columns (double, int32), 33 row groups |

CPU seconds of the whole process for the full read, total/in the kernel (median): pread-1t 4.57/2.64; pread-4t 4.76/3.17; buzzard-1t 4.02/3.19; buzzard-4t 3.34/2.62; npTDMS 14.43/11.36; tedium 8.00/2.80; tdms-rs 3.63/2.78; tdms 117.76/57.49.

Supplementary run (2026-11-16 12:04:22, separate session, file regenerated; page cache 1.0 before, 1.0 after): the full read with and without page faults for the destination memory.

| Reader | New memory inside the timer, s | Memory written to before the timer, s |
|---|---|---|
| pread-1t (ceiling) | 2.53 (2.49-2.98), 1520 MB/s | 1.24 (0.820-1.46), 3102 MB/s |
| pread-4t (ceiling) | 0.817 (0.786-0.871), 4703 MB/s | 0.255 (0.227-0.307), 15056 MB/s |
| buzzard-1t | 2.82 (2.58-2.91), 1363 MB/s | 0.804 (0.770-0.870), 4775 MB/s |
| buzzard-4t | 2.20 (2.01-2.37), 1744 MB/s | 0.214 (0.208-0.230), 17937 MB/s |
| npTDMS | 16.9 (15.8-21.6), 227 MB/s | - |
| tedium | - | 5.22 (4.71-7.37), 736 MB/s |
| tdms-rs | 2.26 (2.25-2.47), 1700 MB/s | 0.811 (0.784-0.853), 4736 MB/s |

### E. 16 x i16 x 12,500,000, driver (DAQmx) raw data, one segment of 125 chunks

File: 400.0 MB, 16 channels x 12,500,000 samples, written by the crate's builder; npTDMS reads it back equal to the generated values: yes. Page cache: 1.0 of the file cached before the runs, 1.0 after; after each repetition: [1.0, 1.0, 1.0, 1.0]. Runs that started with more than 0.25 cores busy elsewhere: 0 of 199.

| Reader | Open, ms | Full read (open + all channels), s | One channel, ms | 1 s window, 1 channel, ms | 1 s window, all channels, ms | Peak memory of full read, MB |
|---|---|---|---|---|---|---|
| pread-1t (ceiling) | - | 0.253 (0.236-0.305), 1584 MB/s | - | - | - | 385 |
| pread-4t (ceiling) | - | 0.074 (0.072-0.088), 5421 MB/s | - | - | - | 385 |
| buzzard-1t | 0.129 (0.102-0.158) | 0.363 (0.355-0.408), 1103 MB/s | 86.7 (77.0-107) | 2.03 (1.52-2.31) | 4.39 (4.15-4.65) | 387 |
| buzzard-4t | 0.117 (0.112-0.223) | 0.255 (0.230-0.272), 1568 MB/s | 35.1 (31.3-36.7) | 1.82 (1.50-2.34) | 3.52 (2.97-3.86) | 390 |
| npTDMS | 0.726 (0.614-0.936) | 3.43 (3.17-3.95), 116 MB/s | 1947 (1777-2023) | 19.4 (16.3-37.0) | 258 (255-276) | 510 |
| tedium | FAILS: open: StringFormatError(FromUtf8Error ...) | cannot open the file | - | - | - | - |
| tdms-rs | FAILS: open: NotImplemented( DataType 4294967295 ) | cannot open the file | - | - | - | - |
| tdms | 0.100 (0.095-0.108) | **WRONG DATA**: 0 of 200,000,000 values, no error | **WRONG DATA**: 0 of 12,500,000 values, no error | **WRONG DATA**: 0 of 100,000 values, no error | **WRONG DATA**: 0 of 1,600,000 values, no error | 11 |

| Further measurements | Median (min-max) |
|---|---|
| buzzard-1t: open through the index file, ms | 0.153 (0.134-0.160) |
| buzzard-1t: full read in batches of 256 MiB, s | 0.498 (0.472-0.561), 803 MB/s |
| buzzard-1t: full read in engineering units (f64), s | 2.28 (2.09-2.34), 176 MB/s |
| buzzard-1t: Parquet export, Snappy, s | 6.80 (6.64-7.09), 59 MB/s; output 945.2 MB (236% of the TDMS file) |
| buzzard-1t: Parquet export, uncompressed, s | 3.51 (3.33-3.62), 114 MB/s; output 1700.4 MB (425% of the TDMS file) |
| buzzard-4t: open through the index file, ms | 0.134 (0.113-0.169) |
| buzzard-4t: full read in batches of 256 MiB, s | 0.369 (0.344-0.393), 1084 MB/s |
| buzzard-4t: full read in engineering units (f64), s | 0.662 (0.609-0.684), 604 MB/s |
| buzzard-4t: Parquet export, Snappy, s | 3.03 (2.88-3.27), 132 MB/s; output 945.2 MB (236% of the TDMS file) |
| buzzard-4t: Parquet export, uncompressed, s | 2.09 (1.91-2.42), 192 MB/s; output 1700.4 MB (425% of the TDMS file) |
| npTDMS: open with the index file beside the data file, ms | 0.715 (0.691-0.894) |
| npTDMS: full read in engineering units (f64), s | 8.28 (8.12-8.63), 48 MB/s |
| npTDMS: `TdmsFile.open` + `channel[:]` for every channel, s | 31.1 (30.9-31.7), 13 MB/s |
| cold cache, one run: pread-1t whole file, s | 0.271, 1479 MB/s |
| cold cache, one run: buzzard-4t full read, s | 0.353, 1132 MB/s |
| cold cache, one run: buzzard-1t full read, s | 0.496, 807 MB/s |
| cold cache, one run: npTDMS full read, s | 3.17, 126 MB/s |
| Parquet export read back with pyarrow and compared | equal: 12500000 rows, 17 columns (double), 13 row groups |

CPU seconds of the whole process for the full read, total/in the kernel (median): pread-1t 0.48/0.27; pread-4t 0.47/0.29; buzzard-1t 0.48/0.25; buzzard-4t 0.55/0.24; npTDMS 4.83/2.23; tdms 0.00/0.00.

## Correctness problems

A reader that returns other data matters more than its speed, so every case is listed.

1. **tdms-rs 2.0.0 returns the first chunk only of the interleaved file (C), without an error.** Every channel has
   `len()` 1,000 instead of 600,000; the full read returns 300,000 of 180,000,000 values in 8 ms and reports success;
   a window from the middle fails with `InvalidRange(300500, 301500, 1000)`. The segment is one lead-in followed by
   600 chunks, which the TDMS format allows and npTDMS, tedium and buzzard-tdms read in full.
2. **tdms 0.3.0 returns no values for driver (DAQmx) raw channels (E), without an error.** It opens the file, lists
   the 16 channels, and each channel's iterator ends at once: 0 of 200,000,000 values. Its README says such
   iterators do not exist, but nothing in the call reports it.
3. **tdms 0.3.0 cannot open the interleaved file (C): it runs out of memory.** In the first attempt it grew to
   13.6 GB of resident memory and the kernel killed it, seven times, before I stopped the run. `dmesg` shows that
   only `b_tdms` processes were killed. The whole of C was then measured again with a 10 GB address-space limit per
   process, under which tdms aborts in `Vec<tdms::segment::ChannelPositions>::grow_one`. The numbers of the first
   attempt are kept in `results/c_aborted_first_attempt.json` and are not used.
4. **tedium 0.2.0 and tdms-rs 2.0.0 cannot open the driver file (E)**: tedium with
   `StringFormatError(FromUtf8Error ...)`, tdms-rs with `NotImplemented( DataType 4294967295 )`. Both are errors,
   not wrong data.
5. **buzzard-tdms: no difference found.** Every operation on every shape returned the generated values, with 1 and
   4 threads, and the Snappy Parquet export of each shape read back equal with pyarrow (all values, row count, and
   the `time_s` column). Two things to know about that statement:
   - Shape E was written by buzzard-tdms's own builder, so there the check is agreement with npTDMS, not with an
     independent writer.
   - The checksum is a sum per channel: it catches lost, repeated or foreign values and swapped channels, not two
     values of one channel that changed places.
6. **npTDMS 1.14.0: no difference found** on any shape.
7. An open question of the crate's report is answered as a side effect: **buzzard-tdms accepts the index files
   that npTDMS's writer produces** (A, B, D: `index_source` is `IndexFile`, and the values read through it are
   equal).

## Reading the numbers

**What limits a full read.** A full read hands back 1.4 to 3.8 GB of new memory. On this VM the kernel's work of
providing that memory (page faults on 4 kB pages) costs more than copying the data into it: plain `pread` of B runs
at 1.8 GB/s into a new buffer and at 5.0 GB/s into one that was written to before (4 threads: 2.5 to 4.8 GB/s
against 18.8 GB/s). About 80% of the CPU time of buzzard-tdms and tdms-rs is kernel time (A: 1.29 of 1.56 s and
1.27 of 1.64 s). buzzard-tdms, tdms-rs and the 1-thread ceiling all take 0.8 to 1.4 s for 1.44 GB, and the
differences between them are mostly differences in how they acquire memory.

**Where buzzard-tdms is not faster, and why.**

- **1 thread, contiguous data, new memory: tdms-rs is as fast (A) or faster (B: 1.09 s against 1.24 s; D: 2.67 s
  against 3.27 s).** `read_columns` allocates each column and fills it with zeros (`Vec::resize`) before reading, so
  every page is written twice: once by the zero fill, once by the read. tdms-rs's caller uses `vec![0; n]`, which
  the allocator hands out untouched, so each page is written once. Measured: allocating B's 300 columns the way
  `read_columns` does takes 0.69 s (0.66 to 0.73, 5 runs) of the 0.95 to 1.24 s. With buffers that exist already,
  the two are equal (B 0.275 s and 0.276 s; D 0.80 s and 0.81 s), and on the many-segment file buzzard-tdms is
  ahead (A 0.53 s against 0.69 s).
- **4 threads give 1.4 to 1.9 times, not 4.** The same zero fill runs on the calling thread before the parallel
  part starts, so of B's 0.79 s about 0.69 s is serial. The parallel part itself reaches the ceiling: into existing
  buffers, 4 threads read B in 0.075 s (19 GB/s) and D in 0.21 s (18 GB/s), equal to 4-thread `pread`. A 4-thread
  `pread` into new memory takes 0.26 to 0.58 s for B, so a `read_columns` that left the first touch to the reading
  threads could come down from 0.79 s towards that; this is an estimate, not a measurement.
- **Reading in batches is not faster in general.** The crate's own harness reads in 256 MiB batches; that is
  slower than one call with 4 threads on A, B and C (1.01 to 1.10 s against 0.79 to 0.88 s) and on E (0.37 s
  against 0.26 s), and faster on D (1.57 s against 2.04 s).
- **Many interleaved channels into caller buffers: no fast path.** `read_columns` reads the rows once for all
  channels (C: 0.88 s), but it only returns new columns. The only call that fills a caller's buffer is
  `Channel::read_into`, one channel at a time, and each call walks all 1.44 GB again: 300 channels take 75 s on 1
  thread and 19 s on 4, where tedium's `read_channels_from` takes 2.3 s into the same kind of buffers.
- **One whole channel of an interleaved or driver file is slow in absolute terms** (C: 272 ms on 1 thread, 64 ms
  on 4, for 4.8 MB; E: 87 ms and 35 ms for 25 MB), because the channel's values are spread over the whole file. It
  is still 15 to 240 times faster than tedium (4.1 s) and npTDMS (15 s on C, 1.9 s on E).
- **Small reads of contiguous files: level with tdms-rs.** One channel: 3.5 to 4.6 ms against 2.9 to 4.0 ms (A, B);
  127 to 174 ms against 153 ms (D). One-second window of all channels: 1.7 to 4.6 ms against 2.1 to 5.0 ms. These
  are a few hundred to a few thousand `pread` calls either way (`strace -c`: A, one channel, 1,803 calls of which
  1,202 are the open).
- **Opening through the index file gains little on A** (17.0 ms against 19.7 ms; ranges overlap). The index of A is
  7.2 MB, because npTDMS's writer repeats the metadata in each of the 600 segments. It helps on D (1.4 to 2.1 ms
  against 2.5 to 5.9 ms). npTDMS does not gain from its index file on any shape.
- **Engineering units cost 2.6 to 6 times the raw read** (E: 0.66 s against 0.26 s on 4 threads, 2.28 s against
  0.36 s on 1). The 4-thread figure is `read_scaled` per channel with the channels spread over rayon by the caller;
  the crate has no call that scales many channels at once.
- **Parquet export:** 1.6 s for 1.44 GB and 5.7 s for 3.84 GB on 4 threads (0.7 to 0.9 GB/s), two to three times the
  read; 1 thread takes 3.2 s and 11.1 s. The output is not smaller than the input: 100% for A to C (Snappy saves
  nothing on doubles with noise in the low bits; this is a property of my generated data), 110% for D (the int32
  columns barely compress and a `time_s` column of 60 million doubles is added), 236% for E (int16 exported as
  scaled doubles). Uncompressed export is 25% faster on 1 thread (A: 2.43 s against 3.21 s) and within the noise on
  4 threads for A to D. The export time includes writing to the page cache, not syncing to disk.

**Where it is faster.**

- **Against npTDMS:** full read 3 to 18 times (A 4.33 s against 1.36 s on 1 thread and 0.81 s on 4; C 16.0 s
  against 1.71 s and 0.88 s; D 11.0 s against 3.27 s and 2.04 s; E 3.43 s against 0.36 s and 0.26 s); open of the
  600-segment file 401 ms against 20 ms; one-second window of all 300 channels 103 ms (A), 364 ms (B) and 3.7 s (C)
  against 2 to 4 ms; one channel of the interleaved file 15 s against 0.27 s. npTDMS needs twice the file size in
  memory for the one-segment and interleaved files (2.8 GB); buzzard-tdms needs the size of the data.
- **Against tedium:** full read 1.5 to 3.5 times (A 2.01 s, B 1.84 s, C 2.77 s, D 7.05 s); tedium reads the file
  in 176,000 to 178,000 small `read` calls on both A and B (`strace -c`). Open of A: 111 ms against 20 ms.
- **Against tdms-rs:** open of A (84 ms against 20 ms; tdms-rs seeks and reads per segment through a `BufReader`),
  and full reads with 4 threads (1.3 to 1.8 times). Otherwise level, as above.
- **Against tdms:** everything; tdms decodes value by value (23 s for A and B, 118 s for D) and its window read
  walks from the start of the channel (11 s for one second of 300 channels).
- **Shapes the others cannot read:** C (tdms-rs wrong, tdms out of memory) and E (only npTDMS).
- **Many segments cost buzzard-tdms little:** A against B, same values: 1.36 s against 1.24 s on 1 thread, 0.81 s
  against 0.79 s on 4, with 181,502 `pread` calls against 604. The 600 identical segments collapse to one run and
  one layout in its index.

## Limits of these numbers

- **One machine, a noisy VM.** Memory first-touch cost is the dominant term and it is not stable here. npTDMS, the
  only reader that gets huge pages, showed it most: its full read of B took 5.34 s (4.38 to 10.4) in the main run,
  7.04 s in the supplementary run, 1.8 and 2.0 s in two runs by hand in between, and 4.2 to 7.0 s minutes after
  those; a rerun of the earlier baseline script gave 735 MB/s (1.96 s) for the same shape, best of 3 in one process. So **the npTDMS ratios above
  could be about 3 times smaller on a quiet machine**; at npTDMS's best observed time buzzard-tdms's lead on B is
  1.5 times (1 thread) to 2.3 times (4 threads), not 4 to 7. The Rust readers moved by up to 30% between sessions
  (buzzard-tdms 1 thread on B: 1.24 s, 1.04 s, 0.95 s).
- **The earlier baseline (tdms-notes.md, section 4) is not reproduced by these runs**: it reports npTDMS at 472 to
  1,066 MB/s on the contiguous 300-channel files and 1,823 MB/s on the int32 file, at 384 MB, best of 3 in one
  process. Here, at full size, median of 5 processes: 334, 270 and 349 MB/s. Rerun today at 384 MB the baseline
  script still gives 839 and 1,800 MB/s. The method (best of 3 in one process against the median of new processes)
  and the size both differ; I did not separate the two.
- **Cold-cache numbers cannot rank the readers.** After `posix_fadvise(DONTNEED)` the guest had 0% of the file
  cached, but only the first cold run of shapes A to D was slow (the `pread` ceiling: 159 to 424 MB/s); the runs after
  it were faster than that ceiling, so the hypervisor was serving them from its own cache. One run each.
- **The "ceiling" is a ceiling for readers that return new memory.** With memory that exists, plain `pread` is 3
  to 4 times faster than the ceiling in the main tables; the supplementary tables show both.
- **Not profiled with a sampling profiler** (`perf` is not installed). The limiter is named from process CPU
  times (user and kernel), `strace -c` call counts, the allocation-only measurement and the existing-buffer
  measurements, plus reading `read_columns` (`crates/buzzard-tdms/src/file.rs`, `zeroed`).
- **API choice per reader is mine.** For each reader I used the call its documentation gives for the job. tedium
  and tdms-rs fill caller buffers; in the main tables those buffers are `vec![0; n]` allocated inside the timer.
- **Not measured:** files with channel lists that change between segments, strings, big-endian data, files larger
  than memory, Windows and macOS, an index file written by LabVIEW, and a real driver-written DAQmx file.
- **Left behind:** pyarrow 26.0.1 was added to `tdms-research/venv`. The data files, Parquet exports and build
  directories are deleted; scripts, sources, logs and results are kept.
