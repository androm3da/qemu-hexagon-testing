// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! USR side effects of instructions inside a packet.
//!
//! Rules (reference-simulator observed):
//!  * USR.OVF is sticky: any saturating instruction that actually saturates
//!    sets it; nothing but an explicit USR write clears it.  Several
//!    saturating instructions in a packet OR their overflows.
//!  * Reading USR in the packet sees the value from before the packet, so a
//!    same-packet saturation is not yet visible.
//!  * Floating-point exception flags (USR[5:1]) are sticky and OR across all
//!    FP instructions of a packet; the rounding mode is USR.FPRND.
//!  * An instruction that does not saturate/raise leaves USR as it was.

use super::*;

const OVF: u32 = 1;
const FPINV: u32 = 1 << 1;
const FPOVF: u32 = 1 << 3;
const FPUNF: u32 = 1 << 4;
const FPINX: u32 = 1 << 5;

fn v() -> Vector {
    Vector::new()
}

fn sat32(x: i64) -> (u32, bool) {
    if x > i32::MAX as i64 {
        (i32::MAX as u32, true)
    } else if x < i32::MIN as i64 {
        (i32::MIN as u32, true)
    } else {
        (x as i32 as u32, false)
    }
}

/// A saturating instruction with its model: (result register values, overflowed).
struct Sat {
    name: &'static str,
    text: &'static str,
    /// Dest register(s): (reg, value).
    model: fn(&[u32; 6]) -> (Vec<(usize, u32)>, bool),
    /// Input vectors (r0..r5).
    ins: Vec<[u32; 6]>,
}

fn lanes_h(x: u64) -> [i16; 4] {
    [x as i16, (x >> 16) as i16, (x >> 32) as i16, (x >> 48) as i16]
}

