/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * int8 relu benchmark. No qhl_hvx relu exists, so this is hand-written
 * as a single Q6_Vb_vmax_VbVb(x, 0) HVX loop.
 */

#include <stdint.h>
#include <stdlib.h>
#include "hvx_internal.h"
#include "bench_common.h"
#include "scalar_ref.h"

#define NELEM 4096u

static void hvx_relu_i8(const int8_t *in, int8_t *out, uint32_t n)
{
    HVX_Vector zero = Q6_V_vzero();
    for (uint32_t i = 0; i < n; i += VLEN) {
        HVX_Vector v = vmemu(in + i);
        vmemu(out + i) = Q6_Vb_vmax_VbVb(v, zero);
    }
}

int main(int argc, char **argv)
{
    long iters = bench_parse_n(argc, argv);
    bench_rng_t rng;
    bench_rng_seed(&rng, 0x1111u);

    int8_t *in = aligned_alloc(128, NELEM * sizeof(int8_t));
    int8_t *out = aligned_alloc(128, NELEM * sizeof(int8_t));
    int8_t *ref = malloc(NELEM * sizeof(int8_t));

    for (uint32_t i = 0; i < NELEM; i++) {
        in[i] = bench_rand_i8(&rng, 100);
    }

    scalar_relu_i8(in, ref, NELEM);

    for (long it = 0; it < iters; it++) {
        hvx_relu_i8(in, out, NELEM);
    }

    int ok = 1;
    for (uint32_t i = 0; i < NELEM && ok; i++) {
        ok = out[i] == ref[i];
    }
    return bench_report("bench_relu_i8", ok);
}
