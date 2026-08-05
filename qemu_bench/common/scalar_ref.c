/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 */

#include "scalar_ref.h"
#include <math.h>

void scalar_matmul_i8(const int8_t *a, const int8_t *b, int8_t *out,
                       uint32_t m, uint32_t k, uint32_t n)
{
    for (uint32_t i = 0; i < m; i++) {
        for (uint32_t j = 0; j < n; j++) {
            int32_t acc = 0;
            for (uint32_t p = 0; p < k; p++) {
                acc += (int32_t)a[i * k + p] * (int32_t)b[p * n + j];
            }
            /* Matches qhblas_hvx_matrix_matrix_mpy_ab: >>7 with saturation. */
            acc >>= 7;
            if (acc > 127) {
                acc = 127;
            } else if (acc < -128) {
                acc = -128;
            }
            out[i * n + j] = (int8_t)acc;
        }
    }
}

void scalar_matmul_f16(const __fp16 *a, const __fp16 *b, __fp16 *out,
                        uint32_t m, uint32_t k, uint32_t n)
{
    for (uint32_t i = 0; i < m; i++) {
        for (uint32_t j = 0; j < n; j++) {
            float acc = 0.0f;
            for (uint32_t p = 0; p < k; p++) {
                acc += (float)a[i * k + p] * (float)b[p * n + j];
            }
            out[i * n + j] = (__fp16)acc;
        }
    }
}

void scalar_matmul_f32(const float *a, const float *b, float *out,
                        uint32_t m, uint32_t k, uint32_t n)
{
    for (uint32_t i = 0; i < m; i++) {
        for (uint32_t j = 0; j < n; j++) {
            float acc = 0.0f;
            for (uint32_t p = 0; p < k; p++) {
                acc += a[i * k + p] * b[p * n + j];
            }
            out[i * n + j] = acc;
        }
    }
}

void scalar_conv3x3_u8(const uint8_t *input, int32_t stride_i,
                        uint32_t width, uint32_t height,
                        const int8_t *mask, uint32_t shift,
                        uint8_t *output, int32_t stride_o)
{
    for (uint32_t row = 1; row + 1 < height; row++) {
        for (uint32_t col = 0; col < width; col++) {
            int32_t acc = 0;
            for (int32_t dy = -1; dy <= 1; dy++) {
                for (int32_t dx = -1; dx <= 1; dx++) {
                    int32_t mi = (dy + 1) * 3 + (dx + 1);
                    int32_t px_col = (int32_t)col + dx;
                    if (px_col < 0 || px_col >= (int32_t)width) {
                        continue;
                    }
                    uint8_t px = input[(row + dy) * stride_i + px_col];
                    acc += (int32_t)px * (int32_t)mask[mi];
                }
            }
            acc >>= shift;
            if (acc > 255) {
                acc = 255;
            } else if (acc < 0) {
                acc = 0;
            }
            output[row * stride_o + col] = (uint8_t)acc;
        }
    }
}

void scalar_conv3x3_f16(const __fp16 *input, uint32_t width, uint32_t height,
                         const float *mask, __fp16 *output)
{
    for (uint32_t row = 1; row + 1 < height; row++) {
        for (uint32_t col = 0; col < width; col++) {
            float acc = 0.0f;
            for (int32_t dy = -1; dy <= 1; dy++) {
                for (int32_t dx = -1; dx <= 1; dx++) {
                    int32_t mi = (dy + 1) * 3 + (dx + 1);
                    int32_t px_col = (int32_t)col + dx;
                    if (px_col < 0 || px_col >= (int32_t)width) {
                        continue;
                    }
                    float px = (float)input[(row + dy) * width + px_col];
                    acc += px * mask[mi];
                }
            }
            output[row * width + col] = (__fp16)acc;
        }
    }
}

void scalar_conv3x3_f32(const float *input, uint32_t width, uint32_t height,
                         const float *mask, float *output)
{
    for (uint32_t row = 1; row + 1 < height; row++) {
        for (uint32_t col = 0; col < width; col++) {
            float acc = 0.0f;
            for (int32_t dy = -1; dy <= 1; dy++) {
                for (int32_t dx = -1; dx <= 1; dx++) {
                    int32_t mi = (dy + 1) * 3 + (dx + 1);
                    int32_t px_col = (int32_t)col + dx;
                    if (px_col < 0 || px_col >= (int32_t)width) {
                        continue;
                    }
                    float px = input[(row + dy) * width + px_col];
                    acc += px * mask[mi];
                }
            }
            output[row * width + col] = acc;
        }
    }
}

