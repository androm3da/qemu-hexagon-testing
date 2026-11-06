// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! HLX (element-wise wide SIMD coprocessor) instructions in packets.
//!
//! Rules (HLX chapter; coprocessor configuration chapter):
//!  * HLX memory is VTCM only and each access moves one whole 2048-byte
//!    register; the low 11 address bits are ignored.
//!  * An HLX load exists only as `.tmp` and an HLX store only as `.new`; the
//!    value flows from the load through ALU instructions of the SAME packet
//!    into the store, whatever their order in the packet.
//!  * HLX shares packets with scalar and HVX instructions, which behave as
//!    usual.
//!  * CCR.XE3 enables HLX.  A write to it takes effect after the packet
//!    that contains it: an HLX instruction in the same packet still sees the
//!    old value.  An HLX instruction with XE3 clear raises 0x19.

use super::*;

const REG_BYTES: usize = 2048;

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

fn vt_default(off: usize, len: usize) -> Vec<u8> {
    (off..off + len).map(def_vt_byte).collect()
}

fn halves(b: &[u8]) -> Vec<u16> {
    b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
}

fn from_halves(h: &[u16]) -> Vec<u8> {
    h.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn add_h(a: &[u8], b: &[u8]) -> Vec<u8> {
    from_halves(&halves(a).iter().zip(halves(b)).map(|(x, y)| x.wrapping_add(y)).collect::<Vec<_>>())
}

fn dv(n: usize) -> Vec<u8> {
    (0..VLEN).map(|i| def_vbyte(n, i)).collect()
}

fn vaddw(a: &[u8], b: &[u8]) -> Vec<u8> {
    a.chunks(4)
        .zip(b.chunks(4))
        .flat_map(|(x, y)| {
            u32::from_le_bytes([x[0], x[1], x[2], x[3]])
                .wrapping_add(u32::from_le_bytes([y[0], y[1], y[2], y[3]]))
                .to_le_bytes()
        })
        .collect()
}

/// Enable HLX in packets of their own (isync makes the next packet see it).
const ENABLE: &str = "{ r27 = ccr }\n{ r26 = setbit(r27, #28) }\n{ ccr = r26 }\n{ isync }";

/// A case that runs with HLX enabled; r1/r2 are clobbered by the prelude.
fn hlx_case(name: String, body: String) -> Case {
    Case::new(name, format!("{ENABLE}\n{body}")).vtcm().restore_enables()
}

const SRC: usize = 0x0000;
const DST: usize = 0x0800;

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    let src = vt_default(SRC, REG_BYTES);
    let doubled = add_h(&src, &src);

    // ---- tmp load -> ALU -> new store, every order ---------------------------
    let insns = ["L0.tmp = mem(r0)", "L1.h = add(L0.h, L0.h)", "mem(r1) = L1.new"];
    for (i, order) in perms(&insns).into_iter().enumerate() {
        out.push(hlx_case(format!("tmp_alu_new_o{i}"), format!("{{ {} }}", order.join("; ")))
            .vec(v().set_vt(0, SRC as u32).set_vt(1, DST as u32).ignore(26).ignore(27)
                .exp_vt_mem(DST, &doubled)));
    }
    // Low address bits are ignored by loads and stores.
    for (lo0, lo1) in [(0x37u32, 0u32), (0, 0x5), (0x7ff, 0x7ff)] {
        out.push(hlx_case(format!("address_low_bits_ignored_{lo0:x}_{lo1:x}"),
            "{ L0.tmp = mem(r0); L1.h = add(L0.h, L0.h); mem(r1) = L1.new }".to_string())
            .vec(v().set_vt(0, SRC as u32 + lo0).set_vt(1, DST as u32 + lo1).ignore(26).ignore(27)
                .exp_vt(0, SRC as u32 + lo0)
                .exp_vt_mem(DST, &doubled)));
    }
    // Source and destination the same register area: the load is ordered
    // before the store, so the stored data comes from the old contents.
    out.push(hlx_case("load_and_store_same_register_area".to_string(),
        "{ L0.tmp = mem(r0); L1.h = add(L0.h, L0.h); mem(r1) = L1.new }".to_string())
        .vec(v().set_vt(0, SRC as u32).set_vt(1, SRC as u32).ignore(26).ignore(27)
            .exp_vt_mem(SRC, &doubled)));
    // Scalar and HVX work in the same packet.
    for (i, order) in perms(&["L0.tmp = mem(r0)", "L1.h = add(L0.h, L0.h)", "mem(r1) = L1.new", "r4 = add(r4, #1)"])
        .into_iter().enumerate() {
        out.push(hlx_case(format!("tmp_alu_new_scalar_o{i}"), format!("{{ {} }}", order.join("; ")))
            .vec(v().set_vt(0, SRC as u32).set_vt(1, DST as u32).ignore(26).ignore(27)
                .exp(4, d(4).wrapping_add(1)).exp_vt_mem(DST, &doubled)));
    }
    for (i, order) in perms(&["L0.tmp = mem(r0)", "L1.h = add(L0.h, L0.h)", "mem(r1) = L1.new", "v2.w = vadd(v0.w, v1.w)"])
        .into_iter().enumerate() {
        out.push(hlx_case(format!("tmp_alu_new_hvx_o{i}"), format!("{{ {} }}", order.join("; "))).hvx()
            .vec(v().set_vt(0, SRC as u32).set_vt(1, DST as u32).ignore(26).ignore(27)
                .expv(2, &vaddw(&dv(0), &dv(1))).exp_vt_mem(DST, &doubled)));
    }

    // ---- CCR.XE3 ---------------------------------------------------------------
    // Enable HLX in a packet of its own (followed by isync so the next packet
    // is sure to see it), then use it.
    let enable = ENABLE;
    let hlx = "L0.tmp = mem(r0); L1.h = add(L0.h, L0.h); mem(r1) = L1.new";
    out.push(Case::new("xe3_clear_hlx_faults",
        format!("{{ {hlx} }}"))
        .vtcm().restore_enables().resume_at_end().elr_at(PROLOGUE)
        .vec(v().set_vt(0, SRC as u32).set_vt(1, DST as u32)
            .ev(Ev::new(EvKind::Err, 0x19))));
    out.push(hlx_case("xe3_enabled_hlx_runs".to_string(), format!("{{ {hlx} }}"))
        .vec(v().set_vt(0, SRC as u32).set_vt(1, DST as u32)
            .ignore(26).ignore(27).exp_vt_mem(DST, &doubled)));
    // XE3 set in the packet of the HLX instruction: still illegal; nothing in
    // the packet commits.
    out.push(Case::new("xe3_set_in_hlx_packet_still_faults",
        format!("{{ r27 = ccr }}\n{{ r26 = setbit(r27, #28) }}\n{{ ccr = r26; {hlx} }}"))
        .vtcm().restore_enables().resume_at_end().elr_at(PROLOGUE + 4 + 4)
        .vec(v().set_vt(0, SRC as u32).set_vt(1, DST as u32).ignore(26).ignore(27)
            .ev(Ev::new(EvKind::Err, 0x19))));
    // XE3 cleared in the HLX packet: the HLX instruction runs, the next one
    // faults.
    out.push(Case::new("xe3_clear_in_hlx_packet_runs_next_faults",
        format!("{enable}\n{{ r26 = clrbit(r26, #28) }}\n{{ ccr = r26; {hlx} }}\n{{ {hlx} }}"))
        .vtcm().restore_enables().resume_at_end().elr_at(PROLOGUE + 16 + 4 + 16)
        // Per the spec the store of the first packet is performed (old XE3 = 1).
        // hexagon-sim 21.0.04 drops it silently, so the destination is not
        // asserted; the old-enable behaviour is checked through the fault
        // being taken only at the second HLX packet.
        .vec(v().set_vt(0, SRC as u32).set_vt(1, DST as u32).ignore(26).ignore(27)
            .ignore_vt(DST, REG_BYTES)
            .exp_vt_mem(DST, if strict() { &doubled } else { &[][..] })
            .ev(Ev::new(EvKind::Err, 0x19))));
    // ---- A CCR write in the HLX packet that leaves XE3 alone ------------------
    // (the packet's HLX store must happen whatever else the CCR write does)
    out.push(Case::new("ccr_write_keeps_xe3_with_hlx_packet",
        format!("{enable}\n{{ ccr = r26; {hlx} }}"))
        .vtcm().restore_enables()
        .vec(v().set_vt(0, SRC as u32).set_vt(1, DST as u32).ignore(26).ignore(27)
            .exp_vt_mem(DST, &doubled)));
    // SSR write (XE untouched) in the same packet as the HLX instructions.
    out.push(Case::new("ssr_write_with_hlx_packet",
        format!("{enable}\n{{ r24 = ssr }}\n{{ ssr = r24; {hlx} }}"))
        .vtcm().restore_enables()
        .vec(v().set_vt(0, SRC as u32).set_vt(1, DST as u32).ignore(24).ignore(26).ignore(27)
            .exp_vt_mem(DST, &doubled)));
    // Clearing XE3 in a packet with only the HLX load and ALU op (no store).
    out.push(Case::new("xe3_clear_in_hlx_packet_no_store_runs",
        format!("{enable}\n{{ r26 = clrbit(r26, #28) }}\n{{ ccr = r26; L0.tmp = mem(r0); L1.h = add(L0.h, L0.h) }}\n{{ L2.tmp = mem(r0); L3.h = add(L2.h, L2.h); mem(r1) = L3.new }}"))
        .vtcm().restore_enables().resume_at_end().elr_at(PROLOGUE + 16 + 4 + 12)
        .vec(v().set_vt(0, SRC as u32).set_vt(1, DST as u32).ignore(26).ignore(27)
            .ignore_vt(DST, REG_BYTES)
            .ev(Ev::new(EvKind::Err, 0x19))));
    out
}
