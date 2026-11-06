#!/bin/bash
# Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
# SPDX-License-Identifier: BSD-3-Clause-Clear
#
# Build and run the packet-semantics tests (test_pkt_*) on hexagon-sim.
#
# The packets are assembled by the Hexagon SDK 21.0.04 assembler, which is
# the only one that knows every instruction through v85 + HLX, and run on
# the matching hexagon-sim.  Override with HEXAGON_TOOLS_BIN / HEXAGON_SIM.
#
# Usage:
#   ./run_pkt_tests.sh                 # build and run all test_pkt_*
#   ./run_pkt_tests.sh test_pkt_regs   # one test
#   ./run_pkt_tests.sh --build-only
#   SIM_ARGS="--mv85qa_1" ./run_pkt_tests.sh
#
# No --timing: it is not needed for verification testing.

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

TOOLS_BIN="${HEXAGON_TOOLS_BIN:-/opt/Hexagon_SDK/6.7.0.0/tools/HEXAGON_Tools/21.0.04/Tools/bin}"
export PATH="$TOOLS_BIN:$PATH"
SIM="${HEXAGON_SIM:-hexagon-sim}"
SIM_ARGS="${SIM_ARGS:---mv85}"
BUILD_DIR="target/hexagon-unknown-none-elf/release"
BUILD_ONLY=0
SINGLE=""
for a in "$@"; do
    case "$a" in
        --build-only) BUILD_ONLY=1 ;;
        *) SINGLE="$a" ;;
    esac
done

echo "=== Building ==="
cargo +nightly build --release --features pkt 2>&1 | grep -E "^(error|warning: unused)|Finished|-->" || true
[[ "$BUILD_ONLY" -eq 1 ]] && exit 0

TESTS=()
if [[ -n "$SINGLE" ]]; then
    TESTS=("$SINGLE")
else
    for f in "$BUILD_DIR"/test_pkt_*; do
        [[ -f "$f" && -x "$f" && ! "$f" =~ \. ]] || continue
        TESTS+=("$(basename "$f")")
    done
fi

PASS=0; FAIL=0; BAD=()
for t in "${TESTS[@]}"; do
    echo "--- $t ---"
    rc=0
    out=$(timeout "${TEST_TIMEOUT:-600}" $SIM $SIM_ARGS --bypass_idle "$BUILD_DIR/$t" 2>&1) || rc=$?
    echo "$out" | grep -vE '^\s+T[0-9]+:|^$|rev_id|^Done|Total:|Architecture of executable'
    # Pass/fail is the suite's own verdict; hexagon-sim's exit status is
    # not the test's (no --simulated_returnval).
    if [[ $rc -eq 0 ]] && echo "$out" | grep -q '^PASS'; then PASS=$((PASS+1)); else FAIL=$((FAIL+1)); BAD+=("$t"); fi
done
echo "=============================="
echo "pkt tests: $PASS passed, $FAIL failed"
[[ $FAIL -eq 0 ]] || { printf '  - %s\n' "${BAD[@]}"; exit 1; }
