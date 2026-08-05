/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * int8 HMX matmul benchmark: one HMX byte matmul instruction computes
 * output[s][oc] = sum_ic activation[s][ic] * weight[oc][ic] for a
 * 64-spatial x 32-input-channel x 32-output-channel.
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include "hmx_intrinsics.h"
#include "bench_common.h"

#define SPATIAL 64u
#define ICH     32u
#define OCH     32u
#define WEIGHT_VAL 5

#define VTCM_SIZE     0x10000
#define VTCM_ACT_OFF  0x0000
#define VTCM_WEI_OFF  0x1000
#define VTCM_BIAS_OFF 0x2000
#define VTCM_OUT_OFF  0x3000

/* SM crouton byte offset for spatial s, channel c. */
static int crouton_off_sm(int spatial, int channel)
{
    return ((spatial >> 2) << 7) | (channel << 2) | (spatial & 3);
}

static uint64_t pack_fxp_bias(int32_t input_bias, uint16_t exponent,
                               uint16_t shape, uint16_t scale,
                               uint16_t out_bias)
{
    uint64_t raw = 0;

    raw |= ((uint64_t)(uint32_t)input_bias) << 32;
    raw |= (scale >> 1) & 0x3FF;
    raw |= (uint64_t)(exponent & 0x1F) << 10;
    raw |= (uint64_t)((shape >> 2) & 1) << 15;
    raw |= (uint64_t)(((~scale) >> 11) & 1) << 16;
    raw |= (uint64_t)(shape & 3) << 17;
    raw |= (uint64_t)(out_bias & 7) << 19;
    raw |= (uint64_t)((out_bias >> 3) & 1) << 22;
    raw |= (uint64_t)((~out_bias >> 4) & 0xFF) << 23;
    raw |= (uint64_t)(scale & 1) << 31;
    return raw;
}

static void write_bias_mxmem2(uint8_t *bias_vtcm, uint64_t *vals, int count)
{
    uint32_t *lo = (uint32_t *)bias_vtcm;
    uint32_t *hi = (uint32_t *)(bias_vtcm + 128);

    for (int i = 0; i < count; i++) {
        lo[i] = (uint32_t)vals[i];
        hi[i] = (uint32_t)(vals[i] >> 32);
    }
}

static void hmx_matmul_ub_sm(uintptr_t act, uint32_t ar, uintptr_t wei,
                              uint32_t wr)
{
    _HMX_PAIRED(_HMX_ACT_UB_PAIRED, _HMX_WEI_B_PAIRED, act, ar, wei, wr);
}

int main(int argc, char **argv)
{
    long iters = bench_parse_n(argc, argv);
    bench_rng_t rng;
    bench_rng_seed(&rng, 0xa1a1u);

    uint8_t *base = aligned_alloc(0x10000, VTCM_SIZE);
    uint8_t *act = base + VTCM_ACT_OFF;
    uint8_t *wei = base + VTCM_WEI_OFF;
    uint8_t *bias_area = base + VTCM_BIAS_OFF;
    uint8_t *out = base + VTCM_OUT_OFF;

    for (uint32_t s = 0; s < SPATIAL; s++) {
        for (uint32_t c = 0; c < ICH; c++) {
            act[crouton_off_sm(s, c)] = (uint8_t)(bench_rng_next(&rng) & 0xFFu);
        }
    }

    memset(wei, WEIGHT_VAL, 1024);

    uint64_t bias_vals[32];
    for (int i = 0; i < 32; i++) {
        bias_vals[i] = pack_fxp_bias(0, 0, 0, 0x400, 0);
    }
    write_bias_mxmem2(bias_area, bias_vals, 32);

    for (long it = 0; it < iters; it++) {
        Q6_mxclracc();
        Q6_bias_mxmem2_A(bias_area);
        hmx_matmul_ub_sm((uintptr_t)act, 0, (uintptr_t)wei, 0);
        memset(out, 0, 2048);
        Q6_mxmem_AR_after_sat_ub(out, 0);
    }

    int ok = 1;
    for (uint32_t s = 0; s < SPATIAL && ok; s++) {
        int32_t rowsum = 0;
        for (uint32_t c = 0; c < ICH; c++) {
            rowsum += (int32_t)act[crouton_off_sm(s, c)];
        }
        int32_t expect = rowsum * WEIGHT_VAL;
        if (expect > 255) {
            expect = 255;
        } else if (expect < 0) {
            expect = 0;
        }
        for (uint32_t oc = 0; oc < OCH && ok; oc++) {
            ok = out[crouton_off_sm(s, oc)] == (uint8_t)expect;
        }
    }
    return bench_report("bench_matmul_hmx_i8", ok);
}
