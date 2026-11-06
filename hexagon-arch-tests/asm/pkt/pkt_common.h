/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * Shared definitions for the table-driven packet-semantics cases.  The
 * numbers here are mirrored in src/pkt/mod.rs and pktgen/mod.rs.
 */
#ifndef PKT_COMMON_H
#define PKT_COMMON_H

#define ST_P    28
#define ST_USR  29
#define ST_M0   30
#define ST_M1   31
#define ST_CS0  32
#define ST_CS1  33
#define ST_GP   34
#define ST_LR   35
#define ST_SA0  36
#define ST_LC0  37
#define ST_SA1  38
#define ST_LC1  39

#define IN_OFF    0
#define OUT_OFF   160
#define FN_OFF    320
#define RET_OFF   324
#define SAVE_OFF  328
#define VPTR_OFF  368

/*
 * Case wrapper.  On entry r28 = ctx and r31 = return address into
 * pkt_run.  The prologue swaps the real return address for the case's
 * input LR; the epilogue records the final LR and restores the return
 * address.
 */
    .macro PKT_CASE_BEGIN name
    .p2align 2
    .globl \name
    .type \name, @function
\name:
    {
        memw(r28+#RET_OFF) = r31
        r31 = memw(r28+#(IN_OFF+ST_LR*4))
    }
    {
        r27 = memw(r28+#(IN_OFF+27*4))
    }
    .endm

    .macro PKT_CASE_END name
    {
        memw(r28+#(OUT_OFF+ST_LR*4)) = r31
    }
    {
        r31 = memw(r28+#RET_OFF)
    }
    {
        jumpr r31
    }
\name\()_end:
    .size \name, .-\name
    .endm

#endif
