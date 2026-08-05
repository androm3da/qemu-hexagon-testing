/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * Scalar (non-HVX) reference implementations used as the correctness
 * oracle for every qemu_bench benchmark. Each HVX/HMX kernel's output is
 * compared against the corresponding scalar_ref_* result.
 *
 * int8 quantized variants of transcendental ops (tanh, softmax) that have
 * no native low-precision HVX kernel are computed by dequantizing to
 * float, applying the float op, and requantizing -- the same
 * dequant/op/requant pattern real quantized NN runtimes use. QSCALE_IN is
 * the fixed-point scale applied to the int8 input; QSCALE_OUT is applied
 * to the float result before requantizing.
 */

#ifndef QEMU_BENCH_SCALAR_REF_H
#define QEMU_BENCH_SCALAR_REF_H

#include <stdint.h>

#define QSCALE_IN  32.0f
#define QSCALE_OUT 127.0f

static inline float dequant_i8(int8_t v, float scale)
{
    return (float)v / scale;
}

static inline int8_t requant_i8(float v, float scale, float lo, float hi)
{
    float scaled = v * scale;
    if (scaled < lo) {
        scaled = lo;
    } else if (scaled > hi) {
        scaled = hi;
    }
    return (int8_t)(scaled >= 0 ? scaled + 0.5f : scaled - 0.5f);
}

/* matmul: out[m x n] = a[m x k] * b[k x n] */
void scalar_matmul_i8(const int8_t *a, const int8_t *b, int8_t *out,
                       uint32_t m, uint32_t k, uint32_t n);
void scalar_matmul_f16(const __fp16 *a, const __fp16 *b, __fp16 *out,
                        uint32_t m, uint32_t k, uint32_t n);
void scalar_matmul_f32(const float *a, const float *b, float *out,
                        uint32_t m, uint32_t k, uint32_t n);

/*
 * conv3x3: interior pixels only (output has height-2 rows), matching
 * qhdsp_hvx_conv3x3_ab's border handling.
 */
void scalar_conv3x3_u8(const uint8_t *input, int32_t stride_i,
                        uint32_t width, uint32_t height,
                        const int8_t *mask, uint32_t shift,
                        uint8_t *output, int32_t stride_o);
void scalar_conv3x3_f16(const __fp16 *input, uint32_t width, uint32_t height,
                         const float *mask, __fp16 *output);
void scalar_conv3x3_f32(const float *input, uint32_t width, uint32_t height,
                         const float *mask, float *output);

void scalar_relu_i8(const int8_t *in, int8_t *out, uint32_t n);
void scalar_relu_f16(const __fp16 *in, __fp16 *out, uint32_t n);
void scalar_relu_f32(const float *in, float *out, uint32_t n);

void scalar_tanh_i8(const int8_t *in, int8_t *out, uint32_t n);
void scalar_tanh_f16(const __fp16 *in, __fp16 *out, uint32_t n);
void scalar_tanh_f32(const float *in, float *out, uint32_t n);

void scalar_softmax_i8(const int8_t *in, int8_t *out, uint32_t n);
void scalar_softmax_f16(const __fp16 *in, __fp16 *out, uint32_t n);
void scalar_softmax_f32(const float *in, float *out, uint32_t n);

/* avgpool: 2x2 non-overlapping window over a width x height image. */
void scalar_avgpool_u8(const uint8_t *input, uint32_t width, uint32_t height,
                        uint8_t *output);
void scalar_avgpool_f16(const __fp16 *input, uint32_t width, uint32_t height,
                         __fp16 *output);
void scalar_avgpool_f32(const float *input, uint32_t width, uint32_t height,
                         float *output);

#endif /* QEMU_BENCH_SCALAR_REF_H */
