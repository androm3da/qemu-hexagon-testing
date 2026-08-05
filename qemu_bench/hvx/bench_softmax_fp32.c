/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * fp32 softmax benchmark: qhmath_hvx_exp_af for the exponential, a
 * scalar reduce-sum (softmax's sum is inherently a whole-vector
 * reduction, not itself a benchmark target here), then
 * qhblas_hvx_vector_scaling_af to divide by the sum.
 */

#include <stdint.h>
#include <stdlib.h>
#include "qhmath_hvx.h"
#include "qhblas_hvx.h"
#include "bench_common.h"
#include "scalar_ref.h"

#define NELEM 4096u

int main(int argc, char **argv)
{
    long iters = bench_parse_n(argc, argv);
    bench_rng_t rng;
    bench_rng_seed(&rng, 0x4444u);

    float *in = aligned_alloc(128, NELEM * sizeof(float));
    float *tmp = aligned_alloc(128, NELEM * sizeof(float));
    float *out = aligned_alloc(128, NELEM * sizeof(float));
    float *ref = malloc(NELEM * sizeof(float));

    for (uint32_t i = 0; i < NELEM; i++) {
        in[i] = bench_rand_f32(&rng, 4.0f);
    }

    scalar_softmax_f32(in, ref, NELEM);

    for (long it = 0; it < iters; it++) {
        qhmath_hvx_exp_af(in, tmp, NELEM);
        float sum = 0.0f;
        for (uint32_t i = 0; i < NELEM; i++) {
            sum += tmp[i];
        }
        qhblas_hvx_vector_scaling_af(tmp, 1.0f / sum, out, NELEM);
    }

    int ok = 1;
    for (uint32_t i = 0; i < NELEM && ok; i++) {
        ok = bench_compare_f32(out[i], ref[i], 0.01f);
    }
    return bench_report("bench_softmax_fp32", ok);
}
