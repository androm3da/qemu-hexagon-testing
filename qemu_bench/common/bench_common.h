/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 *
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 */

#ifndef QEMU_BENCH_COMMON_H
#define QEMU_BENCH_COMMON_H

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <math.h>

/*
 * argv[1], if present, is the repeat count N (how many times the
 * benchmarked op runs). Defaults to 1.
 */
static inline long bench_parse_n(int argc, char **argv)
{
    if (argc < 2) {
        return 1;
    }
    long n = strtol(argv[1], NULL, 10);
    return n > 0 ? n : 1;
}

/* xorshift32: small, deterministic, no libc rand() dependency. */
typedef struct {
    uint32_t state;
} bench_rng_t;

static inline void bench_rng_seed(bench_rng_t *rng, uint32_t seed)
{
    rng->state = seed ? seed : 1;
}

static inline uint32_t bench_rng_next(bench_rng_t *rng)
{
    uint32_t x = rng->state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    rng->state = x;
    return x;
}

/* Signed int8 in [-range, range]. */
static inline int8_t bench_rand_i8(bench_rng_t *rng, int range)
{
    int32_t v = (int32_t)(bench_rng_next(rng) % (uint32_t)(2 * range + 1));
    return (int8_t)(v - range);
}

/* Float in [-scale, scale]. */
static inline float bench_rand_f32(bench_rng_t *rng, float scale)
{
    float unit = (float)(bench_rng_next(rng) & 0xFFFF) / 65535.0f;
    return (unit * 2.0f - 1.0f) * scale;
}

static inline int bench_compare_i32(int32_t a, int32_t b)
{
    return a == b;
}

static inline int bench_compare_f32(float a, float b, float tol)
{
    float diff = fabsf(a - b);
    float scale = fmaxf(1.0f, fmaxf(fabsf(a), fabsf(b)));
    return diff <= tol * scale;
}

/*
 * Reports PASS/FAIL to stdout and returns the process exit code
 * (0 on pass, 1 on fail), so callers can `return bench_report(ok);`.
 */
static inline int bench_report(const char *name, int ok)
{
    if (ok) {
        printf("%s: PASS\n", name);
        return 0;
    }
    printf("%s: FAIL\n", name);
    return 1;
}

#endif /* QEMU_BENCH_COMMON_H */
