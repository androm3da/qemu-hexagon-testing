/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 *
 * Event vector table used by the packet-semantics tests.
 *
 * The three events that matter to them -- precise error (2), instruction
 * TLB miss (4) and data TLB miss (6) -- call Rust hooks (src/pkt/replay.rs)
 * with SSR.EX still set, on the interrupted thread's stack, preserving
 * every register the Hexagon C ABI lets a callee clobber plus the
 * predicates, USR, loop and modifier registers.  The hook sees the machine
 * exactly as the faulting packet left it and may change ELR to redirect the
 * resumed thread.  Everything else is as in crt0.S: trap0/trap1 simply
 * return (semihosting is serviced by the simulator), anything unexpected
 * ends the run via coredump.
 */
    .text
    .p2align 12, 0
    .globl pkt_vectors
    .type pkt_vectors, @function
pkt_vectors:
    jump coredump        /* 0  reset */
    jump coredump        /* 1  NMI */
    jump pkt_err_entry   /* 2  precise error */
    jump coredump        /* 3  reserved */
    jump pkt_tlbx_entry  /* 4  TLB miss X */
    jump coredump        /* 5  reserved */
    jump pkt_tlbrw_entry /* 6  TLB miss RW */
    jump coredump        /* 7  reserved */
    jump pkt_rte         /* 8  trap0 */
    jump pkt_rte         /* 9  trap1 */
    .rept 38
    jump coredump        /* 10..47 reserved, interrupts */
    .endr
    .size pkt_vectors, . - pkt_vectors

    .p2align 4
pkt_rte:
    rte

    .macro PKT_HOOK_ENTRY entry, hook
    .p2align 4
\entry:
    allocframe(#112)
    {
        memd(sp+#0)  = r1:0
        memd(sp+#8)  = r3:2
    }
    {
        memd(sp+#16) = r5:4
        memd(sp+#24) = r7:6
    }
    {
        memd(sp+#32) = r9:8
        memd(sp+#40) = r11:10
    }
    {
        memd(sp+#48) = r13:12
        memd(sp+#56) = r15:14
    }
    r0 = p3:0
    r1 = usr
    r2 = sa0
    r3 = lc0
    r4 = sa1
    r5 = lc1
    r7:6 = c7:6                /* M0, M1 */
    {
        memd(sp+#64) = r1:0
        memd(sp+#72) = r3:2
    }
    {
        memd(sp+#80) = r5:4
        memd(sp+#88) = r7:6
    }
    memw(sp+#96) = r28
    r0 = sp
    memw(##pkt_hook_frame) = r0
    call \hook
    r28 = memw(sp+#96)
    {
        r1:0 = memd(sp+#64)
        r3:2 = memd(sp+#72)
    }
    {
        r5:4 = memd(sp+#80)
        r7:6 = memd(sp+#88)
    }
    p3:0 = r0
    usr = r1
    sa0 = r2
    lc0 = r3
    sa1 = r4
    lc1 = r5
    c7:6 = r7:6
    {
        r1:0 = memd(sp+#0)
        r3:2 = memd(sp+#8)
    }
    {
        r5:4 = memd(sp+#16)
        r7:6 = memd(sp+#24)
    }
    {
        r9:8 = memd(sp+#32)
        r11:10 = memd(sp+#40)
    }
    {
        r13:12 = memd(sp+#48)
        r15:14 = memd(sp+#56)
    }
    deallocframe
    rte
    .endm

    PKT_HOOK_ENTRY pkt_err_entry, pkt_hook_err
    PKT_HOOK_ENTRY pkt_tlbx_entry, pkt_hook_x
    PKT_HOOK_ENTRY pkt_tlbrw_entry, pkt_hook_rw


    .data
    .p2align 2
    /* Address of the register frame saved by the entry above while a hook
     * runs: r0..r15 at +0, P3:0 at +64, USR at +68. */
    .globl pkt_hook_frame
pkt_hook_frame:
    .word 0
