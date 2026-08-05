/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * fp16 conv3x3 benchmark. No qhl_hvx float conv exists, so this is a
 * hand-written direct 3x3 HVX kernel using the qfloat16 accumulation
 * pattern from qhblas_hvx_matrix_matrix_mpy_ahf.c (per-tap scalar splat,
 * Q6_Vqf16_vmpy_VhfVhf / Q6_Vqf16_vadd_Vqf16Vqf16, converted back with
 * Q6_Vhf_equals_Vqf16).
 *
 * Each row is stored with PAD zero columns on both sides so that the
 * unaligned +-1 column taps read zero at the image border, matching
 * scalar_conv3x3_f16's zero-padded boundary handling exactly.
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include "hvx_internal.h"
#include "bench_common.h"
#include "scalar_ref.h"

#define WIDTH 128u
#define HEIGHT 6u
#define PAD 32u
#define STRIDE (WIDTH + 2u * PAD)

static inline int16_t float16_to_bits(const __fp16 *x)
{
    union {
        __fp16 f;
        int16_t i;
    } u = {.f = *x};
    return u.i;
}

static void hvx_conv3x3_f16(const __fp16 *padded, uint32_t width,
                             uint32_t height, const float *mask,
                             __fp16 *output)
{
    for (uint32_t row = 1; row + 1 < height; row++) {
        const __fp16 *r0 = padded + (row - 1) * STRIDE + PAD;
        const __fp16 *r1 = padded + row * STRIDE + PAD;
        const __fp16 *r2 = padded + (row + 1) * STRIDE + PAD;
        __fp16 *out_row = output + row * width;

        for (uint32_t col = 0; col < width; col += VLEN_SHORT) {
            HVX_Vector acc = Q6_V_vzero();
            const __fp16 *rows[3] = {r0, r1, r2};
            for (int32_t dy = 0; dy < 3; dy++) {
                for (int32_t dx = -1; dx <= 1; dx++) {
                    HVX_Vector line =
                        vmemu(rows[dy] + col + dx);
                    __fp16 tap_val = (__fp16)mask[dy * 3 + (dx + 1)];
                    HVX_Vector tap = Q6_Vh_vsplat_R(
                        float16_to_bits(&tap_val));
                    HVX_Vector term = Q6_Vqf16_vmpy_VhfVhf(line, tap);
                    acc = Q6_Vqf16_vadd_Vqf16Vqf16(acc, term);
                }
            }
            vmemu(out_row + col) = Q6_Vhf_equals_Vqf16(acc);
        }
    }
}

int main(int argc, char **argv)
{
    long iters = bench_parse_n(argc, argv);
    bench_rng_t rng;
    bench_rng_seed(&rng, 0xf00eu);

    __fp16 *padded = aligned_alloc(128, STRIDE * HEIGHT * sizeof(__fp16));
    __fp16 *out = aligned_alloc(128, WIDTH * HEIGHT * sizeof(__fp16));
    __fp16 *plain = malloc(WIDTH * HEIGHT * sizeof(__fp16));
    __fp16 *ref = malloc(WIDTH * HEIGHT * sizeof(__fp16));
    float mask[9];

    memset(padded, 0, STRIDE * HEIGHT * sizeof(__fp16));
    for (uint32_t row = 0; row < HEIGHT; row++) {
        for (uint32_t col = 0; col < WIDTH; col++) {
            __fp16 v = (__fp16)bench_rand_f32(&rng, 4.0f);
            padded[row * STRIDE + PAD + col] = v;
            plain[row * WIDTH + col] = v;
        }
    }
    for (uint32_t i = 0; i < 9; i++) {
        mask[i] = bench_rand_f32(&rng, 1.0f);
    }

    scalar_conv3x3_f16(plain, WIDTH, HEIGHT, mask, ref);

    for (long it = 0; it < iters; it++) {
        hvx_conv3x3_f16(padded, WIDTH, HEIGHT, mask, out);
    }

    int ok = 1;
    for (uint32_t row = 1; row + 1 < HEIGHT && ok; row++) {
        for (uint32_t col = 0; col < WIDTH && ok; col++) {
            ok = bench_compare_f32((float)out[row * WIDTH + col],
                                    (float)ref[row * WIDTH + col], 0.05f);
        }
    }
    return bench_report("bench_conv_fp16", ok);
}
