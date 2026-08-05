/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * fp32 matmul benchmark: wraps qhblas_hvx_matrix_matrix_mpy_af.
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

    float *a = aligned_alloc(128, M * K * sizeof(float));
    float *b = aligned_alloc(128, K * N * sizeof(float));
    float *out = aligned_alloc(128, M * N * sizeof(float));
    float *ref = malloc(M * N * sizeof(float));

    for (uint32_t i = 0; i < M * K; i++) {
        a[i] = bench_rand_f32(&rng, 4.0f);
    }
    for (uint32_t i = 0; i < K * N; i++) {
        b[i] = bench_rand_f32(&rng, 4.0f);
    }

    scalar_matmul_f32(a, b, ref, M, K, N);

    for (long it = 0; it < iters; it++) {
        qhblas_hvx_matrix_matrix_mpy_af(a, b, out, M, K, N);
    }

    int ok = 1;
    for (uint32_t i = 0; i < M * N && ok; i++) {
        ok = bench_compare_f32(out[i], ref[i], 1e-4f);
    }
    return bench_report("bench_matmul_fp32", ok);
}
