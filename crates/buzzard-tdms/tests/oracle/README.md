# The npTDMS oracle

`dump.py` asks [npTDMS](https://github.com/adamreeve/npTDMS) what it makes
of a set of TDMS files, and `../oracle.rs` requires this crate's reader to
agree.

## Run it

```sh
python3 -m venv /tmp/oracle
/tmp/oracle/bin/pip install npTDMS==1.14.0 pytest==9.2.0
BUZZARD_TDMS_ORACLE=/tmp/oracle/bin/python cargo test -p buzzard-tdms --test oracle -- --nocapture
```

Without `BUZZARD_TDMS_ORACLE` the test says that it was skipped, and
passes. The `tdms-oracle` job in `.github/workflows/ci.yml` runs it on
every change with `BUZZARD_TDMS_ORACLE_STRICT` set, which turns a missing
pytest into a failure. CI pins npTDMS 1.14.0, and the test compares 760
files with that version.

## What is compared

For every file, the test compares the groups, the channels, each channel's
type and length, every property with its kind and value, the SHA-256 of
each channel's raw values, and the scaled values where a channel has
scaling. It does not compare the width of an integer property, because
npTDMS returns every integer property as a Python `int`. The files are:

- files from this crate's generator (`buzzard_tdms::build`), which cover
  what npTDMS cannot write: big-endian, interleaved and driver data
- files written by `nptdms.TdmsWriter` through its public API
- the real file in `../fixtures`
- the 31 scenario files and 4 data files in the test package of the
  installed npTDMS wheel, read where pip installed them

`KNOWN_DIFFERENCES` in `../oracle.rs` lists every place where the two
readers disagree, with the reason this reader's answer is kept. A
disagreement that is not listed fails the test, and so does an entry that
no longer occurs.

## License boundary

npTDMS is LGPL-3.0. This crate is Apache-2.0. To keep them apart:

- `dump.py` is ours, written for this repository, and Apache-2.0 licensed. It
  imports npTDMS and calls its public API (`TdmsFile.read`,
  `TdmsWriter`, channel and property accessors) and the entry points of
  its shipped test package (`get_scenarios`, `get_bytes_io_file`).
- npTDMS runs as a separate program at test time. It is not a build or
  runtime dependency of the crate, and nothing of it is linked or vendored.
- No npTDMS code, test or fixture was copied or ported. The reader was
  written from the published format description, from research notes that
  state format facts in prose (some of them first established by reading
  npTDMS, and recorded as facts, not as code), and from the behaviour
  observed through this harness.
- Files that npTDMS writes are program output. The test writes them to a
  temporary directory and deletes them.
