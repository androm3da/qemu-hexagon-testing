/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * fp16 HMX matmul benchmark: HMX hf matmul over full-depth
 * (32 spatial x 32 input-channel x 16-bit).
 * 16 weight vectors (one 128B/64-halfword vector per
 * 2 input channels.
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include "hmx_intrinsics.h"
#include "bench_common.h"

#define SPATIAL 32u
#define ICH     32u
#define OCH     32u
#define WEIGHT_VECS 16u
#define WEIGHT_VAL_HF 0x3800u /* fp16 0.5: keeps the 32-term sum in range */

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

static uint64_t pack_fp_bias(uint16_t scale_fp16, uint16_t out_bias_fp16,
                              uint8_t scale_extra, uint8_t out_bias_extra,
                              uint8_t shape, uint8_t negate,
                              uint8_t acc_bias_extra, uint16_t acc_bias_fp16)
{
    uint64_t raw = 0;

    raw |= (uint64_t)scale_fp16;
    raw |= (uint64_t)out_bias_fp16 << 16;
    raw |= (uint64_t)(scale_extra & 0xF) << 32;
    raw |= (uint64_t)(out_bias_extra & 0xF) << 36;
    raw |= (uint64_t)(shape & 0x3) << 40;
    raw |= (uint64_t)(negate & 0x1) << 42;
    raw |= (uint64_t)(acc_bias_extra & 0x1F) << 43;
    raw |= (uint64_t)acc_bias_fp16 << 48;
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

static void hmx_matmul_hf_sm(uintptr_t act, uint32_t ar, uintptr_t wei,
                              uint32_t wr)
{
    _HMX_PAIRED(_HMX_ACT_HF_PAIRED, _HMX_WEI_HF_PAIRED, act, ar, wei, wr);
}

static uint32_t spatial_major_convert(uint32_t val)
{
    uint32_t spatial_bits = (val >> 5) & 0x3;
    uint32_t depth_bits = (val & 0x1F) << 2;

    val &= ~0x7Fu;
    return val | depth_bits | spatial_bits;
}

static uint32_t gen_act_range_sm(int input_depth)
{
    uint32_t dc0 = ((input_depth - 1) & 0x1F) & ~1u;
    uint32_t temp = dc0 | (7u << 8);

    return spatial_major_convert(temp);
}

static float fp16_bits_to_f32(uint16_t h)
{
    uint32_t sign = (uint32_t)(h >> 15) & 1u;
    uint32_t exp = (h >> 10) & 0x1Fu;
    uint32_t mant = h & 0x3FFu;
    union { uint32_t u; float f; } out;

    if (exp == 0) {
        out.u = sign << 31;
        return out.f * (float)mant / 1024.0f;
    }
    uint32_t f32_exp = exp - 15 + 127;
    uint32_t f32_mant = mant << 13;
    out.u = (sign << 31) | (f32_exp << 23) | f32_mant;
    return out.f;
}

int main(int argc, char **argv)
{
    long iters = bench_parse_n(argc, argv);
    bench_rng_t rng;
    bench_rng_seed(&rng, 0xb2b2u);

    uint8_t *base = aligned_alloc(0x10000, VTCM_SIZE);
    uint8_t *act = base + VTCM_ACT_OFF;
    uint8_t *wei = base + VTCM_WEI_OFF;
    uint8_t *bias_area = base + VTCM_BIAS_OFF;
    uint8_t *out = base + VTCM_OUT_OFF;

    memset(act, 0, 2048);
    float act_f32[SPATIAL][ICH];
    for (uint32_t s = 0; s < SPATIAL; s++) {
        for (uint32_t c = 0; c < ICH; c++) {
            /* Small positive fp16 values, matching prng_fp16_small's
             * exponent range in hmx_isa_test.c, so the 32-term sum
             * against WEIGHT_VAL_HF (0.5) stays within fp16 range. */
            uint16_t r = (uint16_t)(bench_rng_next(&rng) & 0x3FFu);
            uint16_t h = (uint16_t)(0x3800u | r); /* exponent 14, +mantissa */
            /* *2: fp16 activation offsets must stay 2-byte aligned, per
             * hmx_isa_test.c's fill_fp16_act_random_depth. */
            uint16_t *act16 = (uint16_t *)(act + crouton_off_sm(s * 2, c));
            *act16 = h;
            act_f32[s][c] = fp16_bits_to_f32(h);
        }
    }

    uint16_t *wei16 = (uint16_t *)wei;
    for (uint32_t i = 0; i < WEIGHT_VECS * 64u; i++) {
        wei16[i] = (uint16_t)WEIGHT_VAL_HF;
    }
    float w_f32 = fp16_bits_to_f32((uint16_t)WEIGHT_VAL_HF);

    uint64_t bias_vals[32];
    for (int i = 0; i < 32; i++) {
        bias_vals[i] = pack_fp_bias(0x3C00, 0, 0, 0, 0, 0, 0, 0);
    }
    write_bias_mxmem2(bias_area, bias_vals, 32);

    uint32_t wei_range = (WEIGHT_VECS - 1) << 7;
    uint32_t act_range = gen_act_range_sm(ICH);

    for (long it = 0; it < iters; it++) {
        Q6_mxclracc_hf();
        Q6_bias_mxmem2_A(bias_area);
        hmx_matmul_hf_sm((uintptr_t)act, act_range, (uintptr_t)wei, wei_range);
        memset(out, 0, 2048);
        Q6_mxmem_AR_after_hf(out, 0);
    }

    int ok = 1;
    for (uint32_t s = 0; s < SPATIAL && ok; s++) {
        float sum = 0.0f;
        for (uint32_t c = 0; c < ICH; c++) {
            sum += act_f32[s][c];
        }
        float expect = sum * w_f32;
        for (uint32_t oc = 0; oc < OCH && ok; oc++) {
            uint16_t *out16 = (uint16_t *)(out + crouton_off_sm(s * 2, oc));
            float got = fp16_bits_to_f32(*out16);
            ok = bench_compare_f32(got, expect, 0.02f);
        }
    }
    return bench_report("bench_matmul_hmx_fp16", ok);
}
