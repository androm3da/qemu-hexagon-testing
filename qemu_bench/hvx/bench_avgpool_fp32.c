/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * fp32 2x2 avg-pool benchmark. No qhl_hvx avg-pool exists, so this is
 * hand-written: the vertical (row-pair) add is done on HVX via
 * Q6_Vqf32_vadd_VsfVsf across the full row width (the dominant cost,
 * since it's proportional to width rather than width/2), then the
 * horizontal (column-pair) combine + divide-by-4 is a scalar pass over
 * the already-vertically-summed row.
 */

#include <stdint.h>
#include <stdlib.h>
#include "hvx_internal.h"
#include "bench_common.h"
#include "scalar_ref.h"

#define WIDTH 128u
#define HEIGHT 128u

static void hvx_avgpool_f32(const float *input, uint32_t width,
                             uint32_t height, float *output)
{
    uint32_t ow = width / 2;
    float vsum[WIDTH];

    for (uint32_t r = 0; r < height / 2; r++) {
        const float *r0 = input + (2 * r) * width;
        const float *r1 = input + (2 * r + 1) * width;

        for (uint32_t col = 0; col < width; col += VLEN_WORD) {
            HVX_Vector v0 = vmemu(r0 + col);
            HVX_Vector v1 = vmemu(r1 + col);
            HVX_Vector sum = Q6_Vsf_equals_Vqf32(Q6_Vqf32_vadd_VsfVsf(v0, v1));
            vmemu(vsum + col) = sum;
        }

        float *out_row = output + r * ow;
        for (uint32_t c = 0; c < ow; c++) {
            out_row[c] = (vsum[2 * c] + vsum[2 * c + 1]) / 4.0f;
        }
    }
}

int main(int argc, char **argv)
{
    long iters = bench_parse_n(argc, argv);
    bench_rng_t rng;
    bench_rng_seed(&rng, 0x8888u);

    float *in = aligned_alloc(128, WIDTH * HEIGHT * sizeof(float));
    float *out = aligned_alloc(128, (WIDTH / 2) * (HEIGHT / 2) * sizeof(float));
    float *ref = malloc((WIDTH / 2) * (HEIGHT / 2) * sizeof(float));

    for (uint32_t i = 0; i < WIDTH * HEIGHT; i++) {
        in[i] = bench_rand_f32(&rng, 4.0f);
    }

    scalar_avgpool_f32(in, WIDTH, HEIGHT, ref);

    for (long it = 0; it < iters; it++) {
        hvx_avgpool_f32(in, WIDTH, HEIGHT, out);
    }

    int ok = 1;
    for (uint32_t i = 0; i < (WIDTH / 2) * (HEIGHT / 2) && ok; i++) {
        ok = bench_compare_f32(out[i], ref[i], 1e-4f);
    }
    return bench_report("bench_avgpool_fp32", ok);
}
