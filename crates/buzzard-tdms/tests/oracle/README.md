# The npTDMS oracle

`dump.py` asks [npTDMS](https://github.com/adamreeve/npTDMS) what it makes
of a set of TDMS files, and `../oracle.rs` requires this crate's reader to
agree, sample for sample.

## Running it

```sh
python3 -m venv /tmp/oracle
/tmp/oracle/bin/pip install npTDMS pytest
BUZZARD_TDMS_ORACLE=/tmp/oracle/bin/python cargo test -p buzzard-tdms --test oracle -- --nocapture
```

Without `BUZZARD_TDMS_ORACLE` the test says that it was skipped, and
passes. The `tdms-oracle` job in `.github/workflows/ci.yml` runs it on
every change with `BUZZARD_TDMS_ORACLE_STRICT` set, which turns a missing
pytest into a failure.

## What is compared

For every file: the groups, the channels, each channel's type and length,
every property with its kind and value, the SHA-256 of each channel's raw
values as little-endian bytes (timestamps as their raw seconds and
fractions), and the scaled values where a channel has scaling. The files
come from four places:

| Source | Files | Why |
| --- | --- | --- |
| This crate's generator (`buzzard_tdms::build`) | the conformance cross product, driver buffers, truncated and ragged files, files that bend the format | covers what npTDMS cannot write: big-endian, interleaved and driver data |
| `nptdms.TdmsWriter`, through its public API | every type, properties, changing channel lists, 200 segments, scaling, appending | files from a second, independent writer |
| The scenarios the installed npTDMS ships (`nptdms.test.scenarios`) | 31 files | the cases npTDMS's authors found worth a test |
| The data files the installed npTDMS ships (`nptdms/test/data`) | 4 files, two of them driver data written by real acquisition software | the only driver files available |

The last two exist because the npTDMS wheel on PyPI contains its test
package. They are run and read where pip installed them. Nothing of them
is in this repository.

A property's integer width is not compared: npTDMS's public API returns
every integer property as a Python `int`.

## Disagreements

Every place where the two readers disagree is in `KNOWN_DIFFERENCES` in
`../oracle.rs` with the reason, and the crate's `README.md` repeats the
list. A disagreement that is not listed fails the test, and so does an
entry that no longer occurs.

## Licence boundary

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
- Files that npTDMS writes are program output; the test writes them to a
  temporary directory and deletes them.
