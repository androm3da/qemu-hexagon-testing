#!/usr/bin/env bash
#
# Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
#
# SPDX-License-Identifier: BSD-3-Clause-Clear
#
# Cross-compile every qemu_bench/{hvx,hmx}/bench_*.c into a statically
# linked hexagon-unknown-linux-musl binary under qemu_bench/output/.
#
# qhl_hvx sources are compiled directly into each hvx/bench_* binary that
# calls into qhblas_hvx/qhmath_hvx/qhdsp_hvx (from source, not the SDK's
# prebuilt .a, since those were built with a different proprietary
# toolchain -- see the qemu_bench design notes for why).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BENCH_DIR="$(dirname "$SCRIPT_DIR")"
OUT_DIR="$BENCH_DIR/output"

CLANG="${HEXAGON_CLANG:-/local/mnt/workspace/install/clang+llvm-21.1.8-cross-hexagon-unknown-linux-musl/x86_64-linux-gnu/bin/clang}"
SDK="${HEXAGON_QHL_HVX_DIR:-/opt/Hexagon_SDK/6.4.0.2/libs/qhl_hvx}"
QHCOMPLEX_INC="${HEXAGON_QHCOMPLEX_INC:-/opt/Hexagon_SDK/6.4.0.2/libs/qhl/inc/qhcomplex}"

COMMON_FLAGS=(--target=hexagon-unknown-linux-musl -mv73 -mhvx -mhvx-length=128B -O2 -static)
HVX_INC=(-I "$BENCH_DIR/common" -I "$BENCH_DIR/hvx"
         -I "$SDK/inc/qhblas_hvx" -I "$SDK/inc/qhmath_hvx"
         -I "$SDK/inc/qhdsp_hvx" -I "$SDK/inc/qhdsp_hvx/image_processing"
         -I "$SDK/inc/internal" -I "$QHCOMPLEX_INC")
HMX_INC=(-I "$BENCH_DIR/common" -I "$BENCH_DIR/hmx")

mkdir -p "$OUT_DIR"

# name -> extra qhl_hvx source files (space-separated), relative to $SDK/src
declare -A QHL_SRCS=(
    [bench_matmul_i8]="qhblas_hvx/qhblas_hvx_matrix_matrix_mpy_ab.c"
    [bench_matmul_fp32]="qhblas_hvx/qhblas_hvx_matrix_matrix_mpy_af.c"
    [bench_matmul_fp16]="qhblas_hvx/qhblas_hvx_matrix_matrix_mpy_ahf.c"
    [bench_conv_i8]="qhdsp_hvx/image_processing/qhdsp_hvx_conv3x3_ab.c"
    [bench_tanh_i8]="qhmath_hvx/qhmath_hvx_tanh_af.c"
    [bench_tanh_fp32]="qhmath_hvx/qhmath_hvx_tanh_af.c"
    [bench_tanh_fp16]="qhmath_hvx/qhmath_hvx_tanh_ahf.c"
    [bench_softmax_i8]="qhblas_hvx/qhblas_hvx_vector_scaling_af.c qhmath_hvx/qhmath_hvx_exp_af.c"
    [bench_softmax_fp32]="qhblas_hvx/qhblas_hvx_vector_scaling_af.c qhmath_hvx/qhmath_hvx_exp_af.c"
    [bench_softmax_fp16]="qhblas_hvx/qhblas_hvx_vector_scaling_ahf.c qhmath_hvx/qhmath_hvx_exp_ahf.c"
)

fail=0

build_hvx() {
    local src="$1"
    local name
    name="$(basename "$src" .c)"
    local extra=()
    if [[ -n "${QHL_SRCS[$name]:-}" ]]; then
        for rel in ${QHL_SRCS[$name]}; do
            extra+=("$SDK/src/$rel")
        done
    fi
    echo "building $name"
    if ! "$CLANG" "${COMMON_FLAGS[@]}" "${HVX_INC[@]}" \
            "$src" "$BENCH_DIR/common/scalar_ref.c" "${extra[@]}" \
            -o "$OUT_DIR/$name"; then
        echo "FAILED: $name" >&2
        fail=1
    fi
}

build_hmx() {
    local src="$1"
    local name
    name="$(basename "$src" .c)"
    echo "building $name"
    if ! "$CLANG" "${COMMON_FLAGS[@]}" -fno-builtin -Wno-inline-asm "${HMX_INC[@]}" \
            "$src" \
            -o "$OUT_DIR/$name"; then
        echo "FAILED: $name" >&2
        fail=1
    fi
}

for src in "$BENCH_DIR"/hvx/bench_*.c; do
    build_hvx "$src"
done

for src in "$BENCH_DIR"/hmx/bench_*.c; do
    build_hmx "$src"
done

if [[ "$fail" -ne 0 ]]; then
    echo "one or more benchmarks failed to build" >&2
    exit 1
fi

echo "all benchmarks built into $OUT_DIR"
