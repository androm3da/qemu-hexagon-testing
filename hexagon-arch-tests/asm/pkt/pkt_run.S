/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * Trampoline for table-driven packet-semantics cases.
 *
 *   void pkt_run(struct PktCtx *ctx);
 *
 * Loads r0..r27 and a set of control registers from ctx->in[], calls the
 * case function ctx->fn, then stores the same state back to ctx->out[].
 * The layout (state ids and byte offsets) is mirrored by src/pkt/mod.rs.
 *
 *   state id  0..27  r0..r27
 *             28     P3:0
 *             29     USR
 *             30,31  M0, M1
 *             32,33  CS0, CS1
 *             34     GP
 *             35     LR (r31)
 *             36..39 SA0, LC0, SA1, LC1
 *   in[]  at byte offset 0    (40 words)
 *   out[] at byte offset 160  (40 words)
 *   fn    at byte offset 320
 *   ret   at byte offset 324  (case wrapper saves the real return address)
 *   save  at byte offset 328  (10 words: caller's USR,M0,M1,CS0,CS1,GP,
 *                              SA0,LC0,SA1,LC1)
 *
 * r28 holds the ctx pointer for the whole case and must not be touched by
 * packets under test.  r29 (SP) and r30 (FP) are likewise off limits.
 */
#include "pkt_common.h"



/*
 * HVX state (pkt_run_hvx only): ctx->vptr points at a 128-byte aligned
 * buffer of 128-byte slots: v0..v31 inputs at 0, outputs at 32, Q
 * inputs at 64..67, outputs at 68..71.  A Q register is exchanged as the
 * 128-byte vector of 0xff/0x00 bytes `vand(Q, -1)` produces.
 */
    .macro PKT_RUN name, hvx
    .text
    .p2align 4
    .globl \name
    .type \name, @function
