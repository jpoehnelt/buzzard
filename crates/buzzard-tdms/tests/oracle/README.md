# The npTDMS oracle

`dump.py` reads a set of TDMS files with
[npTDMS](https://github.com/adamreeve/npTDMS) and prints what npTDMS finds
in them. `the_reader_agrees_with_nptdms` in
`crates/buzzard-tdms/tests/oracle.rs` requires this crate's reader to agree.

## Run the oracle

To run the comparison, install npTDMS and pytest into a virtual
environment and name its Python in `BUZZARD_TDMS_ORACLE`.

```sh
python3 -m venv /tmp/oracle
/tmp/oracle/bin/pip install npTDMS==1.14.0 pytest==9.2.0
BUZZARD_TDMS_ORACLE=/tmp/oracle/bin/python cargo test -p buzzard-tdms --test oracle -- --nocapture
```

The test prints how many files it compared. With npTDMS 1.14.0, the version
that CI pins, it compares 760.

Without `BUZZARD_TDMS_ORACLE`, the test says that it was skipped and
passes. Without pytest, the test leaves out the scenario files that npTDMS
ships. To make a missing pytest a failure, also set
`BUZZARD_TDMS_ORACLE_STRICT`. The `tdms-oracle` job in
`.github/workflows/ci.yml` sets both variables and runs on every change.

## What the test compares

For every file, the test compares the groups, the channels, and each
channel's type and length. It compares every property with its kind and
value, the SHA-256 of each channel's raw values, and the scaled values
where a channel has scaling.

The test does not compare the width of an integer property, because
npTDMS returns every integer property as a Python `int`.

The files come from four places:

- This crate's generator (`buzzard_tdms::build`). Its files cover what
  npTDMS cannot write: big-endian, interleaved, and driver data.
- `nptdms.TdmsWriter`, called through its public API.
- The real file in `crates/buzzard-tdms/tests/fixtures`.
- The test package of the installed npTDMS wheel, read where pip installed
  it. Version 1.14.0 ships 31 scenario files and 4 data files.

`KNOWN_DIFFERENCES` in `crates/buzzard-tdms/tests/oracle.rs` lists every
place where the reader and npTDMS disagree, and says why the test keeps the
reader's answer. A disagreement that is not listed fails the test. So does
an entry that no longer occurs.

## Where the license boundary runs

npTDMS is LGPL-3.0. This crate is Apache-2.0. Four facts keep them apart:

- `dump.py` was written for this repository and is Apache-2.0 licensed. It
  imports npTDMS and calls its public API (`TdmsFile.read`,
  `TdmsWriter`, channel and property accessors) and the entry points of
  its shipped test package (`get_scenarios`, `get_bytes_io_file`).
- npTDMS runs as a separate program at test time. It is not a build or
  runtime dependency of the crate, and nothing of it is linked or vendored.
- No npTDMS code, test, or fixture was copied or ported. The reader was
  written from three sources: the published format description, research
  notes that state format facts in prose, and the behavior of npTDMS
  observed through `dump.py`. Some of the facts in the notes were first
  established by reading npTDMS. The notes record them as facts, not as
  code.
- Files that npTDMS writes are program output. The test writes them to a
  temporary directory and deletes them.
