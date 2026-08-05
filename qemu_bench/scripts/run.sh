#!/usr/bin/env bash
#
# Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
#
# SPDX-License-Identifier: BSD-3-Clause-Clear
#
# Run every qemu_bench binary under qemu-hexagon, tabulating wall-clock
# time and PASS/FAIL for each. Usage: run.sh [N]
#
# All benchmarks (including HMX) run under the qemu-hexagon-sysemu build
# below -- the in-tree build's HMX TCG implementation is incomplete, so
# HMX binaries there don't run correctly; this binary is confirmed to run
# both plain-HVX and HMX .word-polyfill binaries correctly.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BENCH_DIR="$(dirname "$SCRIPT_DIR")"
OUT_DIR="$BENCH_DIR/output"

QEMU=/local/mnt/workspace/install/qemu-hexagon-sysemu-10-july-2026/bin/qemu-hexagon
N="${1:-100}"

printf "%-28s %-6s %10s\n" "benchmark" "result" "wall(s)"
printf "%-28s %-6s %10s\n" "---------" "------" "-------"

overall=0
for bin in "$OUT_DIR"/bench_*; do
    name="$(basename "$bin")"
    start=$(date +%s.%N)
    if out=$("$QEMU" "$bin" "$N" 2>&1); then
        result="PASS"
    else
        result="FAIL"
        overall=1
    fi
    end=$(date +%s.%N)
    elapsed=$(awk -v s="$start" -v e="$end" 'BEGIN{printf "%.3f", e-s}')
    printf "%-28s %-6s %10s\n" "$name" "$result" "$elapsed"
    if [[ "$result" == "FAIL" ]]; then
        echo "  -> $out"
    fi
done

exit "$overall"
