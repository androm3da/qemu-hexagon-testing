// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Register read/write semantics inside a packet.
//!
//! Rule under test (architectural, hexagon-sim observed): every instruction
//! in a packet reads its source registers as they were before the packet
//! started; all results are committed together when the packet completes.
//! That includes register pairs versus their halves, post-incremented base
//! registers, and implicit writes such as the link register.

use super::*;

fn v() -> Vector {
    Vector::new()
}

fn lo(x: u64) -> u32 {
    x as u32
}
fn hi(x: u64) -> u32 {
    (x >> 32) as u32
}
fn pair(l: usize, h: usize) -> u64 {
    ((d(h) as u64) << 32) | d(l) as u64
}

pub fn cases() -> Vec<Case> {
    let mut c: Vec<Case> = Vec::new();
    let p = BASE; // window offset the pointer registers start at

    // ---- Register-to-register reads before writes -------------------------
    c.push(Case::new("swap_r0_r1", "{ r0 = r1; r1 = r0 }")
        .vec(v().exp(0, d(1)).exp(1, d(0))));
    c.push(Case::new("rotate3", "{ r0 = r1; r1 = r2; r2 = r0 }")
        .vec(v().exp(0, d(1)).exp(1, d(2)).exp(2, d(0))));
    c.push(Case::new("shift_chain4", "{ r1 = r0; r2 = r1; r3 = r2; r4 = r3 }")
        .vec(v().exp(1, d(0)).exp(2, d(1)).exp(3, d(2)).exp(4, d(3))));
    c.push(Case::new("swap_via_add", "{ r0 = add(r0, r1); r1 = sub(r0, r1) }")
        .vec(v()
            .exp(0, d(0).wrapping_add(d(1)))
            .exp(1, d(0).wrapping_sub(d(1)))));

    // ---- Register pairs versus their halves -------------------------------
    c.push(Case::new("pair_swap", "{ r1:0 = r3:2; r3:2 = r1:0 }")
        .vec(v().exp(0, d(2)).exp(1, d(3)).exp(2, d(0)).exp(3, d(1))));
    c.push(Case::new("combine_swap_self", "{ r1:0 = combine(r0, r1) }")
        .vec(v().exp(1, d(0)).exp(0, d(1))));
    c.push(Case::new("pair_write_reads_halves",
        "{ r1:0 = combine(r5, r6); r7 = r0; r8 = r1 }")
        .vec(v().exp(0, d(6)).exp(1, d(5)).exp(7, d(0)).exp(8, d(1))));
    c.push(Case::new("pair_write_add_halves",
        "{ r1:0 = combine(#1, #2); r9 = add(r0, r1) }")
        .vec(v().exp(0, 2).exp(1, 1).exp(9, d(0).wrapping_add(d(1)))));
    c.push(Case::new("half_write_pair_read",
        "{ r0 = #5; r3:2 = combine(r1, r0) }")
        .vec(v().exp(0, 5).exp(3, d(1)).exp(2, d(0))));
    c.push(Case::new("pair_copy_chain", "{ r3:2 = r1:0; r5:4 = r3:2 }")
        .vec(v().exp(2, d(0)).exp(3, d(1)).exp(4, d(2)).exp(5, d(3))));
    c.push(Case::new("pair_write_then_read_upper_half", "{ r1:0 = r3:2; r2 = r1 }")
        .vec(v().exp(0, d(2)).exp(1, d(3)).exp(2, d(1))));
    c.push(Case::new("pair_write_then_read_lower_half", "{ r3:2 = r1:0; r1 = r2 }")
        .vec(v().exp(2, d(0)).exp(3, d(1)).exp(1, d(2))));
    {
        let a = pair(0, 1);
        let b = pair(2, 3);
        let s = a.wrapping_add(b);
        let df = a.wrapping_sub(b);
        c.push(Case::new("pair_arith_cross",
            "{ r1:0 = add(r1:0, r3:2); r3:2 = sub(r1:0, r3:2) }")
            .vec(v().exp(0, lo(s)).exp(1, hi(s)).exp(2, lo(df)).exp(3, hi(df))));
    }
    c.push(Case::new("half_high_write_reads_old",
        "{ r0.h = #0x1234; r1 = r0 }")
        .vec(v().exp(0, (d(0) & 0xffff) | 0x1234_0000).exp(1, d(0))));
    c.push(Case::new("half_low_write_reads_old",
        "{ r0.l = #0x5678; r1 = add(r0, r2) }")
        .vec(v().exp(0, (d(0) & 0xffff_0000) | 0x5678).exp(1, d(0).wrapping_add(d(2)))));
    c.push(Case::new("extender_producer_reads_old",
        "{ r0 = ##0x12345678; r1 = r0 }")
        .vec(v().exp(0, 0x1234_5678).exp(1, d(0))));
    // Accumulating instructions read their own destination.
    c.push(Case::new("accumulate_reads_old_dest",
        "{ r0 += mpyi(r1, r2); r3 = r0 }")
        .vec(v().exp(0, d(0).wrapping_add(d(1).wrapping_mul(d(2)))).exp(3, d(0))));
    {
        let acc = pair(0, 1) as i64;
        let prod = (d(2) as i32 as i64).wrapping_mul(d(3) as i32 as i64);
        let r = acc.wrapping_add(prod) as u64;
        c.push(Case::new("pair_accumulate_reads_old_dest",
            "{ r1:0 += mpy(r2, r3); r5:4 = r1:0 }")
            .vec(v().exp(0, lo(r)).exp(1, hi(r)).exp(4, d(0)).exp(5, d(1))));
    }

    // ---- Loads/stores versus registers written in the same packet ----------
    let d0 = d(0);
    let d1 = d(1);
    c.push(Case::new("load_into_store_data",
        "{ r0 = memw(r20+#0); memw(r20+#4) = r0 }")
        .vec(v().set_win(20, p)
            .exp(0, dval(p as usize, 4) as u32)
            .exp_mem(p as usize + 4, &le32(d0))));
    c.push(Case::new("store_then_load_same_reg",
        "{ memw(r20+#0) = r0; r0 = memw(r20+#4) }")
        .vec(v().set_win(20, p)
            .exp(0, dval(p as usize + 4, 4) as u32)
            .exp_mem(p as usize, &le32(d0))));
    c.push(Case::new("load_base_updated_same_packet",
        "{ r20 = add(r20, #8); r0 = memw(r20+#0) }")
        .vec(v().set_win(20, p)
            .exp_win(20, p + 8)
            .exp(0, dval(p as usize, 4) as u32)));
    c.push(Case::new("store_base_updated_same_packet",
        "{ r20 = add(r20, #8); memw(r20+#0) = r1 }")
        .vec(v().set_win(20, p)
            .exp_win(20, p + 8)
            .exp_mem(p as usize, &le32(d1))));
    c.push(Case::new("postinc_load_reads_old_base",
        "{ r0 = memw(r20++#4); r1 = r20 }")
        .vec(v().set_win(20, p)
            .exp_win(20, p + 4)
            .exp(0, dval(p as usize, 4) as u32)
            .exp_win(1, p)));
    c.push(Case::new("postinc_store_reads_old_base",
        "{ memw(r20++#4) = r0; r1 = r20 }")
        .vec(v().set_win(20, p)
            .exp_win(20, p + 4)
            .exp_win(1, p)
            .exp_mem(p as usize, &le32(d0))));
    c.push(Case::new("postinc_load_other_access_uses_old_base",
        "{ r0 = memw(r20++#4); memw(r20+#8) = r1 }")
        .vec(v().set_win(20, p)
            .exp_win(20, p + 4)
            .exp(0, dval(p as usize, 4) as u32)
            .exp_mem(p as usize + 8, &le32(d1))));
    c.push(Case::new("load_pair_store_half",
        "{ r1:0 = memd(r20+#0); memw(r21+#0) = r1 }")
        .vec(v().set_win(20, p).set_win(21, p + 0x40)
            .exp(0, dval(p as usize, 4) as u32)
            .exp(1, dval(p as usize + 4, 4) as u32)
            .exp_mem(p as usize + 0x40, &le32(d1))));
    c.push(Case::new("store_pair_and_write_half",
        "{ memd(r20+#0) = r1:0; r0 = r1 }")
        .vec(v().set_win(20, p)
            .exp(0, d1)
            .exp_mem(p as usize, &le64(pair(0, 1)))));
    c.push(Case::new("load_dest_is_other_store_data",
        "{ r3 = memw(r20+#0); memw(r21+#0) = r3 }")
        .vec(v().set_win(20, p).set_win(21, p + 0x40)
            .exp(3, dval(p as usize, 4) as u32)
            .exp_mem(p as usize + 0x40, &le32(d(3)))));
    c.push(Case::new("load_into_own_base_other_store_old_base",
        "{ r20 = memw(r20+#0); memw(r20+#4) = r1 }")
        .vec(v().set_win(20, p)
            .exp(20, dval(p as usize, 4) as u32)
            .exp_mem(p as usize + 4, &le32(d1))));

    // ---- Predicate registers and C4 ----------------------------------------
    // P is 0 on input except where a case says otherwise.
    c.push(Case::new("pred_write_reads_old_pregs",
        "{ p0 = cmp.eq(r0, r0); r5 = p3:0 }")
        .vec(v().set(ST_P, 0x0000_0012)
            .exp(5, 0x0000_0012)
            .exp(ST_P, 0x0000_0012 | 0xff)));
    c.push(Case::new("pred_write_two_preds",
        "{ p0 = cmp.eq(r0, r0); p1 = cmp.gt(r1, r0) }")
        .vec(v().set(ST_P, 0)
            .set(0, 5).set(1, 9)
            .exp(ST_P, 0x0000_ffff)));
    c.push(Case::new("c4_write_predicated_op_uses_old",
        "{ p3:0 = r1; if (p0) r2 = #1 }")
        .vec(v().set(ST_P, 0x0000_00ff).set(1, 0)
            .exp(ST_P, 0).exp(2, 1)));
    c.push(Case::new("c4_write_predicated_op_uses_old_false",
        "{ p3:0 = r1; if (p0) r2 = #1 }")
        .vec(v().set(ST_P, 0).set(1, 0x0000_00ff)
            .exp(ST_P, 0x0000_00ff)));
    c.push(Case::new("pred_use_old_value",
        "{ p0 = cmp.gt(r0, r1); if (p0) r2 = #1; if (!p0) r3 = #2 }")
        .vec(v().set(ST_P, 0xff).set(0, 1).set(1, 5)
            // old p0 = true: r2 written, r3 not.  New p0 = (1 > 5) = false.
            .exp(2, 1).exp(ST_P, 0))
        .vec(v().set(ST_P, 0x00).set(0, 9).set(1, 5)
            // old p0 = false: r3 written.  New p0 = true.
            .exp(3, 2).exp(ST_P, 0xff)));

    // ---- Implicit link-register write by call -------------------------------
    // LR and r6 end up holding code addresses, so they are not checked
    // directly; r7 = LR - &label proves LR got the address after the call.
    c.push(Case::new("call_lr_write_reads_old",
        "{ call 1f; r2 = r31; r6 = ##1f }\n1:\n{ r7 = sub(r31, r6) }")
        .vec(v().exp(2, 0xC0DE_0123).ignore(6).ignore(ST_LR).exp(7, 0)));

    c
}
