/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * fp16 tanh benchmark: wraps qhmath_hvx_tanh_ahf.
 */

#include <stdint.h>
#include <stdlib.h>
#include "qhmath_hvx.h"
#include "bench_common.h"
#include "scalar_ref.h"

#define NELEM 4096u

int main(int argc, char **argv)
{
    long iters = bench_parse_n(argc, argv);
    bench_rng_t rng;
    bench_rng_seed(&rng, 0x5678u);

    __fp16 *in = aligned_alloc(128, NELEM * sizeof(__fp16));
    __fp16 *out = aligned_alloc(128, NELEM * sizeof(__fp16));
    __fp16 *ref = malloc(NELEM * sizeof(__fp16));

    for (uint32_t i = 0; i < NELEM; i++) {
        in[i] = (__fp16)bench_rand_f32(&rng, 4.0f);
    }

    scalar_tanh_f16(in, ref, NELEM);

    for (long it = 0; it < iters; it++) {
        qhmath_hvx_tanh_ahf(in, out, NELEM);
    }

    int ok = 1;
    for (uint32_t i = 0; i < NELEM && ok; i++) {
        ok = bench_compare_f32((float)out[i], (float)ref[i], 0.02f);
    }
    return bench_report("bench_tanh_fp16", ok);
}