fn sats() -> Vec<Sat> {
    let edge: Vec<[u32; 6]> = vec![
        [1, 2, 0, 0, 0, 0],
        [0x7fff_ffff, 1, 0, 0, 0, 0],
        [0x7fff_ffff, 0x7fff_ffff, 0, 0, 0, 0],
        [0x8000_0000, 0xffff_ffff, 0, 0, 0, 0],
        [0x8000_0000, 0x8000_0000, 0, 0, 0, 0],
        [0x7fff_fffe, 1, 0, 0, 0, 0],
        [0xffff_ffff, 1, 0, 0, 0, 0],
        [0x8000_0001, 0xffff_ffff, 0, 0, 0, 0],
    ];
    let shift: Vec<[u32; 6]> = vec![
        [1, 1, 0, 0, 0, 0],
        [0x4000_0000, 1, 0, 0, 0, 0],
        [0x3fff_ffff, 1, 0, 0, 0, 0],
        [0xc000_0000, 1, 0, 0, 0, 0],
        [0xbfff_ffff, 1, 0, 0, 0, 0],
        [1, 31, 0, 0, 0, 0],
        [1, 30, 0, 0, 0, 0],
        [0xffff_ffff, 31, 0, 0, 0, 0],
        [0x1234, 8, 0, 0, 0, 0],
        [0x1234_5678, 8, 0, 0, 0, 0],
    ];
    let wide: Vec<[u32; 6]> = vec![
        [0, 0, 0, 0, 0, 0],
        [0x7fff_ffff, 0, 0, 0, 0, 0],
        [0x8000_0000, 0xffff_ffff, 0, 0, 0, 0],
        [0xffff_ffff, 0xffff_ffff, 0, 0, 0, 0],
        [0x8000_0000, 0, 0, 0, 0, 0],
        [0x0000_0001, 0x0000_0001, 0, 0, 0, 0],
        [0xffff_fff0, 0x7fff_ffff, 0, 0, 0, 0],
        [0, 0x8000_0000, 0, 0, 0, 0],
    ];
    let half: Vec<[u32; 6]> = vec![
        [0x0000_0001, 0x0000_0002, 0, 0, 0, 0],
        [0x0000_8000, 0x0000_8000, 0, 0, 0, 0],
        [0x0000_8000, 0xffff_8000, 0, 0, 0, 0],
        [0x0000_7fff, 0x0000_7fff, 0, 0, 0, 0],
        [0xffff_8000, 0xffff_8000, 0, 0, 0, 0],
        [0x0000_4000, 0x0000_4000, 0, 0, 0, 0],
    ];
    let pair64: Vec<[u32; 6]> = vec![
        [1, 0, 2, 0, 0, 0],
        [0xffff_ffff, 0x7fff_ffff, 1, 0, 0, 0],
        [0, 0x8000_0000, 0xffff_ffff, 0xffff_ffff, 0, 0],
        [0, 0x7fff_ffff, 0, 0x7fff_ffff, 0, 0],
        [0, 0x8000_0000, 0, 0x8000_0000, 0, 0],
        [5, 6, 7, 8, 0, 0],
    ];
    let vh: Vec<[u32; 6]> = vec![
        [0x0001_0002, 0x0003_0004, 0x0005_0006, 0x0007_0008, 0, 0],
        [0x7fff_7fff, 0x7fff_7fff, 0x0001_0001, 0x0001_0001, 0, 0],
        [0x8000_8000, 0x8000_8000, 0xffff_ffff, 0xffff_ffff, 0, 0],
        [0x7ffe_7ffe, 0x0000_0001, 0x0001_0000, 0x7fff_0001, 0, 0],
    ];
    vec![
        Sat { name: "add", text: "r6 = add(r0, r1):sat", ins: edge.clone(),
            model: |r| { let (x, o) = sat32(r[0] as i32 as i64 + r[1] as i32 as i64); (vec![(6, x)], o) } },
        Sat { name: "sub", text: "r6 = sub(r0, r1):sat", ins: edge.clone(),
            model: |r| { let (x, o) = sat32(r[0] as i32 as i64 - r[1] as i32 as i64); (vec![(6, x)], o) } },
        Sat { name: "abs", text: "r6 = abs(r0):sat", ins: edge.clone(),
            model: |r| { let (x, o) = sat32((r[0] as i32 as i64).abs()); (vec![(6, x)], o) } },
        Sat { name: "neg", text: "r6 = neg(r0):sat", ins: edge.clone(),
            model: |r| { let (x, o) = sat32(-(r[0] as i32 as i64)); (vec![(6, x)], o) } },
        Sat { name: "asl_reg", text: "r6 = asl(r0, r1):sat", ins: shift.clone(),
            model: |r| { let (x, o) = sat32((r[0] as i32 as i64) << (r[1] & 63)); (vec![(6, x)], o) } },
        Sat { name: "asl_imm", text: "r6 = asl(r0, #3):sat", ins: shift.clone(),
            model: |r| { let (x, o) = sat32((r[0] as i32 as i64) << 3); (vec![(6, x)], o) } },
        Sat { name: "mpy_ll_s1", text: "r6 = mpy(r0.l, r1.l):<<1:sat", ins: half.clone(),
            model: |r| { let p = (r[0] as i16 as i64) * (r[1] as i16 as i64) * 2; let (x, o) = sat32(p); (vec![(6, x)], o) } },
        Sat { name: "satb", text: "r6 = satb(r0)", ins: wide.clone(),
            model: |r| { let x = r[0] as i32; let y = x.clamp(-128, 127); (vec![(6, y as u32)], x != y) } },
        Sat { name: "sath", text: "r6 = sath(r0)", ins: wide.clone(),
            model: |r| { let x = r[0] as i32; let y = x.clamp(-32768, 32767); (vec![(6, y as u32)], x != y) } },
        Sat { name: "satub", text: "r6 = satub(r0)", ins: wide.clone(),
            model: |r| { let x = r[0] as i32; let y = x.clamp(0, 255); (vec![(6, y as u32)], x != y) } },
        Sat { name: "satuh", text: "r6 = satuh(r0)", ins: wide.clone(),
            model: |r| { let x = r[0] as i32; let y = x.clamp(0, 65535); (vec![(6, y as u32)], x != y) } },
        Sat { name: "sat64", text: "r6 = sat(r1:0)", ins: wide.clone(),
            model: |r| { let x = ((r[1] as u64) << 32 | r[0] as u64) as i64; let (y, o) = sat32(x); (vec![(6, y)], o) } },
        Sat { name: "add64", text: "r7:6 = add(r1:0, r3:2):sat", ins: pair64.clone(),
            model: |r| {
                let a = ((r[1] as u64) << 32 | r[0] as u64) as i64 as i128;
                let b = ((r[3] as u64) << 32 | r[2] as u64) as i64 as i128;
                let s = a + b;
                let (y, o) = if s > i64::MAX as i128 { (i64::MAX, true) } else if s < i64::MIN as i128 { (i64::MIN, true) } else { (s as i64, false) };
                (vec![(6, y as u32), (7, (y >> 32) as u32)], o)
            } },
        Sat { name: "vaddh_sat", text: "r7:6 = vaddh(r1:0, r3:2):sat", ins: vh.clone(),
            model: |r| {
                let a = lanes_h((r[1] as u64) << 32 | r[0] as u64);
                let b = lanes_h((r[3] as u64) << 32 | r[2] as u64);
                let mut out = 0u64;
                let mut ovf = false;
                for i in 0..4 {
                    let s = a[i] as i32 + b[i] as i32;
                    let y = s.clamp(-32768, 32767);
                    ovf |= y != s;
                    out |= ((y as i16 as u16) as u64) << (16 * i);
                }
                (vec![(6, out as u32), (7, (out >> 32) as u32)], ovf)
            } },
        Sat { name: "vsubh_sat", text: "r7:6 = vsubh(r1:0, r3:2):sat", ins: vh.clone(),
            model: |r| {
                let a = lanes_h((r[1] as u64) << 32 | r[0] as u64);
                let b = lanes_h((r[3] as u64) << 32 | r[2] as u64);
                let mut out = 0u64;
                let mut ovf = false;
                for i in 0..4 {
                    let s = a[i] as i32 - b[i] as i32;
                    let y = s.clamp(-32768, 32767);
                    ovf |= y != s;
                    out |= ((y as i16 as u16) as u64) << (16 * i);
                }
                (vec![(6, out as u32), (7, (out >> 32) as u32)], ovf)
            } },
    ]
}

