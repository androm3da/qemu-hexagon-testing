/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * int8 tanh benchmark. qhl_hvx has no native int8 tanh kernel, so this
 * follows the quantized-runtime dequant/op/requant pattern: dequantize to
 * float, run the HVX tanh_af kernel, requantize. Matches scalar_tanh_i8's
 * oracle in scalar_ref.c.
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

    int8_t *in = aligned_alloc(128, NELEM * sizeof(int8_t));
    int8_t *out = aligned_alloc(128, NELEM * sizeof(int8_t));
    int8_t *ref = malloc(NELEM * sizeof(int8_t));
    float *fin = aligned_alloc(128, NELEM * sizeof(float));
    float *fout = aligned_alloc(128, NELEM * sizeof(float));

    for (uint32_t i = 0; i < NELEM; i++) {
        in[i] = bench_rand_i8(&rng, 100);
    }

    scalar_tanh_i8(in, ref, NELEM);

    for (long it = 0; it < iters; it++) {
        for (uint32_t i = 0; i < NELEM; i++) {
            fin[i] = dequant_i8(in[i], QSCALE_IN);
        }
        qhmath_hvx_tanh_af(fin, fout, NELEM);
        for (uint32_t i = 0; i < NELEM; i++) {
            out[i] = requant_i8(fout[i], QSCALE_OUT, -128.0f, 127.0f);
        }
    }

    int ok = 1;
    for (uint32_t i = 0; i < NELEM && ok; i++) {
        ok = abs((int)out[i] - (int)ref[i]) <= 1;
    }
    return bench_report("bench_tanh_i8", ok);
}