void scalar_relu_i8(const int8_t *in, int8_t *out, uint32_t n)
{
    for (uint32_t i = 0; i < n; i++) {
        out[i] = in[i] > 0 ? in[i] : 0;
    }
}

void scalar_relu_f16(const __fp16 *in, __fp16 *out, uint32_t n)
{
    for (uint32_t i = 0; i < n; i++) {
        out[i] = (float)in[i] > 0.0f ? in[i] : (__fp16)0.0f;
    }
}

void scalar_relu_f32(const float *in, float *out, uint32_t n)
{
    for (uint32_t i = 0; i < n; i++) {
        out[i] = in[i] > 0.0f ? in[i] : 0.0f;
    }
}

void scalar_tanh_i8(const int8_t *in, int8_t *out, uint32_t n)
{
    for (uint32_t i = 0; i < n; i++) {
        float x = dequant_i8(in[i], QSCALE_IN);
        float y = tanhf(x);
        out[i] = requant_i8(y, QSCALE_OUT, -128.0f, 127.0f);
    }
}

void scalar_tanh_f16(const __fp16 *in, __fp16 *out, uint32_t n)
{
    for (uint32_t i = 0; i < n; i++) {
        out[i] = (__fp16)tanhf((float)in[i]);
    }
}

void scalar_tanh_f32(const float *in, float *out, uint32_t n)
{
    for (uint32_t i = 0; i < n; i++) {
        out[i] = tanhf(in[i]);
    }
}

void scalar_softmax_i8(const int8_t *in, int8_t *out, uint32_t n)
{
    float tmp[n];
    float sum = 0.0f;
    for (uint32_t i = 0; i < n; i++) {
        tmp[i] = expf(dequant_i8(in[i], QSCALE_IN));
        sum += tmp[i];
    }
    for (uint32_t i = 0; i < n; i++) {
        out[i] = requant_i8(tmp[i] / sum, QSCALE_OUT, -128.0f, 127.0f);
    }
}

void scalar_softmax_f16(const __fp16 *in, __fp16 *out, uint32_t n)
{
    float tmp[n];
    float sum = 0.0f;
    for (uint32_t i = 0; i < n; i++) {
        tmp[i] = expf((float)in[i]);
        sum += tmp[i];
    }
    for (uint32_t i = 0; i < n; i++) {
        out[i] = (__fp16)(tmp[i] / sum);
    }
}

void scalar_softmax_f32(const float *in, float *out, uint32_t n)
{
    float tmp[n];
    float sum = 0.0f;
    for (uint32_t i = 0; i < n; i++) {
        tmp[i] = expf(in[i]);
        sum += tmp[i];
    }
    for (uint32_t i = 0; i < n; i++) {
        out[i] = tmp[i] / sum;
    }
}

void scalar_avgpool_u8(const uint8_t *input, uint32_t width, uint32_t height,
                        uint8_t *output)
{
    uint32_t ow = width / 2;
    uint32_t oh = height / 2;
    for (uint32_t r = 0; r < oh; r++) {
        for (uint32_t c = 0; c < ow; c++) {
            uint32_t sum = input[(2 * r) * width + 2 * c] +
                            input[(2 * r) * width + 2 * c + 1] +
                            input[(2 * r + 1) * width + 2 * c] +
                            input[(2 * r + 1) * width + 2 * c + 1];
            output[r * ow + c] = (uint8_t)((sum + 2) / 4);
        }
    }
}

void scalar_avgpool_f16(const __fp16 *input, uint32_t width, uint32_t height,
                         __fp16 *output)
{
    uint32_t ow = width / 2;
    uint32_t oh = height / 2;
    for (uint32_t r = 0; r < oh; r++) {
        for (uint32_t c = 0; c < ow; c++) {
            float sum = (float)input[(2 * r) * width + 2 * c] +
                        (float)input[(2 * r) * width + 2 * c + 1] +
                        (float)input[(2 * r + 1) * width + 2 * c] +
                        (float)input[(2 * r + 1) * width + 2 * c + 1];
            output[r * ow + c] = (__fp16)(sum / 4.0f);
        }
    }
}

void scalar_avgpool_f32(const float *input, uint32_t width, uint32_t height,
                         float *output)
{
    uint32_t ow = width / 2;
    uint32_t oh = height / 2;
    for (uint32_t r = 0; r < oh; r++) {
        for (uint32_t c = 0; c < ow; c++) {
            float sum = input[(2 * r) * width + 2 * c] +
                        input[(2 * r) * width + 2 * c + 1] +
                        input[(2 * r + 1) * width + 2 * c] +
                        input[(2 * r + 1) * width + 2 * c + 1];
            output[r * ow + c] = sum / 4.0f;
        }
    }
}
