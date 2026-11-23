# TDMS fixtures

One real measurement file, copied byte for byte from a public data set.
Every other test file is written at test time by `buzzard_tdms::build`.

| File | Source | License |
| --- | --- | --- |
| `zenodo-7758683/waveforms-point21.tdms` | `Waveforms Point21.tdms` from Saullo G. P. Castro, "Impact hammer test of an Aluminium 6082 T6 plate, 260 x 246 x 1.5 mm, free boundary conditions", Zenodo, 2022, [doi:10.5281/zenodo.7758683](https://doi.org/10.5281/zenodo.7758683) | [Creative Commons Attribution 4.0 International](https://creativecommons.org/licenses/by/4.0/legalcode) |

Changes: none to the contents. The file was renamed from
`Waveforms Point21.tdms`.

| | |
| --- | --- |
| Size | 526,053 bytes |
| MD5, as Zenodo lists it | `c061a9d27d59ee18ae57f9b7f607f1f4` |
| SHA-256 | `e65dd8e2cc58e406c99a16d494a9f6648c8ac6b31c65495110dc5a4c63b47442` |
| Retrieved | 2026-11-16, from `https://zenodo.org/records/7758683` |

`the_fixture_is_the_published_file` in `../real.rs` checks the size and the
SHA-256, so an edited copy fails the tests.

## What the file is

A recording by sound-and-vibration acquisition software: one group with a
force channel and an acceleration channel, 32,768 samples of 64-bit floats
each, written as two segments (the second has metadata only), format
version 4713, with waveform properties (`wf_start_time`, `wf_increment`,
`wf_samples`) and some twenty other properties per channel.

It is the only file in the tests that was written by the software the
format comes from. The expected values in `../real.rs` are the hashes that
npTDMS computes for the same file. `../oracle.rs` compares the two readers
on it again whenever the oracle runs.

## Other real files

The same Zenodo record holds 24 more recordings of the same kind, and
other records hold files of many gigabytes. They are not in the
repository. `real_files_outside_the_repository` in `../real.rs` reads
every file of a directory named by `BUZZARD_TDMS_SAMPLES`:

```sh
BUZZARD_TDMS_SAMPLES=/data/tdms cargo test -p buzzard-tdms --release --test real -- --ignored --nocapture
```
