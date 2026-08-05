/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * fp16 softmax benchmark: qhmath_hvx_exp_ahf for the exponential, a
 * scalar reduce-sum, then qhblas_hvx_vector_scaling_ahf to divide by
 * the sum.
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
    bench_rng_seed(&rng, 0x5555u);

    __fp16 *in = aligned_alloc(128, NELEM * sizeof(__fp16));
    __fp16 *tmp = aligned_alloc(128, NELEM * sizeof(__fp16));
    __fp16 *out = aligned_alloc(128, NELEM * sizeof(__fp16));
    __fp16 *ref = malloc(NELEM * sizeof(__fp16));

    for (uint32_t i = 0; i < NELEM; i++) {
        in[i] = (__fp16)bench_rand_f32(&rng, 4.0f);
    }

    scalar_softmax_f16(in, ref, NELEM);

    for (long it = 0; it < iters; it++) {
        qhmath_hvx_exp_ahf(in, tmp, NELEM);
        float sum = 0.0f;
        for (uint32_t i = 0; i < NELEM; i++) {
            sum += (float)tmp[i];
        }
        __fp16 inv_sum = (__fp16)(1.0f / sum);
        qhblas_hvx_vector_scaling_ahf(tmp, &inv_sum, out, NELEM);
    }

    int ok = 1;
    for (uint32_t i = 0; i < NELEM && ok; i++) {
        ok = bench_compare_f32((float)out[i], (float)ref[i], 0.02f);
    }
    return bench_report("bench_softmax_fp16", ok);
}
