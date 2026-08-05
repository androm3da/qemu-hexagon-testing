/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * fp16 matmul benchmark: wraps qhblas_hvx_matrix_matrix_mpy_ahf.
 *
 * Inputs are strictly positive (1.0 +/- 0.5) rather than zero-mean random:
 * with 128-term dot products and zero-mean terms, many output elements are
 * near-cancelling sums where fp16's 10-bit mantissa accumulates enough
 * rounding error (relative to the tiny near-zero result) to blow past any
 * reasonable tolerance -- an inherent fp16 precision limit, not a QEMU
 * emulation bug. Positive-only inputs keep every partial sum monotonically
 * growing, avoiding that cancellation.
 */

#include <stdint.h>
#include <stdlib.h>
#include "qhblas_hvx.h"
#include "bench_common.h"
#include "scalar_ref.h"

#define M 128u
#define K 128u
#define N 128u

int main(int argc, char **argv)
{
    long iters = bench_parse_n(argc, argv);
    bench_rng_t rng;
    bench_rng_seed(&rng, 0x1234u);

    __fp16 *a = aligned_alloc(128, M * K * sizeof(__fp16));
    __fp16 *b = aligned_alloc(128, K * N * sizeof(__fp16));
    __fp16 *out = aligned_alloc(128, M * N * sizeof(__fp16));
    __fp16 *ref = malloc(M * N * sizeof(__fp16));

    for (uint32_t i = 0; i < M * K; i++) {
        a[i] = (__fp16)(bench_rand_f32(&rng, 0.5f) + 1.0f);
    }
    for (uint32_t i = 0; i < K * N; i++) {
        b[i] = (__fp16)(bench_rand_f32(&rng, 0.5f) + 1.0f);
    }

    scalar_matmul_f16(a, b, ref, M, K, N);

    for (long it = 0; it < iters; it++) {
        qhblas_hvx_matrix_matrix_mpy_ahf(a, b, out, M, K, N);
    }

    int ok = 1;
    for (uint32_t i = 0; i < M * N && ok; i++) {
        ok = bench_compare_f32((float)out[i], (float)ref[i], 0.05f);
    }
    return bench_report("bench_matmul_fp16", ok);
}
