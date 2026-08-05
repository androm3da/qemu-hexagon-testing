/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * fp16 relu benchmark. No qhl_hvx relu exists, so this is hand-written
 * as a single Q6_Vhf_vmax_VhfVhf(x, 0) HVX loop.
 */

#include <stdint.h>
#include <stdlib.h>
#include "hvx_internal.h"
#include "bench_common.h"
#include "scalar_ref.h"

#define NELEM 4096u

static void hvx_relu_f16(const __fp16 *in, __fp16 *out, uint32_t n)
{
    HVX_Vector zero = Q6_V_vzero();
    for (uint32_t i = 0; i < n; i += VLEN_SHORT) {
        HVX_Vector v = vmemu(in + i);
        vmemu(out + i) = Q6_Vhf_vmax_VhfVhf(v, zero);
    }
}

int main(int argc, char **argv)
{
    long iters = bench_parse_n(argc, argv);
    bench_rng_t rng;
    bench_rng_seed(&rng, 0x2222u);

    __fp16 *in = aligned_alloc(128, NELEM * sizeof(__fp16));
    __fp16 *out = aligned_alloc(128, NELEM * sizeof(__fp16));
    __fp16 *ref = malloc(NELEM * sizeof(__fp16));

    for (uint32_t i = 0; i < NELEM; i++) {
        in[i] = (__fp16)bench_rand_f32(&rng, 4.0f);
    }

    scalar_relu_f16(in, ref, NELEM);

    for (long it = 0; it < iters; it++) {
        hvx_relu_f16(in, out, NELEM);
    }

    int ok = 1;
    for (uint32_t i = 0; i < NELEM && ok; i++) {
        ok = bench_compare_f32((float)out[i], (float)ref[i], 1e-4f);
    }
    return bench_report("bench_relu_fp16", ok);
}