\name:
    {
        allocframe(#64)
    }
    {
        memd(sp+#0)  = r17:16
        memd(sp+#8)  = r19:18
    }
    {
        memd(sp+#16) = r21:20
        memd(sp+#24) = r23:22
    }
    {
        memd(sp+#32) = r25:24
        memd(sp+#40) = r27:26
    }
    {
        memw(sp+#48) = r28
        r28 = r0
    }
    /* Save the caller's control registers so they can be restored.  Each
     * control-register move is slot 3 only, so one per packet. */
    { r1 = usr }
    { memw(r28+#(SAVE_OFF+0*4)) = r1 ; r1 = m0 }
    { memw(r28+#(SAVE_OFF+1*4)) = r1 ; r1 = m1 }
    { memw(r28+#(SAVE_OFF+2*4)) = r1 ; r1 = cs0 }
    { memw(r28+#(SAVE_OFF+3*4)) = r1 ; r1 = cs1 }
    { memw(r28+#(SAVE_OFF+4*4)) = r1 ; r1 = gp }
    { memw(r28+#(SAVE_OFF+5*4)) = r1 ; r1 = sa0 }
    { memw(r28+#(SAVE_OFF+6*4)) = r1 ; r1 = lc0 }
    { memw(r28+#(SAVE_OFF+7*4)) = r1 ; r1 = sa1 }
    { memw(r28+#(SAVE_OFF+8*4)) = r1 ; r1 = lc1 }
    { memw(r28+#(SAVE_OFF+9*4)) = r1 }
    /* Install the case's control-register inputs. */
    { r1 = memw(r28+#(IN_OFF+ST_USR*4)) }
    { usr = r1 ; r1 = memw(r28+#(IN_OFF+ST_P*4)) }
    { p3:0 = r1 ; r1 = memw(r28+#(IN_OFF+ST_M0*4)) }
    { m0 = r1 ; r1 = memw(r28+#(IN_OFF+ST_M1*4)) }
    { m1 = r1 ; r1 = memw(r28+#(IN_OFF+ST_CS0*4)) }
    { cs0 = r1 ; r1 = memw(r28+#(IN_OFF+ST_CS1*4)) }
    { cs1 = r1 ; r1 = memw(r28+#(IN_OFF+ST_GP*4)) }
    { gp = r1 ; r1 = memw(r28+#(IN_OFF+ST_SA0*4)) }
    { sa0 = r1 ; r1 = memw(r28+#(IN_OFF+ST_LC0*4)) }
    { lc0 = r1 ; r1 = memw(r28+#(IN_OFF+ST_SA1*4)) }
    { sa1 = r1 ; r1 = memw(r28+#(IN_OFF+ST_LC1*4)) }
    { lc1 = r1 ; r1:0 = memd(r28+#(IN_OFF+0*4)) }
    .if \hvx
    { r3 = memw(r28+#VPTR_OFF) ; r2 = #-1 }
    { r4 = add(r3, #8192) }
    .irp n,0,1,2,3
    { v0 = vmem(r4++#1) }
    { q\n = vand(v0, r2) }
    .endr
    .irp n,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    { v\n = vmem(r3++#1) }
    .endr
    .endif
    /* General registers r0..r27. */
    {
        r3:2 = memd(r28+#(IN_OFF+2*4))
        r5:4 = memd(r28+#(IN_OFF+4*4))
    }
    {
        r7:6 = memd(r28+#(IN_OFF+6*4))
        r9:8 = memd(r28+#(IN_OFF+8*4))
    }
    {
        r11:10 = memd(r28+#(IN_OFF+10*4))
        r13:12 = memd(r28+#(IN_OFF+12*4))
    }
    {
        r15:14 = memd(r28+#(IN_OFF+14*4))
        r17:16 = memd(r28+#(IN_OFF+16*4))
    }
    {
        r19:18 = memd(r28+#(IN_OFF+18*4))
        r21:20 = memd(r28+#(IN_OFF+20*4))
    }
    {
        r23:22 = memd(r28+#(IN_OFF+22*4))
        r25:24 = memd(r28+#(IN_OFF+24*4))
    }
    {
        r27:26 = memd(r28+#(IN_OFF+26*4))
    }
    /* r27 carries the case address for the call; the case's wrapper puts
     * its input value back. */
    {
        r27 = memw(r28+#FN_OFF)
    }
    {
        callr r27
    }
    /* r0..r27 hold the results.  Store them, then reuse r0 as scratch. */
    {
        memd(r28+#(OUT_OFF+0*4))   = r1:0
        memd(r28+#(OUT_OFF+2*4))   = r3:2
    }
    {
        memd(r28+#(OUT_OFF+4*4))   = r5:4
        memd(r28+#(OUT_OFF+6*4))   = r7:6
    }
    {
        memd(r28+#(OUT_OFF+8*4))   = r9:8
        memd(r28+#(OUT_OFF+10*4))  = r11:10
    }
    {
        memd(r28+#(OUT_OFF+12*4))  = r13:12
        memd(r28+#(OUT_OFF+14*4))  = r15:14
    }
    {
        memd(r28+#(OUT_OFF+16*4))  = r17:16
        memd(r28+#(OUT_OFF+18*4))  = r19:18
    }
    {
        memd(r28+#(OUT_OFF+20*4))  = r21:20
        memd(r28+#(OUT_OFF+22*4))  = r23:22
    }
    {
        memd(r28+#(OUT_OFF+24*4))  = r25:24
        memd(r28+#(OUT_OFF+26*4))  = r27:26
    }
    .if \hvx
    { r1 = memw(r28+#VPTR_OFF) ; r2 = #-1 }
    { r1 = add(r1, #4096) }
    .irp n,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    { vmem(r1++#1) = v\n }
    .endr
    { r1 = add(r1, #512) }
    .irp n,0,1,2,3
    { v0 = vand(q\n, r2) }
    { vmem(r1++#1) = v0 }
    .endr
    .endif
    { r0 = p3:0 }
    { memw(r28+#(OUT_OFF+ST_P*4)) = r0 ; r0 = usr }
    { memw(r28+#(OUT_OFF+ST_USR*4)) = r0 ; r0 = m0 }
    { memw(r28+#(OUT_OFF+ST_M0*4)) = r0 ; r0 = m1 }
    { memw(r28+#(OUT_OFF+ST_M1*4)) = r0 ; r0 = cs0 }
    { memw(r28+#(OUT_OFF+ST_CS0*4)) = r0 ; r0 = cs1 }
    { memw(r28+#(OUT_OFF+ST_CS1*4)) = r0 ; r0 = gp }
    { memw(r28+#(OUT_OFF+ST_GP*4)) = r0 ; r0 = sa0 }
    { memw(r28+#(OUT_OFF+ST_SA0*4)) = r0 ; r0 = lc0 }
    { memw(r28+#(OUT_OFF+ST_LC0*4)) = r0 ; r0 = sa1 }
    { memw(r28+#(OUT_OFF+ST_SA1*4)) = r0 ; r0 = lc1 }
    { memw(r28+#(OUT_OFF+ST_LC1*4)) = r0 ; r0 = memw(r28+#(SAVE_OFF+1*4)) }
    /* Restore the caller's control registers. */
    { m0 = r0 ; r0 = memw(r28+#(SAVE_OFF+2*4)) }
    { m1 = r0 ; r0 = memw(r28+#(SAVE_OFF+3*4)) }
    { cs0 = r0 ; r0 = memw(r28+#(SAVE_OFF+4*4)) }
    { cs1 = r0 ; r0 = memw(r28+#(SAVE_OFF+5*4)) }
    { gp = r0 ; r0 = memw(r28+#(SAVE_OFF+6*4)) }
    { sa0 = r0 ; r0 = memw(r28+#(SAVE_OFF+7*4)) }
    { lc0 = r0 ; r0 = memw(r28+#(SAVE_OFF+8*4)) }
    { sa1 = r0 ; r0 = memw(r28+#(SAVE_OFF+9*4)) }
    { lc1 = r0 ; r0 = memw(r28+#(SAVE_OFF+0*4)) }
    { usr = r0 ; r28 = memw(sp+#48) }
    {
        r17:16 = memd(sp+#0)
        r19:18 = memd(sp+#8)
    }
    {
        r21:20 = memd(sp+#16)
        r23:22 = memd(sp+#24)
    }
    {
        r25:24 = memd(sp+#32)
        r27:26 = memd(sp+#40)
    }
    {
        dealloc_return
    }
    .size \name, .-\name
    .endm

    PKT_RUN pkt_run, 0
    PKT_RUN pkt_run_hvx, 1
