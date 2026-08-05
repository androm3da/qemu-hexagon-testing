/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * int8 matmul benchmark: wraps qhblas_hvx_matrix_matrix_mpy_ab (GEMM,
 * out[m x n] = a[m x k] * b[k x n], accumulate in 16-bit, >>7 saturate to
 * int8 -- see qhblas_hvx_matrix_matrix_mpy_ab.c).
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>
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

    int8_t *a = aligned_alloc(128, M * K * sizeof(int8_t));
    int8_t *b = aligned_alloc(128, K * N * sizeof(int8_t));
    int8_t *out = aligned_alloc(128, M * N * sizeof(int8_t));
    int8_t *ref = malloc(M * N * sizeof(int8_t));

    for (uint32_t i = 0; i < M * K; i++) {
        a[i] = bench_rand_i8(&rng, 32);
    }
    for (uint32_t i = 0; i < K * N; i++) {
        b[i] = bench_rand_i8(&rng, 32);
    }

    scalar_matmul_i8(a, b, ref, M, K, N);

    for (long it = 0; it < iters; it++) {
        qhblas_hvx_matrix_matrix_mpy_ab(a, b, out, M, K, N);
    }

    int ok = memcmp(out, ref, M * N * sizeof(int8_t)) == 0;
    return bench_report("bench_matmul_i8", ok);
}
