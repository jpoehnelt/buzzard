#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Oracle harness for buzzard-tdms: what does npTDMS make of a TDMS file?

This script is part of buzzard and is MIT licensed. It uses npTDMS
(https://github.com/adamreeve/npTDMS, LGPL-3.0) only through its public
API, as a separate program. Nothing of npTDMS is copied here or linked
into the Rust crate; see README.md beside this file.

Commands:

  dump PATH...     Print, as JSON on stdout, npTDMS's view of every .tdms
                   file named or found (not recursively) in a directory.
  write DIR        Write a set of files into DIR with nptdms.TdmsWriter.
  scenarios DIR    Write into DIR the files of the test scenarios that the
                   installed npTDMS package ships (nptdms.test.scenarios).
                   Exits with status 3 when they cannot be imported, which
                   is the case when pytest is not installed.
  data-dir         Print the directory of the data files that the
                   installed npTDMS package ships (nptdms/test/data).

The JSON of `dump` is {"nptdms": version, "files": {path: view}}. A view
is {"error": text} when npTDMS raises, else:

  {"properties": [property...],
   "groups": [{"name", "properties", "channels": [channel...]}]}

  property: {"name", "kind", "value"} with kind and value one of
    "int"        decimal text
    "float"      the 64-bit pattern as 16 hex digits
    "str"        the text
    "bool"       true or false
    "timestamp"  [seconds, fractions], both decimal text
    "complex"    [bits of the real part, bits of the imaginary part]
    "other"      repr text

  channel: {"name", "type", "length", "properties", "raw", "scaled"}
    "type"    the name of npTDMS's type class, or null
    "raw"     {scaler: {"dtype", "length", "sha256"}}: the unscaled data;
              the scaler is "" except for DAQmx raw data, where it is the
              scale id. The hash is over the values as little-endian
              bytes; a timestamp is its fractions (u64) then its seconds
              (i64); a string is its UTF-8 bytes followed by a zero byte.
    "scaled"  null, or the scaled values as 64-bit patterns (16 hex digits
              each, concatenated) when they differ from the raw data.
"""

import hashlib
import json
import os
import struct
import sys

import numpy as np

import nptdms
from nptdms import ChannelObject, GroupObject, RootObject, TdmsFile, TdmsWriter
from nptdms.timestamp import TdmsTimestamp


def bits(value):
    return struct.pack(">d", float(value)).hex()


def property_view(name, value):
    if isinstance(value, TdmsTimestamp):
        kind, out = "timestamp", [str(value.seconds), str(value.second_fractions)]
    elif isinstance(value, (bool, np.bool_)):
        kind, out = "bool", bool(value)
    elif isinstance(value, (int, np.integer)):
        kind, out = "int", str(int(value))
    elif isinstance(value, (float, np.floating)):
        kind, out = "float", bits(value)
    elif isinstance(value, (complex, np.complexfloating)):
        kind, out = "complex", [bits(value.real), bits(value.imag)]
    elif isinstance(value, str):
        kind, out = "str", value
    else:
        kind, out = "other", repr(value)
    return {"name": name, "kind": kind, "value": out}


def properties_view(properties):
    return [property_view(name, value) for name, value in properties.items()]


def raw_view(data):
    """The dtype, length and hash of an array of unscaled values."""
    if hasattr(data, "seconds") and hasattr(data, "second_fractions"):
        pairs = np.empty(len(data), dtype=[("f", "<u8"), ("s", "<i8")])
        pairs["f"] = data.second_fractions
        pairs["s"] = data.seconds
        dtype, payload = "timestamp", pairs.tobytes()
    elif data.dtype == object or data.dtype.kind in "US":
        dtype = "string"
        payload = b"".join(str(s).encode("utf-8") + b"\0" for s in data)
    else:
        dtype = data.dtype.name
        payload = np.ascontiguousarray(data).astype(data.dtype.newbyteorder("<")).tobytes()
    return {"dtype": dtype, "length": len(data), "sha256": hashlib.sha256(payload).hexdigest()}


def channel_view(channel):
    view = {
        "name": channel.name,
        "type": channel.data_type.__name__ if channel.data_type is not None else None,
        "length": len(channel),
        "properties": properties_view(channel.properties),
        "raw": {},
        "scaled": None,
    }
    if len(channel) == 0:
        return view
    raw = channel.read_data(scaled=False)
    if isinstance(raw, dict):
        view["raw"] = {str(scale): raw_view(data) for scale, data in sorted(raw.items())}
    else:
        view["raw"] = {"": raw_view(raw)}
    scaled = channel[:]
    plain = not isinstance(raw, dict) and (
        scaled.dtype == object
        or hasattr(scaled, "seconds")
        or (scaled.dtype == raw.dtype and np.array_equal(scaled, raw, equal_nan=scaled.dtype.kind in "fc"))
    )
    if not plain:
        view["scaled"] = "".join(bits(v) for v in np.asarray(scaled, dtype=np.float64))
    return view


def file_view(path):
    try:
        with TdmsFile.read(path, raw_timestamps=True) as tdms:
            return {
                "properties": properties_view(tdms.properties),
                "groups": [
                    {
                        "name": group.name,
                        "properties": properties_view(group.properties),
                        "channels": [channel_view(c) for c in group.channels()],
                    }
                    for group in tdms.groups()
                ],
            }
    except Exception as e:  # the oracle's refusal is part of its view
        return {"error": "%s: %s" % (type(e).__name__, e)}


def tdms_files(paths):
    for path in paths:
        if os.path.isdir(path):
            for name in sorted(os.listdir(path)):
                if name.endswith(".tdms"):
                    yield os.path.join(path, name)
        else:
            yield path


def dump(paths):
    files = {path: file_view(path) for path in tdms_files(paths)}
    json.dump({"nptdms": nptdms.__version__, "files": files}, sys.stdout, indent=1, sort_keys=True)
    sys.stdout.write("\n")


def write(directory):
    """Files written through npTDMS's public writer."""
    rng = np.random.default_rng(20261116)

    def path(name):
        return os.path.join(directory, name + ".tdms")

    numeric = ["int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64", "float32", "float64"]

    def numbers(dtype, n):
        if dtype.startswith("float"):
            return rng.normal(size=n).astype(dtype)
        info = np.iinfo(dtype)
        return rng.integers(info.min, info.max, size=n, dtype=dtype, endpoint=True)

    def every_type(n):
        channels = [ChannelObject("numbers", dtype, numbers(dtype, n)) for dtype in numeric]
        channels.append(ChannelObject("others", "bool", rng.integers(0, 2, size=n).astype(np.bool_)))
        channels.append(ChannelObject("others", "complex64", (rng.normal(size=n) + 1j * rng.normal(size=n)).astype("complex64")))
        channels.append(ChannelObject("others", "complex128", rng.normal(size=n) + 1j * rng.normal(size=n)))
        channels.append(ChannelObject("others", "text", ["s%d" % i * (i % 3) for i in range(n)]))
        channels.append(
            ChannelObject(
                "others",
                "time",
                np.array(["2026-11-16T12:00:00"], dtype="datetime64[us]") + np.arange(n).astype("timedelta64[us]") * 1001,
            )
        )
        return channels

    for version in (4712, 4713):
        with TdmsWriter(path("every_type_%d" % version), version=version) as writer:
            writer.write_segment(every_type(7))
            writer.write_segment(every_type(7))
            writer.write_segment(every_type(3))

    properties = {
        "an int": 7,
        "a negative int": -70000,
        "a big int": 2**40,
        "a float": 1.5,
        "a string": "naïve 'text' 日本",
        "an empty string": "",
        "a bool": True,
        "an int8": np.int8(-8),
        "a uint16": np.uint16(60000),
        "a uint64": np.uint64(2**63 + 5),
        "a float32": np.float32(0.1),
        "a time": np.datetime64("2026-11-16T12:34:56.789012"),
        "a raw time": TdmsTimestamp(3850000000, 2**63 + 12345),
    }
    with TdmsWriter(path("properties")) as writer:
        writer.write_segment(
            [
                RootObject(properties),
                GroupObject("Dr. T's Events", properties),
                ChannelObject("Dr. T's Events", "Ti'me / 日本", np.arange(5, dtype=np.float64), properties),
                GroupObject("empty group", {"note": "no channels"}),
            ]
        )
        writer.write_segment(
            [
                RootObject({"an int": 8, "added": "later"}),
                ChannelObject("Dr. T's Events", "Ti'me / 日本", np.arange(5, dtype=np.float64), {"a float": 2.5}),
            ]
        )

    with TdmsWriter(path("changing_channels"), index_file=True) as writer:
        a = lambda n: ChannelObject("g", "a", numbers("int32", n))
        b = lambda n: ChannelObject("g", "b", numbers("float64", n))
        c = lambda n: ChannelObject("h", "c", numbers("uint16", n))
        writer.write_segment([a(4), b(4)])
        writer.write_segment([a(4), b(4)])
        writer.write_segment([a(4), b(4), c(9)])
        writer.write_segment([b(2), a(6)])
        writer.write_segment([c(1)])
        writer.write_segment([a(6), c(1)])
        writer.write_segment([ChannelObject("g", "a", np.array([], dtype=np.int32)), b(3)])
        writer.write_segment([a(1), b(3)])

    with TdmsWriter(path("many_segments")) as writer:
        for _ in range(200):
            writer.write_segment([ChannelObject("g", "c%d" % i, numbers("float64", 10)) for i in range(12)])

    with TdmsWriter(path("waveform")) as writer:
        writer.write_segment(
            [
                ChannelObject(
                    "g",
                    "wave",
                    numbers("float32", 100),
                    {
                        "wf_start_time": np.datetime64("2026-11-16T12:00:00"),
                        "wf_start_offset": 0.0,
                        "wf_increment": 0.001,
                        "wf_samples": 100,
                        "unit_string": "V",
                    },
                )
            ]
        )

    def scaled(n_scales, extra):
        properties = {"NI_Scaling_Status": "unscaled", "NI_Number_Of_Scales": np.uint32(n_scales)}
        properties.update(extra)
        return properties

    def linear(i, slope, intercept, source):
        return {
            "NI_Scale[%d]_Scale_Type" % i: "Linear",
            "NI_Scale[%d]_Linear_Slope" % i: slope,
            "NI_Scale[%d]_Linear_Y_Intercept" % i: intercept,
            "NI_Scale[%d]_Linear_Input_Source" % i: np.uint32(source),
        }

    with TdmsWriter(path("linear_scaling")) as writer:
        two = dict(linear(0, 0.5, 1.0, 0xFFFFFFFF))
        two.update(linear(1, 10.0, -2.0, 0))
        done = scaled(1, linear(0, 3.0, 4.0, 0xFFFFFFFF))
        done["NI_Scaling_Status"] = "scaled"
        writer.write_segment(
            [
                ChannelObject("g", "one", numbers("int16", 50), scaled(1, linear(0, 0.25, -3.0, 0xFFFFFFFF))),
                ChannelObject("g", "chain", numbers("int32", 50), scaled(2, two)),
                ChannelObject("g", "float", numbers("float64", 50), scaled(1, linear(0, 2.0, 0.5, 0xFFFFFFFF))),
                ChannelObject("g", "already", numbers("int16", 50), done),
                ChannelObject("g", "none", numbers("int16", 50), {"NI_Number_Of_Scales": np.uint32(0)}),
            ]
        )

    with TdmsWriter(path("appended")) as writer:
        writer.write_segment([ChannelObject("g", "a", numbers("int64", 5))])
    with TdmsWriter(path("appended"), mode="a") as writer:
        writer.write_segment([ChannelObject("g", "a", numbers("int64", 5)), ChannelObject("g", "new", ["x", "yz"])])


def scenarios(directory):
    """The scenario files of the installed npTDMS, run as a black box."""
    try:
        from nptdms.test import scenarios as shipped
    except Exception as e:
        sys.stderr.write("the scenarios shipped with npTDMS cannot be imported: %r\n" % (e,))
        return 3
    written = []
    for number, scenario in enumerate(shipped.get_scenarios()):
        values = getattr(scenario, "values", scenario)
        test_file = values[0]
        name = "%02d_%s" % (number, getattr(scenario, "id", None) or "scenario")
        name = "".join(ch if ch.isalnum() or ch in "_-" else "_" for ch in name)
        target = os.path.join(directory, name + ".tdms")
        with test_file.get_bytes_io_file() as source, open(target, "wb") as out:
            out.write(source.read())
        written.append(target)
    json.dump(written, sys.stdout)
    sys.stdout.write("\n")
    return 0


def main(argv):
    if len(argv) >= 3 and argv[1] == "dump":
        dump(argv[2:])
    elif len(argv) == 3 and argv[1] == "write":
        write(argv[2])
    elif len(argv) == 3 and argv[1] == "scenarios":
        return scenarios(argv[2])
    elif len(argv) == 2 and argv[1] == "data-dir":
        print(os.path.join(os.path.dirname(nptdms.__file__), "test", "data"))
    else:
        sys.stderr.write(__doc__)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
