// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! The same packet written in every source order.
//!
//! The assembler keeps the order it is given wherever slot rules allow, so
//! each permutation is a different encoding of the same semantics (a `.new`
//! consumer before its producer, a store before the load it must not
//! affect, jumps at either end).  A packet's meaning is independent of its
//! encoding order, with one deliberate exception: when two stores overlap,
//! the one that is later in the packet (slot 0) wins.  That exception is
//! modelled explicitly.

use super::*;

const WB: usize = BASE as usize;

fn v() -> Vector {
    Vector::new()
}

fn perms<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut out = Vec::new();
    for i in 0..items.len() {
        let mut rest = items.to_vec();
        let x = rest.remove(i);
        for mut p in perms(&rest) {
            p.insert(0, x.clone());
            out.push(p);
        }
    }
    out
}

fn packet(order: &[&str]) -> String {
    format!("{{ {} }}", order.join("; "))
}

/// Order-independent templates: all permutations must give `vecs`.
fn independent(out: &mut Vec<Case>, name: &str, insns: &[&str], tail: &str, vecs: Vec<Vector>) {
    for (i, order) in perms(insns).into_iter().enumerate() {
        let body = if tail.is_empty() {
            packet(&order)
        } else {
            format!("{}\n{}", packet(&order), tail)
        };
        let mut c = Case::new(format!("{name}_o{i}"), body);
        for vec in &vecs {
            c = c.vec(vec.clone());
        }
        out.push(c);
    }
}

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    let (d0, d1) = (d(0), d(1));

    // 1. Register swap plus unrelated work.
    independent(&mut out, "swap_alu_load",
        &["r0 = r1", "r1 = r0", "r2 = add(r2, #1)", "r3 = memw(r20+#0)"], "",
        vec![v().set_win(20, BASE)
            .exp(0, d1).exp(1, d0).exp(2, d(2).wrapping_add(1)).exp(3, dval(WB, 4) as u32)]);

    // 2. Store, independent load, dependent ALU, compare.
    independent(&mut out, "st_ld_alu_cmp",
        &["memw(r20+#0) = r0", "r2 = memw(r21+#0)", "r3 = add(r2, r1)", "p0 = cmp.eq(r4, r5)"], "",
        vec![v().set_win(20, BASE).set_win(21, BASE + 0x40).set(4, 1).set(5, 1).set(ST_P, 0)
            .exp_mem(WB, &le32(d0))
            .exp(2, dval(WB + 0x40, 4) as u32)
            .exp(3, d(2).wrapping_add(d1))
            .exp(ST_P, 0xff)]);

    // 3. `.new` predicate consumers and an old-predicate consumer.
    for (p_old, eq) in [(0u32, true), (0xff, true), (0, false), (0xff, false)] {
        let r1 = if eq { 7 } else { 8 };
        let mut vv = v().set(ST_P, p_old | 0x5a00).set(0, 7).set(1, r1);
        vv = vv.exp(ST_P, 0x5a00 | if eq { 0xff } else { 0 });
        vv = if eq { vv.exp(2, 1) } else { vv.exp(3, 2) };
        if p_old != 0 {
            vv = vv.exp(4, 3);
        }
        independent(&mut out, &format!("pnew_consumers_pold{}_eq{}", p_old & 1, eq as u32),
            &["p0 = cmp.eq(r0, r1)", "if (p0.new) r2 = #1", "if (!p0.new) r3 = #2", "if (p0) r4 = #3"], "",
            vec![vv]);
    }

    // 4. New-value store with its producer, a load and an ALU op.
    independent(&mut out, "nvstore_load_alu",
        &["r2 = add(r0, r1)", "memw(r20+#0) = r2.new", "r3 = memw(r21+#0)", "r4 = sub(r1, r0)"], "",
        vec![v().set_win(20, BASE).set_win(21, BASE + 0x40)
            .exp(2, d0.wrapping_add(d1)).exp_mem(WB, &le32(d0.wrapping_add(d1)))
            .exp(3, dval(WB + 0x40, 4) as u32).exp(4, d1.wrapping_sub(d0))]);
    independent(&mut out, "nvstore_postinc_load_producer",
        &["r2 = memw(r21++#4)", "memw(r20++#4) = r2.new", "r3 = add(r0, r1)"], "",
        vec![v().set_win(20, BASE).set_win(21, BASE + 0x40)
            .exp(2, dval(WB + 0x40, 4) as u32).exp_mem(WB, &le32(dval(WB + 0x40, 4) as u32))
            .exp_win(20, BASE + 4).exp_win(21, BASE + 0x44).exp(3, d0.wrapping_add(d1))]);

    // 5. Post-increment loads/stores reading the old base.
    independent(&mut out, "postinc_readers",
        &["r0 = memw(r20++#4)", "memw(r21++#4) = r1", "r2 = r20", "r3 = r21"], "",
        vec![v().set_win(20, BASE).set_win(21, BASE + 0x40)
            .exp(0, dval(WB, 4) as u32).exp_mem(WB + 0x40, &le32(d1))
            .exp_win(20, BASE + 4).exp_win(21, BASE + 0x44)
            .exp_win(2, BASE).exp_win(3, BASE + 0x40)]);

    // 6. Pair write read through its halves.
    independent(&mut out, "pair_halves",
        &["r1:0 = combine(r5, r4)", "r6 = r0", "r7 = r1", "r8 = add(r1, r0)"], "",
        vec![v().exp(0, d(4)).exp(1, d(5)).exp(6, d0).exp(7, d1).exp(8, d0.wrapping_add(d1))]);

    // 7. Predicated memory ops with a compare producer.
    for t in [true, false] {
        let mut vv = v().set_win(20, BASE).set_win(21, BASE + 0x40).set(8, 5).set(9, if t { 5 } else { 6 })
            .set(ST_P, 0);
        vv = vv.exp(ST_P, if t { 0xff } else { 0 });
        vv = if t {
            vv.exp_mem(WB, &le32(d0))
        } else {
            vv.exp(2, dval(WB + 0x40, 4) as u32)
        };
        independent(&mut out, &format!("pred_mem_{}", if t { "T" } else { "F" }),
            &["p0 = cmp.eq(r8, r9)", "if (p0.new) memw(r20+#0) = r0", "if (!p0.new) r2 = memw(r21+#0)", "r3 = r4"], "",
            vec![vv.exp(3, d(4))]);
    }

    // 8. Compound jump with unrelated work, taken and not taken.
    for t in [true, false] {
        let mut vv = v().set(0, 3).set(1, if t { 3 } else { 4 }).set_win(20, BASE)
            .exp(ST_P, (def_state(ST_P) & !0xff) | if t { 0xff } else { 0 })
            .exp(2, 1).exp_mem(WB, &le32(d(3)));
        if !t {
            vv = vv.exp(10, 1);
        }
        independent(&mut out, &format!("jump_pnew_{}", if t { "taken" } else { "nottaken" }),
            &["p0 = cmp.eq(r0, r1)", "if (p0.new) jump:t 1f", "r2 = #1", "memw(r20+#0) = r3"],
            "{ r10 = #1 }\n1:", vec![vv]);
    }

    // 9. Overlapping stores: the later one in the packet is in slot 0 and
    // lands last.  Everything else is independent of order.
    for (a_sz, b_sz, b_off) in [(4usize, 4usize, 0i32), (4, 1, 2), (1, 4, 0), (2, 4, 0), (4, 2, 2)] {
        let mn = |n: usize| match n { 1 => "memb", 2 => "memh", _ => "memw" };
        let a_ins = format!("{}(r20+#0) = r0", mn(a_sz));
        let b_ins = format!("{}(r21+#0) = r1", mn(b_sz));
        let others = ["r4 = add(r4, #1)", "r5 = add(r5, #2)"];
        let items: Vec<String> = vec![a_ins.clone(), b_ins.clone(), others[0].into(), others[1].into()];
        for (i, order) in perms(&items).into_iter().enumerate() {
            let pos_a = order.iter().position(|s| *s == a_ins).unwrap();
            let pos_b = order.iter().position(|s| *s == b_ins).unwrap();
            let mut mem: Vec<u8> = (0..WIN_BYTES).map(def_mem_byte).collect();
            let wr = |m: &mut Vec<u8>, off: usize, sz: usize, val: u32| {
                for k in 0..sz {
                    m[off + k] = (val >> (8 * k)) as u8;
                }
            };
            let b_addr = (WB as i32 + b_off) as usize;
            // Earlier in source = slot 1 = executes first.
            if pos_a < pos_b {
                wr(&mut mem, WB, a_sz, d0);
                wr(&mut mem, b_addr, b_sz, d1);
            } else {
                wr(&mut mem, b_addr, b_sz, d1);
                wr(&mut mem, WB, a_sz, d0);
            }
            let refs: Vec<&str> = order.iter().map(|s| s.as_str()).collect();
            let vv = v().set_win(20, BASE).set_win(21, (BASE as i32 + b_off) as u32)
                .exp(4, d(4).wrapping_add(1)).exp(5, d(5).wrapping_add(2)).exp_window(&mem);
            out.push(Case::new(format!("dual_store_a{a_sz}_b{b_sz}_@{b_off:+}_o{i}"), packet(&refs)).vec(vv));
        }
    }
    out
}
