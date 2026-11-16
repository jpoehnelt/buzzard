#!/bin/sh
# Benchmark the TDMS reader: build the release example and run it for
# every shape with 1 thread and with all cores.
#
#   tools/tdms-bench.sh [SIZE_MB] [DIR]
#
# SIZE_MB is the size of each generated file (default 400). DIR is where
# the files go (default target/tdms-bench); it needs SIZE_MB of free space
# twice over, for the file and its Parquet export. Everything written is
# deleted again. Results are the best of three runs with the file in the
# page cache.
set -eu

size=${1:-400}
dir=${2:-target/tdms-bench}
cores=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)

cargo build --release --locked -p buzzard-tdms --features parquet --example tdms_bench
for shape in stand segments interleaved; do
    for threads in 1 "$cores"; do
        echo
        target/release/examples/tdms_bench --dir "$dir" --size-mb "$size" \
            --shape "$shape" --threads "$threads"
    done
done
rmdir "$dir" 2>/dev/null || true
