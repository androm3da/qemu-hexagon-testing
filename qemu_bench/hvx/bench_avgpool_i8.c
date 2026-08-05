/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * uint8 2x2 avg-pool benchmark. No qhl_hvx avg-pool exists, so this is
 * hand-written: per row-pair, Q6_Wh_vadd_VubVub widens+adds the two
 * rows byte-lane-wise into a VectorPair whose lo/hi halves are the
 * even/odd-indexed 16-bit widened bytes; Q6_Vh_vadd_VhVh(lo, hi) then
 * sums each adjacent even/odd (i.e. column) pair in one instruction,
 * yielding the full 2x2 window sum per output column, fully
 * vectorized. Only the final +2 (round) and >>2 (divide by 4) +
 * pack-to-8-bit are scalar.
 */

#include <stdint.h>
#include <stdlib.h>
#include "hvx_internal.h"
#include "bench_common.h"
#include "scalar_ref.h"

#define WIDTH 128u
#define HEIGHT 128u

static void hvx_avgpool_u8(const uint8_t *input, uint32_t width,
                            uint32_t height, uint8_t *output)
{
    uint32_t ow = width / 2;
    uint16_t wsum[WIDTH / 2];

    for (uint32_t r = 0; r < height / 2; r++) {
        const uint8_t *r0 = input + (2 * r) * width;
        const uint8_t *r1 = input + (2 * r + 1) * width;

        for (uint32_t col = 0; col < width; col += VLEN) {
            HVX_Vector v0 = vmemu(r0 + col);
            HVX_Vector v1 = vmemu(r1 + col);
            HVX_VectorPair vsum_pair = Q6_Wh_vadd_VubVub(v0, v1);
            HVX_Vector wsum_vec =
                Q6_Vh_vadd_VhVh(Q6_V_lo_W(vsum_pair), Q6_V_hi_W(vsum_pair));
            vmemu(wsum + col / 2) = wsum_vec;
        }

        uint8_t *out_row = output + r * ow;
        for (uint32_t c = 0; c < ow; c++) {
            out_row[c] = (uint8_t)((wsum[c] + 2) / 4);
        }
    }
}

int main(int argc, char **argv)
{
    long iters = bench_parse_n(argc, argv);
    bench_rng_t rng;
    bench_rng_seed(&rng, 0x7777u);

    uint8_t *in = aligned_alloc(128, WIDTH * HEIGHT);
    uint8_t *out = aligned_alloc(128, (WIDTH / 2) * (HEIGHT / 2));
    uint8_t *ref = malloc((WIDTH / 2) * (HEIGHT / 2));

    for (uint32_t i = 0; i < WIDTH * HEIGHT; i++) {
        in[i] = (uint8_t)(bench_rng_next(&rng) & 0xFFu);
    }

    scalar_avgpool_u8(in, WIDTH, HEIGHT, ref);

    for (long it = 0; it < iters; it++) {
        hvx_avgpool_u8(in, WIDTH, HEIGHT, out);
    }

    int ok = 1;
    for (uint32_t i = 0; i < (WIDTH / 2) * (HEIGHT / 2) && ok; i++) {
        ok = out[i] == ref[i];
    }
    return bench_report("bench_avgpool_i8", ok);
}
