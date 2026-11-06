// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! HVX instructions inside packets: vector register and pair read/write
//! ordering, `.cur`/`.tmp`/`.new`, predicate (Q) registers, predicated vector
//! memory operations, and mixing with scalar work.
//!
//! Rules (HVX register-file model): like scalar registers, every source is
//! read before any result of the packet is written, so a vector pair written
//! in the packet is still seen as its old halves by other instructions, and a
//! Q register written by a compare is seen with its old value by a `vmux` or
//! predicated store of the same packet.  A `.cur` load also writes its
//! register and is forwarded to the packet's consumers; a `.tmp` load is
//! forwarded but never written to the register file; a `.new` vector store
//! stores the producer's result.  A predicated-off vector access has no
//! effect.  Vector memory accesses ignore the low address bits.

use super::*;

type V = Vec<u8>;

pub fn dv(n: usize) -> V {
    (0..VLEN).map(|i| def_vbyte(n, i)).collect()
}

pub fn dq(n: usize) -> Vec<bool> {
    (0..VLEN).map(|i| def_qbyte(n, i) != 0).collect()
}

fn win_v(off: usize) -> V {
    dmem(off, VLEN)
}

fn lanes32(x: &V) -> Vec<u32> {
    x.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

pub fn from32(l: &[u32]) -> V {
    l.iter().flat_map(|w| w.to_le_bytes()).collect()
}

pub fn vaddw(a: &V, b: &V) -> V {
    from32(&lanes32(a).iter().zip(lanes32(b)).map(|(x, y)| x.wrapping_add(y)).collect::<Vec<_>>())
}

fn vsubw(a: &V, b: &V) -> V {
    from32(&lanes32(a).iter().zip(lanes32(b)).map(|(x, y)| x.wrapping_sub(y)).collect::<Vec<_>>())
}

fn vaddb(a: &V, b: &V) -> V {
    a.iter().zip(b).map(|(x, y)| x.wrapping_add(*y)).collect()
}

fn vand(a: &V, b: &V) -> V {
    a.iter().zip(b).map(|(x, y)| x & y).collect()
}

fn vsplatw(x: u32) -> V {
    from32(&vec![x; VLEN / 4])
}

fn vcmp_eq_w(a: &V, b: &V) -> Vec<bool> {
    // Q holds one bit per byte; a word compare sets 4 consecutive bits.
    lanes32(a).iter().zip(lanes32(b)).flat_map(|(x, y)| [*x == y; 4]).collect()
}

fn vcmp_gt_w(a: &V, b: &V) -> Vec<bool> {
    lanes32(a).iter().zip(lanes32(b)).flat_map(|(x, y)| [(*x as i32) > (y as i32); 4]).collect()
}

fn vmux(q: &[bool], a: &V, b: &V) -> V {
    (0..VLEN).map(|i| if q[i] { a[i] } else { b[i] }).collect()
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

fn v() -> Vector {
    Vector::new()
}

fn packet(order: &[&str]) -> String {
    format!("{{ {} }}", order.join("; "))
}

/// Add `perms` of `insns` as cases named `name_oN`, all with `vec`.
fn all_orders(out: &mut Vec<Case>, name: &str, insns: &[&str], tail: &str, vec: Vector) {
    for (i, order) in perms(insns).into_iter().enumerate() {
        let body = if tail.is_empty() {
            packet(&order)
        } else {
            format!("{}\n{}", packet(&order), tail)
        };
        out.push(Case::new(format!("{name}_o{i}"), body).hvx().vec(vec.clone()));
    }
}

const WB: usize = BASE as usize; // 0x80: a 128-byte aligned window area
const WB2: usize = 0x100;

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();

    // ---- Register read-before-write ----------------------------------------
    all_orders(&mut out, "swap", &["v0 = v1", "v1 = v0"], "",
        v().expv(0, &dv(1)).expv(1, &dv(0)));
    all_orders(&mut out, "pair_swap", &["v1:0 = v3:2", "v3:2 = v1:0"], "",
        v().expv(0, &dv(2)).expv(1, &dv(3)).expv(2, &dv(0)).expv(3, &dv(1)));
    all_orders(&mut out, "pair_write_halves_read",
        &["v1:0 = vcombine(v3, v2)", "v4 = v0", "v5 = v1"], "",
        v().expv(0, &dv(2)).expv(1, &dv(3)).expv(4, &dv(0)).expv(5, &dv(1)));
    all_orders(&mut out, "alu_chain_reads_old",
        &["v2.w = vadd(v0.w, v1.w)", "v3 = v2", "v4.w = vsub(v2.w, v0.w)"], "",
        v().expv(2, &vaddw(&dv(0), &dv(1))).expv(3, &dv(2)).expv(4, &vsubw(&dv(2), &dv(0))));
    all_orders(&mut out, "pair_write_single_read_add",
        &["v1:0 = vcombine(v5, v4)", "v6.w = vadd(v0.w, v1.w)"], "",
        v().expv(0, &dv(4)).expv(1, &dv(5)).expv(6, &vaddw(&dv(0), &dv(1))));
    // Scalar work in the same packet.
    for (i, order) in perms(&["r6 = add(r0, r1)", "v1 = vsplat(r6)", "v2.w = vadd(v0.w, v1.w)"]).into_iter().enumerate() {
        out.push(Case::new(format!("scalar_producer_vsplat_reads_old_o{i}"), packet(&order.iter().map(|s| *s).collect::<Vec<_>>()))
            .hvx().vec(v().exp(6, d(0).wrapping_add(d(1))).expv(1, &vsplatw(d(6)))
                .expv(2, &vaddw(&dv(0), &dv(1)))));
    }

    // ---- Loads: .cur, .tmp, plain; stores: .new ------------------------------
    let m0 = win_v(WB);
    all_orders(&mut out, "cur_load_consumer",
        &["v1.cur = vmem(r20+#0)", "v2.w = vadd(v1.w, v3.w)"], "",
        v().set_win(20, BASE).expv(1, &m0).expv(2, &vaddw(&m0, &dv(3))));
    all_orders(&mut out, "tmp_load_consumer",
        &["v1.tmp = vmem(r20+#0)", "v2.w = vadd(v1.w, v3.w)"], "",
        v().set_win(20, BASE).expv(2, &vaddw(&m0, &dv(3))));
    all_orders(&mut out, "plain_load_reads_old_reg",
        &["v1 = vmem(r20+#0)", "v2.w = vadd(v1.w, v3.w)"], "",
        v().set_win(20, BASE).expv(1, &m0).expv(2, &vaddw(&dv(1), &dv(3))));
    all_orders(&mut out, "new_store",
        &["v1.w = vadd(v2.w, v3.w)", "vmem(r20+#0) = v1.new"], "",
        v().set_win(20, BASE).expv(1, &vaddw(&dv(2), &dv(3)))
            .exp_mem(WB, &vaddw(&dv(2), &dv(3))));
    all_orders(&mut out, "cur_load_alu_new_store",
        &["v1.cur = vmem(r20+#0)", "v2.w = vadd(v1.w, v3.w)", "vmem(r21+#0) = v2.new"], "",
        v().set_win(20, BASE).set_win(21, WB2 as u32)
            .expv(1, &m0).expv(2, &vaddw(&m0, &dv(3)))
            .exp_mem(WB2, &vaddw(&m0, &dv(3))));
    all_orders(&mut out, "tmp_load_alu_new_store",
        &["v1.tmp = vmem(r20+#0)", "v2.w = vadd(v1.w, v3.w)", "vmem(r21+#0) = v2.new"], "",
        v().set_win(20, BASE).set_win(21, WB2 as u32)
            .expv(2, &vaddw(&m0, &dv(3)))
            .exp_mem(WB2, &vaddw(&m0, &dv(3))));
    all_orders(&mut out, "load_store_distinct_with_alu",
        &["v1 = vmem(r20+#0)", "vmem(r21+#0) = v2", "v3.w = vadd(v4.w, v5.w)"], "",
        v().set_win(20, BASE).set_win(21, WB2 as u32)
            .expv(1, &m0).expv(3, &vaddw(&dv(4), &dv(5))).exp_mem(WB2, &dv(2)));
    // Load and store of the same vector in one packet.
    all_orders(&mut out, "load_store_same_address",
        &["v1 = vmem(r20+#0)", "vmem(r20+#0) = v2"], "",
        v().set_win(20, BASE).expv(1, &m0).exp_mem(WB, &dv(2)));
    // Post-increment.
    all_orders(&mut out, "load_postinc_reads_old_base",
        &["v1 = vmem(r20++#1)", "r2 = r20", "v3.w = vadd(v4.w, v5.w)"], "",
        v().set_win(20, BASE).expv(1, &m0).exp_win(20, BASE + 128).exp_win(2, BASE)
            .expv(3, &vaddw(&dv(4), &dv(5))));
    all_orders(&mut out, "store_postinc",
        &["vmem(r20++#1) = v1", "r2 = r20"], "",
        v().set_win(20, BASE).exp_win(20, BASE + 128).exp_win(2, BASE).exp_mem(WB, &dv(1)));

    // ---- Predicated vector memory ---------------------------------------------
    for t in [true, false] {
        let pin = if t { 0xff } else { 0 };
        for (name, ins, loads) in [
            ("p_load", "if (p0) v1 = vmem(r20+#0)", true),
            ("np_load", "if (!p0) v1 = vmem(r20+#0)", true),
            ("p_store", "if (p0) vmem(r20+#0) = v2", false),
            ("np_store", "if (!p0) vmem(r20+#0) = v2", false),
            ("p_load_postinc", "if (p0) v1 = vmem(r20++#1)", true),
            ("p_store_postinc", "if (p0) vmem(r20++#1) = v2", false),
        ] {
            let neg = name.starts_with("np");
            let exec = t != neg;
            let mut vv = v().set_win(20, BASE).set(ST_P, pin);
            if exec {
                vv = if loads { vv.expv(1, &m0) } else { vv.exp_mem(WB, &dv(2)) };
            }
            if name.ends_with("postinc") && exec {
                vv = vv.exp_win(20, BASE + 128);
            }
            out.push(Case::new(format!("{name}_{}", if t { "T" } else { "F" }),
                format!("{{ {ins}; r4 = add(r4, #1) }}")).hvx().vec(vv.exp(4, d(4).wrapping_add(1))));
        }
        // A compare in the same packet rewrites p0; HVX memory ops (which have
        // no `.new` predicate form) must use the old value.
        let mut vv = v().set_win(20, BASE).set(ST_P, pin).set(5, 1).set(6, if t { 2 } else { 1 });
        vv = vv.exp(ST_P, if t { 0 } else { 0xff });
        if t {
            vv = vv.exp_mem(WB, &dv(2));
        }
        out.push(Case::new(format!("pold_store_with_compare_{}", if t { "T" } else { "F" }),
            "{ p0 = cmp.eq(r5, r6); if (p0) vmem(r20+#0) = v2 }").hvx().vec(vv));
        // Predicated new-value store.
        let mut vv = v().set_win(20, BASE).set(ST_P, pin).expv(1, &vaddw(&dv(2), &dv(3)));
        if t {
            vv = vv.exp_mem(WB, &vaddw(&dv(2), &dv(3)));
        }
        out.push(Case::new(format!("p_new_store_{}", if t { "T" } else { "F" }),
            "{ v1.w = vadd(v2.w, v3.w); if (p0) vmem(r20+#0) = v1.new }").hvx().vec(vv));
    }
    // Q-predicated (byte-masked) stores: use the old Q.
    for neg in [false, true] {
        let q = dq(0);
        let qq: Vec<bool> = q.iter().map(|b| *b != neg).collect();
        let mut mem = m0.clone();
        for i in 0..VLEN {
            if qq[i] {
                mem[i] = dv(2)[i];
            }
        }
        let ins = if neg { "if (!q0) vmem(r20+#0) = v2" } else { "if (q0) vmem(r20+#0) = v2" };
        out.push(Case::new(format!("q_store_{}", if neg { "neg" } else { "pos" }),
            format!("{{ {ins} }}")).hvx().vec(v().set_win(20, BASE).exp_mem(WB, &mem)));
        // A compare in the same packet writes q0 but the store sees the old one.
        let newq = vcmp_gt_w(&dv(5), &dv(6));
        out.push(Case::new(format!("q_store_{}_old_q_with_compare", if neg { "neg" } else { "pos" }),
            format!("{{ q0 = vcmp.gt(v5.w, v6.w); {ins} }}")).hvx()
            .vec(v().set_win(20, BASE).exp_mem(WB, &mem).expq(0, &newq)));
    }

    // ---- Q registers --------------------------------------------------------------
    let newq = vcmp_eq_w(&dv(1), &dv(1));
    all_orders(&mut out, "q_write_vmux_reads_old",
        &["q0 = vcmp.eq(v1.w, v1.w)", "v2 = vmux(q0, v3, v4)"], "",
        v().expq(0, &newq).expv(2, &vmux(&dq(0), &dv(3), &dv(4))));
    let g = vcmp_gt_w(&dv(5), &dv(6));
    all_orders(&mut out, "q_two_writes_two_reads",
        &["q0 = vcmp.gt(v5.w, v6.w)", "q1 = vcmp.eq(v7.w, v7.w)", "v2 = vmux(q0, v3, v4)", "v8 = vmux(q1, v3, v4)"], "",
        v().expq(0, &g).expq(1, &vcmp_eq_w(&dv(7), &dv(7)))
            .expv(2, &vmux(&dq(0), &dv(3), &dv(4))).expv(8, &vmux(&dq(1), &dv(3), &dv(4))));
    let notq: Vec<bool> = dq(1).iter().map(|b| !b).collect();
    all_orders(&mut out, "q_not_and_reads_old",
        &["q0 = not(q1)", "q2 = and(q0, q1)"], "",
        v().expq(0, &notq).expq(2, &dq(0).iter().zip(dq(1)).map(|(a, b)| *a && b).collect::<Vec<_>>()));
    let _ = (vand(&dv(0), &dv(1)), vaddb(&dv(0), &dv(1)));

    gather_scatter_cases(&mut out);

    // ---- Ordering of a load and a store to the same vector ------------------
    for noshuf in [false, true] {
        for load_first in [false, true] {
            let ld = "v1 = vmem(r20+#0)";
            let st = "vmem(r20+#0) = v2";
            let body = if load_first { format!("{{ {ld}; {st} }}") } else { format!("{{ {st}; {ld} }}") };
            let body = if noshuf { format!("{body}:mem_noshuf") } else { body };
            // An aligned vector store can only be in slot 0, so the load is in
            // slot 1 and runs first whatever the source order: it returns the
            // old memory, and the store lands afterwards.
            let vv = v().set_win(20, BASE).exp_mem(WB, &dv(2)).expv(1, &m0);
            out.push(Case::new(
                format!("ld_st_same_{}_{}", if noshuf { "noshuf" } else { "shuf" }, if load_first { "ldfirst" } else { "stfirst" }),
                body,
            ).hvx().vec(vv));
        }
    }

    // ---- Low address bits of VMEM are ignored; VMEMU honours them ----------
    for lowbits in [1u32, 37, 127] {
        out.push(Case::new(format!("vmem_load_ignores_low_bits_{lowbits}"),
            "{ v1 = vmem(r20+#0); r4 = add(r4, #1) }").hvx()
            .vec(v().set_win(20, BASE + lowbits).expv(1, &m0).exp(4, d(4).wrapping_add(1))
                .exp_win(20, BASE + lowbits)));
        out.push(Case::new(format!("vmem_store_ignores_low_bits_{lowbits}"),
            "{ vmem(r20+#0) = v2; r4 = add(r4, #1) }").hvx()
            .vec(v().set_win(20, BASE + lowbits).exp_mem(WB, &dv(2)).exp(4, d(4).wrapping_add(1))
                .exp_win(20, BASE + lowbits)));
    }
    for off in [0usize, 1, 2, 31, 64, 127] {
        // An unaligned vector spanning two aligned ones (WB .. WB+256 are
        // all inside the window).
        let mut un = Vec::new();
        for i in 0..VLEN {
            un.push(def_mem_byte(WB + off + i));
        }
        out.push(Case::new(format!("vmemu_load_off{off}"),
            "{ v1 = vmemu(r20+#0); r4 = add(r4, #1) }").hvx()
            .vec(v().set_win(20, BASE + off as u32).expv(1, &un).exp(4, d(4).wrapping_add(1))
                .exp_win(20, BASE + off as u32)));
        out.push(Case::new(format!("vmemu_load_with_alu_off{off}"),
            "{ v1 = vmemu(r20+#0); v3.w = vadd(v4.w, v5.w); r4 = add(r4, #1) }").hvx()
            .vec(v().set_win(20, BASE + off as u32).expv(1, &un).expv(3, &vaddw(&dv(4), &dv(5)))
                .exp(4, d(4).wrapping_add(1)).exp_win(20, BASE + off as u32)));
        let mut mem: Vec<u8> = (0..WIN_BYTES).map(def_mem_byte).collect();
        mem[WB + off..WB + off + VLEN].copy_from_slice(&dv(2));
        out.push(Case::new(format!("vmemu_store_off{off}"),
            "{ vmemu(r20+#0) = v2; r4 = add(r4, #1) }").hvx()
            .vec(v().set_win(20, BASE + off as u32).exp_window(&mem).exp(4, d(4).wrapping_add(1))
                .exp_win(20, BASE + off as u32)));
        out.push(Case::new(format!("vmemu_store_postinc_off{off}"),
            "{ vmemu(r20++#1) = v2; r4 = r20 }").hvx()
            .vec(v().set_win(20, BASE + off as u32).exp_window(&mem)
                .exp_win(20, BASE + off as u32 + 128).exp_win(4, BASE + off as u32)));
    }
    out
}

/// Gather / scatter against the VTCM.
///
/// Rules (HVX gather/scatter): the offsets are byte offsets into the region
/// [Rt, Rt + Mu + 1); a gather writes the internal temporary register that a
/// `.new` store of the SAME packet stores; a scatter writes each lane of the
/// source vector to the lane's offset; `+=` accumulates.
fn gather_scatter_cases(out: &mut Vec<Case>) {
    let region = 0x800usize; // region size in bytes
    let vt = |off: usize, len: usize| -> Vec<u8> { (off..off + len).map(def_vt_byte).collect() };
    // Word offsets: a permutation of the first 32 words of the region.
    let offs: Vec<u32> = (0..32u32).map(|i| ((i * 7 + 3) % 32) * 4 + 0x100).collect();
    let offv = from32(&offs);
    let mm = (region - 1) as u32;
    let setup = "{ m0 = r4 }";

    // Gather + `.new` store, each source order.
    let mut gathered = Vec::new();
    for o in &offs {
        gathered.extend_from_slice(&vt(*o as usize, 4));
    }
    for (i, order) in perms(&["vtmp.w = vgather(r2, m0, v1.w).w", "vmem(r3+#0) = vtmp.new"])
        .into_iter().enumerate() {
        out.push(Case::new(format!("gather_new_store_o{i}"),
            format!("{setup}\n{}", packet(&order.iter().map(|s| *s).collect::<Vec<_>>())))
            .vtcm().hvx()
            .vec(v().exp(ST_M0, mm).set_vt(2, 0).set(4, mm).set_vt(3, 0x1800).setv(1, &offv)
                .exp_vt_mem(0x1800, &gathered)));
    }
    // With a scalar op and an ALU op in the packet.
    out.push(Case::new("gather_new_store_with_alu_and_scalar",
        format!("{setup}\n{{ vtmp.w = vgather(r2, m0, v1.w).w; vmem(r3+#0) = vtmp.new; v5.w = vadd(v6.w, v7.w); r5 = add(r5, #1) }}"))
        .vtcm().hvx()
        .vec(v().exp(ST_M0, mm).set_vt(2, 0).set(4, mm).set_vt(3, 0x1800).setv(1, &offv)
            .exp_vt_mem(0x1800, &gathered)
            .expv(5, &vaddw(&dv(6), &dv(7))).exp(5, d(5).wrapping_add(1))));

    // Scatter: each lane of v2 goes to its offset (offsets are distinct).
    let mut scattered = vt(0, 0x2000);
    for (i, o) in offs.iter().enumerate() {
        scattered[*o as usize..*o as usize + 4].copy_from_slice(&dv(2)[i * 4..i * 4 + 4]);
    }
    out.push(Case::new("scatter",
        format!("{setup}\n{{ vscatter(r2, m0, v1.w).w = v2 }}"))
        .vtcm().hvx()
        .vec(v().exp(ST_M0, mm).set_vt(2, 0).set(4, mm).setv(1, &offv)
            .exp_vt_mem(0, &scattered)));
    // Scatter-accumulate.
    let mut acc = vt(0, 0x2000);
    for (i, o) in offs.iter().enumerate() {
        let old = u32::from_le_bytes([acc[*o as usize], acc[*o as usize + 1], acc[*o as usize + 2], acc[*o as usize + 3]]);
        let add = u32::from_le_bytes([dv(2)[i * 4], dv(2)[i * 4 + 1], dv(2)[i * 4 + 2], dv(2)[i * 4 + 3]]);
        acc[*o as usize..*o as usize + 4].copy_from_slice(&old.wrapping_add(add).to_le_bytes());
    }
    out.push(Case::new("scatter_accumulate",
        format!("{setup}\n{{ vscatter(r2, m0, v1.w).w += v2 }}"))
        .vtcm().hvx()
        .vec(v().exp(ST_M0, mm).set_vt(2, 0).set(4, mm).setv(1, &offv)
            .exp_vt_mem(0, &acc)));
}
