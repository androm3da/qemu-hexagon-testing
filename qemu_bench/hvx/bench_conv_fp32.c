/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * fp32 conv3x3 benchmark. No qhl_hvx float conv exists, so this is a
 * hand-written direct 3x3 HVX kernel using the qfloat accumulation
 * pattern from qhblas_hvx_matrix_matrix_mpy_af.c (per-tap scalar splat,
 * Q6_Vqf32_vmpy_VsfVsf / Q6_Vqf32_vadd_Vqf32Vqf32, converted back with
 * Q6_Vsf_equals_Vqf32).
 *
 * Each row is stored with PAD zero columns on both sides so that the
 * unaligned +-1 column taps read zero at the image border, matching
 * scalar_conv3x3_f32's zero-padded boundary handling exactly.
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

static inline int32_t float_to_bits(float x)
{
    union {
        float f;
        int32_t i;
    } u = {.f = x};
    return u.i;
}

static void hvx_conv3x3_f32(const float *padded, uint32_t width,
                             uint32_t height, const float *mask,
                             float *output)
{
    for (uint32_t row = 1; row + 1 < height; row++) {
        const float *r0 = padded + (row - 1) * STRIDE + PAD;
        const float *r1 = padded + row * STRIDE + PAD;
        const float *r2 = padded + (row + 1) * STRIDE + PAD;
        float *out_row = output + row * width;

        for (uint32_t col = 0; col < width; col += VLEN_WORD) {
            HVX_Vector acc = Q6_V_vzero();
            const float *rows[3] = {r0, r1, r2};
            for (int32_t dy = 0; dy < 3; dy++) {
                for (int32_t dx = -1; dx <= 1; dx++) {
                    HVX_Vector line =
                        vmemu(rows[dy] + col + dx);
                    HVX_Vector tap = Q6_V_vsplat_R(
                        float_to_bits(mask[dy * 3 + (dx + 1)]));
                    HVX_Vector term = Q6_Vqf32_vmpy_VsfVsf(line, tap);
                    acc = Q6_Vqf32_vadd_Vqf32Vqf32(acc, term);
                }
            }
            vmemu(out_row + col) = Q6_Vsf_equals_Vqf32(acc);
        }
    }
}

int main(int argc, char **argv)
{
    long iters = bench_parse_n(argc, argv);
    bench_rng_t rng;
    bench_rng_seed(&rng, 0xf00du);

    float *padded = aligned_alloc(128, STRIDE * HEIGHT * sizeof(float));
    float *out = aligned_alloc(128, WIDTH * HEIGHT * sizeof(float));
    float *plain = malloc(WIDTH * HEIGHT * sizeof(float));
    float *ref = malloc(WIDTH * HEIGHT * sizeof(float));
    float mask[9];

    memset(padded, 0, STRIDE * HEIGHT * sizeof(float));
    for (uint32_t row = 0; row < HEIGHT; row++) {
        for (uint32_t col = 0; col < WIDTH; col++) {
            float v = bench_rand_f32(&rng, 4.0f);
            padded[row * STRIDE + PAD + col] = v;
            plain[row * WIDTH + col] = v;
        }
    }
    for (uint32_t i = 0; i < 9; i++) {
        mask[i] = bench_rand_f32(&rng, 1.0f);
    }

    scalar_conv3x3_f32(plain, WIDTH, HEIGHT, mask, ref);

    for (long it = 0; it < iters; it++) {
        hvx_conv3x3_f32(padded, WIDTH, HEIGHT, mask, out);
    }

    int ok = 1;
    for (uint32_t row = 1; row + 1 < HEIGHT && ok; row++) {
        for (uint32_t col = 0; col < WIDTH && ok; col++) {
            ok = bench_compare_f32(out[row * WIDTH + col],
                                    ref[row * WIDTH + col], 1e-4f);
        }
    }
    return bench_report("bench_conv_fp32", ok);
}
