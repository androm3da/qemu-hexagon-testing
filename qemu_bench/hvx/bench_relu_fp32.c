/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * fp32 relu benchmark. No qhl_hvx relu exists, so this is hand-written
 * as a single Q6_Vsf_vmax_VsfVsf(x, 0) HVX loop.
 */

#include <stdint.h>
#include <stdlib.h>
#include "hvx_internal.h"
#include "bench_common.h"
#include "scalar_ref.h"

#define NELEM 4096u

static void hvx_relu_f32(const float *in, float *out, uint32_t n)
{
    HVX_Vector zero = Q6_V_vzero();
    for (uint32_t i = 0; i < n; i += VLEN_WORD) {
        HVX_Vector v = vmemu(in + i);
        vmemu(out + i) = Q6_Vsf_vmax_VsfVsf(v, zero);
    }
}

int main(int argc, char **argv)
{
    long iters = bench_parse_n(argc, argv);
    bench_rng_t rng;
    bench_rng_seed(&rng, 0x3333u);

    float *in = aligned_alloc(128, NELEM * sizeof(float));
    float *out = aligned_alloc(128, NELEM * sizeof(float));
    float *ref = malloc(NELEM * sizeof(float));

    for (uint32_t i = 0; i < NELEM; i++) {
        in[i] = bench_rand_f32(&rng, 4.0f);
    }

    scalar_relu_f32(in, ref, NELEM);

    for (long it = 0; it < iters; it++) {
        hvx_relu_f32(in, out, NELEM);
    }

    int ok = 1;
    for (uint32_t i = 0; i < NELEM && ok; i++) {
        ok = out[i] == ref[i];
    }
    return bench_report("bench_relu_fp32", ok);
}