fn set_inputs(mut vv: Vector, r: &[u32; 6]) -> Vector {
    for (i, &x) in r.iter().enumerate() {
        vv = vv.set(i, x);
    }
    vv
}

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();

    // ---- Every saturating instruction alone, with USR.OVF clear and set ----
    for s in sats() {
        let mut case = Case::new(format!("sat_{}", s.name), format!("{{ {} }}", s.text));
        for r in &s.ins {
            for usr_in in [0u32, OVF] {
                let (res, ovf) = (s.model)(r);
                let mut vv = set_inputs(v().set(ST_USR, usr_in), r);
                for (reg, val) in res {
                    vv = vv.exp(reg, val);
                }
                vv = vv.exp(ST_USR, usr_in | if ovf { OVF } else { 0 });
                case = case.vec(vv);
            }
        }
        out.push(case);
    }

    // ---- Pairs of saturating instructions in one packet: OVF is the OR -----
    let all = sats();
    let find = |n: &str| all.iter().find(|s| s.name == n).unwrap();
    for (an, bn) in [("add", "sub"), ("add", "abs"), ("sath", "satb"), ("asl_imm", "neg"),
                     ("add64", "add"), ("vaddh_sat", "sath")] {
        let (a, b) = (find(an), find(bn));
        // Retarget b's destination so the two results do not collide.
        let a_text = a.text.to_string();
        let b_text = b.text.replace("r6", "r8").replace("r7:6", "r9:8");
        let mut case = Case::new(format!("sat_pair_{an}_{bn}"), format!("{{ {a_text}; {b_text} }}"));
        for ra in &a.ins {
            // Pair each a-vector with b's first/last vectors (overflowing or not).
            for rb in [&b.ins[0], b.ins.last().unwrap(), &b.ins[1]] {
                // Both instructions read r0..r5, so the inputs must serve
                // both; use a's inputs for a and mask-free reuse for b.
                let _ = rb;
                let (ra_res, ovf_a) = (a.model)(ra);
                let (rb_res, ovf_b) = (b.model)(ra);
                let mut vv = set_inputs(v(), ra);
                for (reg, val) in ra_res {
                    vv = vv.exp(reg, val);
                }
                for (reg, val) in rb_res {
                    vv = vv.exp(reg + 2, val);
                }
                vv = vv.exp(ST_USR, if ovf_a || ovf_b { OVF } else { 0 });
                case = case.vec(vv);
                break;
            }
        }
        out.push(case);
    }

    // ---- USR is read before the packet's own saturation shows --------------
    out.push(Case::new("usr_read_sees_old_ovf",
        "{ r6 = add(r0, r1):sat; r3 = usr }")
        .vec(v().set(0, 0x7fff_ffff).set(1, 1).exp(6, 0x7fff_ffff).exp(3, 0).exp(ST_USR, OVF))
        .vec(v().set(0, 1).set(1, 1).set(ST_USR, OVF).exp(6, 2).exp(3, OVF).exp(ST_USR, OVF))
        .vec(v().set(0, 1).set(1, 1).exp(6, 2).exp(3, 0).exp(ST_USR, 0)));
    // Non-saturating work leaves a sticky OVF alone.
    out.push(Case::new("ovf_sticky_across_plain_packet",
        "{ r6 = add(r0, r1); r7 = mpyi(r0, r1); p0 = cmp.eq(r0, r1) }")
        .vec(v().set(ST_USR, OVF).set(0, 3).set(1, 4).exp(6, 7).exp(7, 12)
            .exp(ST_P, (def_state(ST_P) & !0xff))));
    // Writing USR in a packet with other work.
    out.push(Case::new("usr_write_with_alu",
        "{ usr = r0; r6 = add(r1, r2) }")
        .vec(v().set(0, OVF).set(1, 1).set(2, 2).exp(ST_USR, OVF).exp(6, 3).set(ST_USR, 0))
        .vec(v().set(0, 0).set(1, 1).set(2, 2).exp(ST_USR, 0).exp(6, 3).set(ST_USR, OVF)));
    // Writable-bit mask of USR.
    out.push(Case::new("usr_write_masks_reserved_bits",
        "{ usr = r0 }\n{ r6 = usr }")
        .vec(v().set(0, 0xffff_ffff).exp(6, 0x3fff_ff7f).exp(ST_USR, 0x3fff_ff7f)));

    // ---- Floating point flags ----------------------------------------------
    let one = 0x3f80_0000u32;
    let two = 0x4000_0000u32;
    let three = 0x4040_0000u32;
    let tiny = 0x3080_0000u32; // 2^-30
    let big = 0x7f7f_ffffu32; // max finite
    let min_norm_p1 = 0x0080_0001u32;
    let half = 0x3f00_0000u32;
    let fp: Vec<(&str, &str, u32, u32, u32, u32)> = vec![
        // name, text, a, b, result, flags
        ("sfadd_exact", "r6 = sfadd(r0, r1)", one, two, three, 0),
        ("sfadd_inexact", "r6 = sfadd(r0, r1)", one, tiny, one, FPINX),
        ("sfadd_overflow", "r6 = sfadd(r0, r1)", big, big, 0x7f80_0000, FPOVF | FPINX),
        ("sfmpy_exact", "r6 = sfmpy(r0, r1)", two, three, 0x40c0_0000, 0),
        ("sfmpy_underflow", "r6 = sfmpy(r0, r1)", min_norm_p1, half, 0x0040_0000, FPUNF | FPINX),
        ("sfsub_exact", "r6 = sfsub(r0, r1)", three, two, one, 0),
    ];
    for (n, text, a, b, res, flags) in &fp {
        for usr_in in [0u32, FPINV, OVF | FPUNF] {
            out.push(Case::new(format!("fp_{n}"), format!("{{ {text} }}"))
                .vec(v().set(0, *a).set(1, *b).set(ST_USR, usr_in)
                    .exp(6, *res).exp(ST_USR, usr_in | flags)));
        }
    }
    // Flags OR across the FP instructions of a packet.
    out.push(Case::new("fp_flags_or_in_packet",
        "{ r6 = sfadd(r0, r1); r7 = sfmpy(r2, r3) }")
        .vec(v().set(0, one).set(1, tiny).set(2, big).set(3, two)
            .exp(6, one).exp(7, 0x7f80_0000).exp(ST_USR, FPINX | FPOVF)));
    out.push(Case::new("fp_exact_pair_no_flags",
        "{ r6 = sfadd(r0, r1); r7 = sfmpy(r2, r3) }")
        .vec(v().set(0, one).set(1, two).set(2, two).set(3, three)
            .exp(6, three).exp(7, 0x40c0_0000).exp(ST_USR, 0)));
    // Rounding mode from USR.FPRND (00 nearest even, 01 toward zero, 10 toward
    // -inf, 11 toward +inf).  1.0 + 0.75 ulp and -1.0 - 0.75 ulp.
    let q = 0x33c0_0000u32; // 0.75 * 2^-23
    for (mode, pos, neg) in [
        (0u32, 0x3f80_0001u32, 0xbf80_0001u32),
        (1, 0x3f80_0000, 0xbf80_0000),
        (2, 0x3f80_0000, 0xbf80_0001),
        (3, 0x3f80_0001, 0xbf80_0000),
    ] {
        out.push(Case::new(format!("fp_round_mode{mode}"),
            "{ r6 = sfadd(r0, r1); r7 = sfadd(r2, r3) }")
            .vec(v().set(ST_USR, mode << 22).set(0, one).set(1, q).set(2, 0xbf80_0000).set(3, q | 0x8000_0000)
                .exp(6, pos).exp(7, neg).exp(ST_USR, (mode << 22) | FPINX)));
    }
    out
}
