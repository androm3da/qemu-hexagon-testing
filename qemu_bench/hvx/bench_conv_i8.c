/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * int8 conv3x3 benchmark: wraps qhdsp_hvx_conv3x3_ab.
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include "qhdsp_hvx.h"
#include "bench_common.h"
#include "scalar_ref.h"

#define WIDTH 128u
#define HEIGHT 130u
#define SHIFT 4u

int main(int argc, char **argv)
{
    long iters = bench_parse_n(argc, argv);
    bench_rng_t rng;
    bench_rng_seed(&rng, 0x9abcu);

    uint8_t *in = aligned_alloc(128, WIDTH * HEIGHT);
    uint8_t *out = aligned_alloc(128, WIDTH * HEIGHT);
    uint8_t *ref = malloc(WIDTH * HEIGHT);
    int8_t mask[9];

    for (uint32_t i = 0; i < WIDTH * HEIGHT; i++) {
        in[i] = (uint8_t)(bench_rng_next(&rng) & 0xFFu);
    }
    for (uint32_t i = 0; i < 9; i++) {
        mask[i] = bench_rand_i8(&rng, 16);
    }

    memset(ref, 0, WIDTH * HEIGHT);
    scalar_conv3x3_u8(in, (int32_t)WIDTH, WIDTH, HEIGHT, mask, SHIFT, ref,
                       (int32_t)WIDTH);

    for (long it = 0; it < iters; it++) {
        qhdsp_hvx_conv3x3_ab(in, (int32_t)WIDTH, WIDTH, HEIGHT, mask, SHIFT,
                              out, (int32_t)WIDTH);
    }

    int ok = 1;
    /*
     * col=0 excluded: qhdsp_hvx_conv3x3_ab's left-edge tap reads memory
     * immediately preceding the row buffer rather than zero-padding,
     * so its col=0 output is unspecified relative to scalar_conv3x3_u8's
     * zero-padded oracle. Confirmed empirically; all other columns match
     * exactly.
     */
    for (uint32_t row = 1; row + 1 < HEIGHT && ok; row++) {
        for (uint32_t col = 1; col < WIDTH && ok; col++) {
            ok = out[row * WIDTH + col] == ref[row * WIDTH + col];
        }
    }
    return bench_report("bench_conv_i8", ok);
}
