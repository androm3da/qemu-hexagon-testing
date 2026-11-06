// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Change of flow inside packets: dual jumps, compound compare-and-jump,
//! calls, frame returns and hardware loops.
//!
//! Rules (hexagon-sim observed): a packet takes at most one branch - when
//! two are taken, the first in packet order wins; the other contributes
//! nothing (in particular a second `call` does not write LR).  All the
//! packet's other effects (stores, register and predicate writes) commit
//! whether or not a branch is taken.  Branch operands are read at packet
//! start, so a register written in the packet does not redirect a `jumpr`
//! or `callr` of the same packet.  `endloopN` redirects only while the loop
//! count is greater than one and decrements it; `spNloop0` makes P3 false
//! for the first N iterations and true after.

use super::*;

const WB: usize = BASE as usize;

fn v() -> Vector {
    Vector::new()
}

// ---------------------------------------------------------------------------
// Dual jumps
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum J {
    /// `if (pN) jump:HINT target`
    Pred(usize, bool, &'static str),
    /// unconditional `jump target`
    Uncond,
    /// `if (pN) jumpr:HINT rS`
    PredR(usize, bool),
    /// unconditional `jumpr rS`
    UncondR,
}

fn jtext(j: J, label: &str, reg: usize) -> String {
    match j {
        J::Pred(p, neg, hint) => {
            format!("if ({}p{p}) jump{hint} {label}", if neg { "!" } else { "" })
        }
        J::Uncond => format!("jump {label}"),
        J::PredR(p, neg) => format!("if ({}p{p}) jumpr:t r{reg}", if neg { "!" } else { "" }),
        J::UncondR => format!("jumpr r{reg}"),
    }
}

fn jtaken(j: J, p: u32) -> bool {
    let pb = |n: usize| (p >> (8 * n)) & 1 != 0;
    match j {
        J::Pred(n, neg, _) | J::PredR(n, neg) => pb(n) != neg,
        J::Uncond | J::UncondR => true,
    }
}

fn dual_jump_cases(out: &mut Vec<Case>) {
    // Pairs the architecture allows in one packet (the assembler rejects the
    // others).
    // Only PC-relative jumps pair, and the first must be conditional.
    let firsts = [
        J::Pred(0, false, ":t"),
        J::Pred(0, false, ":nt"),
        J::Pred(0, true, ":t"),
        J::Pred(0, true, ":nt"),
    ];
    let seconds = [
        J::Pred(1, false, ":t"),
        J::Pred(1, true, ":nt"),
        J::Uncond,
    ];
    for (fi, &f) in firsts.iter().enumerate() {
        for (si, &s) in seconds.iter().enumerate() {
            // Both jumps may be written in either source order; the
            // assembler keeps it (the first listed has priority).
            for swap in [false, true] {
                // An unconditional jump cannot be the first branch.
                if swap && matches!(s, J::Uncond) {
                    continue;
                }
                let (a, b, la, lb) = if swap { (s, f, "2f", "1f") } else { (f, s, "1f", "2f") };
                let body = format!(
                    "r5 = ##1f\nr6 = ##2f\n{{ {}; {} }}\n{{ r10 = #1; jump 9f }}\n1:\n{{ r11 = #1; jump 9f }}\n2:\n{{ r12 = #1 }}\n9:",
                    jtext(a, la, 5),
                    jtext(b, lb, 6)
                )
                .replace("r5 = ##1f\nr6 = ##2f\n", "{ r5 = ##1f; r6 = ##2f }\n");
                let mut case = Case::new(
                    format!("dual_jump_f{}_s{}{}", fi, si, if swap { "_swapped" } else { "" }),
                    body,
                );
                for pa in [0u32, 1] {
                    for pb in [0u32, 1] {
                        // Packet text names p0 for `f` and p1 for `s`.
                        let p = pa | (pb << 8);
                        let (ta, tb) = if swap {
                            (jtaken(s, p), jtaken(f, p))
                        } else {
                            (jtaken(f, p), jtaken(s, p))
                        };
                        let mut vv = v().set(ST_P, (if pa == 1 { 0xff } else { 0 }) | (if pb == 1 { 0xff00 } else { 0 }))
                            .ignore(5).ignore(6);
                        // Targets: 1 for the first-listed jump, 2 for the second.
                        // The first-listed jump targets `la`, the second `lb`.
                        let (m_first, m_second) = if swap { (12, 11) } else { (11, 12) };
                        vv = if ta {
                            vv.exp(m_first, 1)
                        } else if tb {
                            vv.exp(m_second, 1)
                        } else {
                            vv.exp(10, 1)
                        };
                        // r12 is only reached by falling out of label 1/2
                        // sequence: label 1 jumps to 9, label 2 falls to 9.
                        case = case.vec(vv);
                    }
                }
                out.push(case);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Compound compare-and-jump and jumps with side effects
// ---------------------------------------------------------------------------

fn compound_cases(out: &mut Vec<Case>) {
    struct C {
        name: &'static str,
        text: &'static str,
        eval: fn(u32, u32) -> bool,
    }
    let cs = [
        C { name: "eq", text: "cmp.eq(r0, r1)", eval: |a, b| a == b },
        C { name: "gt", text: "cmp.gt(r0, r1)", eval: |a, b| (a as i32) > (b as i32) },
        C { name: "gtu", text: "cmp.gtu(r0, r1)", eval: |a, b| a > b },
        C { name: "eq_i", text: "cmp.eq(r0, #7)", eval: |a, _| a == 7 },
        C { name: "gt_i", text: "cmp.gt(r0, #7)", eval: |a, _| (a as i32) > 7 },
        C { name: "gtu_i", text: "cmp.gtu(r0, #7)", eval: |a, _| a > 7 },
        C { name: "eq_m1", text: "cmp.eq(r0, #-1)", eval: |a, _| a == 0xffff_ffff },
        C { name: "gt_m1", text: "cmp.gt(r0, #-1)", eval: |a, _| (a as i32) > -1 },
        C { name: "tstbit0", text: "tstbit(r0, #0)", eval: |a, _| a & 1 != 0 },
    ];
    let vals: [(u32, u32); 8] = [
        (7, 7), (8, 7), (6, 7), (0xffff_ffff, 7), (0, 7), (1, 0), (0x8000_0000, 1), (3, 3),
    ];
    for c in &cs {
        for pn in [0usize, 1] {
            for neg in [false, true] {
                for hint in [":t", ":nt"] {
                    let n = if neg { "!" } else { "" };
                    let body = format!(
                        "{{ p{pn} = {}; if ({n}p{pn}.new) jump{hint} 1f }}\n{{ r10 = #1 }}\n1:",
                        c.text
                    );
                    let mut case = Case::new(
                        format!("compound_{}_p{pn}_{}{}", c.name, if neg { "n" } else { "" }, &hint[1..]),
                        body,
                    );
                    for &(a, b) in &vals {
                        let cond = (c.eval)(a, b);
                        // `!p.new` negates the stored predicate, not the compare.
                        let taken = cond != neg;
                        let pin = 0x00a5_5a00u32;
                        let mask = !(0xffu32 << (8 * pn));
                        let pout = (pin & mask) | if cond { 0xff << (8 * pn) } else { 0 };
                        let mut vv = v().set(0, a).set(1, b).set(ST_P, pin).exp(ST_P, pout);
                        if !taken {
                            vv = vv.exp(10, 1);
                        }
                        case = case.vec(vv);
                    }
                    out.push(case);
                }
            }
        }
    }

    // Side effects commit whether or not the branch is taken.
    for taken in [true, false] {
        let pin = if taken { 0xff } else { 0 };
        for (name, extra) in [
            ("store", "memw(r20+#0) = r0"),
            ("alu", "r2 = add(r0, r1)"),
            ("load", "r2 = memw(r20+#0)"),
            ("pred", "p1 = cmp.eq(r0, r0)"),
            ("store_pi", "memw(r20++#4) = r0"),
        ] {
            let body = format!("{{ if (p0) jump 1f; {extra} }}\n{{ r10 = #1 }}\n1:");
            let mut vv = v().set_win(20, BASE).set(ST_P, pin);
            vv = match name {
                "store" => vv.exp_mem(WB, &le32(d(0))),
                "alu" => vv.exp(2, d(0).wrapping_add(d(1))),
                "load" => vv.exp(2, dval(WB, 4) as u32),
                "pred" => vv.exp(ST_P, pin | 0xff00),
                _ => vv.exp_mem(WB, &le32(d(0))).exp_win(20, BASE + 4),
            };
            if !taken {
                vv = vv.exp(10, 1);
            }
            out.push(Case::new(
                format!("jump_side_effect_{name}_{}", if taken { "taken" } else { "nottaken" }),
                body,
            ).vec(vv));
        }
    }

    // callr through LR itself: the target is the old LR, then LR becomes the
    // return address.
    out.push(Case::new("callr_through_lr",
        "{ r31 = ##1f }\n{ callr r31; r6 = ##2f }\n2:\n{ r10 = #1; jump 9f }\n1:\n{ r11 = #1; r7 = sub(r31, r6) }\n9:")
        .vec(v().ignore(6).ignore(ST_LR).exp(7, 0).exp(11, 1)));
    out.push(Case::new("jumpr_through_lr",
        "{ r31 = ##1f }\n{ jumpr r31; r6 = ##1f }\n{ r10 = #1 }\n1:\n{ r7 = sub(r31, r6) }")
        .vec(v().ignore(6).ignore(ST_LR).exp(7, 0)));

    // jumpr / callr read the register as it was before the packet.
    out.push(Case::new("jumpr_target_written_same_packet",
        "{ r5 = ##1f; r6 = ##2f }\n{ r5 = r6; jumpr r5 }\n{ r10 = #1; jump 9f }\n1:\n{ r11 = #1; jump 9f }\n2:\n{ r12 = #1 }\n9:")
        .vec(v().ignore(5).ignore(6).exp(11, 1)));
    out.push(Case::new("callr_target_written_same_packet",
        "{ r5 = ##1f; r6 = ##2f }\n{ r5 = r6; callr r5 }\n{ r10 = #1; jump 9f }\n1:\n{ r11 = #1; jump 9f }\n2:\n{ r12 = #1 }\n9:")
        .vec(v().ignore(5).ignore(6).ignore(ST_LR).exp(11, 1)));
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

fn call_cases(out: &mut Vec<Case>) {
    // A not-taken predicated call does not write LR; a taken one writes the
    // address of the next packet.  r6 holds that address for comparison.
    for taken in [true, false] {
        for form in ["if (p0) call 1f", "if (!p0) call 1f", "if (p0) callr r5"] {
            let negated = form.contains('!');
            let cond = if negated { !taken } else { taken };
            let body = format!(
                "{{ r5 = ##1f; r6 = ##2f }}\n{{ {form}; r3 = r31 }}\n2:\n{{ r10 = #1 }}\n1:\n{{ r7 = sub(r31, r6) }}"
            );
            // After a taken call LR = 2:, so r7 = 0 (and r10 is skipped);
            // not taken: LR keeps its input value.
            let mut vv = v().set(ST_P, if taken { 0xff } else { 0 }).ignore(5).ignore(6);
            vv = vv.exp(3, 0xC0DE_0123);
            if cond {
                vv = vv.exp(7, 0).ignore(ST_LR);
            } else {
                vv = vv.exp(10, 1).ignore(7).exp(ST_LR, 0xC0DE_0123);
            }
            out.push(Case::new(
                format!("pred_call_{}_{}", form.replace(['(', ')', ' ', '!'], ""), if taken { "T" } else { "F" }),
                body,
            ).vec(vv));
        }
    }
    // Only the first taken branch of a packet may write LR.
    out.push(Case::new("call_after_taken_jump_no_lr",
        "{ r5 = ##1f; r6 = ##2f }\n{ if (p0) jump 1f; call 2f }\n{ r10 = #1 }\n1:\n{ jump 9f }\n2:\n{ r11 = #1 }\n9:")
        .vec(v().set(ST_P, 0xff).ignore(5).ignore(6).exp(ST_LR, 0xC0DE_0123)));
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

fn frame_cases(out: &mut Vec<Case>) {
    // Subroutine with a frame; we call it, check conditions of the return.
    for (name, ret, p0v, expect_ret) in [
        ("dealloc_return", "dealloc_return", 0u32, true),
        ("dealloc_return_pt", "if (p0) dealloc_return", 0xff, true),
        ("dealloc_return_pf", "if (p0) dealloc_return", 0, false),
        ("dealloc_return_npt", "if (!p0) dealloc_return", 0, true),
        ("dealloc_return_npf", "if (!p0) dealloc_return", 0xff, false),
        ("dealloc_return_pnew_t", "p0 = cmp.eq(r0, r0); if (p0.new) dealloc_return:t", 0, true),
        ("dealloc_return_pnew_nt", "p0 = cmp.eq(r0, r0); if (p0.new) dealloc_return:nt", 0, true),
        ("dealloc_return_npnew_f", "p0 = cmp.eq(r0, r0); if (!p0.new) dealloc_return:t", 0, false),
    ] {
        let body = format!(
            "{{ call 10f }}\n{{ r9 = #1; jump 99f }}\n10:\n{{ allocframe(#16) }}\n{{ r2 = sub(r30, r29) }}\n{{ {ret} }}\n{{ r8 = #1 }}\n{{ dealloc_return }}\n99:"
        );
        // After allocframe(#16): FP - SP = 16 (the frame plus saved FP/LR
        // pair sits above SP).
        let mut vv = v().set(ST_P, p0v).exp(9, 1).ignore(ST_LR);
        if ret.contains("cmp.eq") {
            // The compare in the return packet writes p0 = true.
            vv = vv.exp(ST_P, 0xff);
        }
        // The packets after the early return are skipped; a not-taken
        // predicated return falls to `r8 = #1` and then the full return.
        vv = vv.exp(2, 16);
        if !expect_ret {
            vv = vv.exp(8, 1);
        }
        out.push(Case::new(format!("frame_{name}"), body).vec(vv));
    }
    // allocframe reads the old SP/FP in its packet.
    out.push(Case::new("allocframe_reads_old_sp_fp",
        "{ call 10f }\n{ jump 99f }\n10:\n{ allocframe(#8); r2 = r29; r3 = r30 }\n{ dealloc_return }\n99:")
        .vec(v().ignore(2).ignore(3).ignore(ST_LR)));
    // dealloc_return with a load in the same packet (slot 1).
    out.push(Case::new("dealloc_return_with_load",
        "{ call 10f }\n{ jump 99f }\n10:\n{ allocframe(#8) }\n{ r2 = memw(r20+#0); dealloc_return }\n99:")
        .vec(v().set_win(20, BASE).exp(2, dval(WB, 4) as u32).ignore(ST_LR)));
    out.push(Case::new("dealloc_return_with_alu",
        "{ call 10f }\n{ jump 99f }\n10:\n{ allocframe(#8) }\n{ r2 = add(r0, r1); r3 = r29; dealloc_return }\n99:")
        .vec(v().exp(2, d(0).wrapping_add(d(1))).ignore(3).ignore(ST_LR)));
}

// ---------------------------------------------------------------------------
// Hardware loops
// ---------------------------------------------------------------------------

fn loop_cases(out: &mut Vec<Case>) {
    // Loop counters end at 1: endloop only decrements while LC > 1.
    for count in [1u32, 2, 3, 4, 7] {
        // Single-packet body.
        out.push(Case::new(
            format!("loop0_single_packet_n{count}"),
            format!("{{ loop0(1f, #{count}) }}\n1:\n{{ r8 = add(r8, #1) }}:endloop0"),
        ).vec(v().set(8, 0).exp(8, count).exp(ST_LC0, 1).ignore(ST_SA0)));
        out.push(Case::new(
            format!("loop1_single_packet_n{count}"),
            format!("{{ loop1(1f, #{count}) }}\n1:\n{{ r8 = add(r8, #1) }}:endloop1"),
        ).vec(v().set(8, 0).exp(8, count).exp(ST_LC1, 1).ignore(ST_SA1)));
        // Register trip count.
        out.push(Case::new(
            format!("loop0_reg_count_n{count}"),
            "{ loop0(1f, r5) }\n1:\n{ r8 = add(r8, #1) }:endloop0".to_string(),
        ).vec(v().set(5, count).set(8, 0).exp(8, count).exp(ST_LC0, 1).ignore(ST_SA0)));
        // Multi-packet body; the last packet holds a store with post-increment.
        let mut vv = v().set_win(20, BASE).set(8, 0).exp(8, count).exp(ST_LC0, 1).ignore(ST_SA0)
            .exp_win(20, BASE + 4 * count);
        for i in 0..count {
            vv = vv.exp_mem(WB + 4 * i as usize, &le32(i));
        }
        out.push(Case::new(
            format!("loop0_multi_packet_store_n{count}"),
            format!("{{ loop0(1f, #{count}) }}\n1:\n{{ r9 = add(r8, #0) }}\n{{ r10 = add(r10, #1) }}\n{{ memw(r20++#4) = r9; r8 = add(r8, #1) }}:endloop0"),
        ).vec(vv.exp(9, count - 1).exp(10, d(10).wrapping_add(count))));
        // The endloop packet reads LC0: record it per iteration.
        let mut vv = v().set_win(20, BASE).exp(ST_LC0, 1).ignore(ST_SA0).exp_win(20, BASE + 4 * count)
            .exp(2, 1);
        for i in 0..count {
            // LC0 is read before the endloop of this iteration decrements it.
            vv = vv.exp_mem(WB + 4 * i as usize, &le32(count - i));
        }
        out.push(Case::new(
            format!("loop0_read_lc0_n{count}"),
            format!("{{ loop0(1f, #{count}) }}\n1:\n{{ r2 = lc0 }}\n{{ memw(r20++#4) = r2 }}:endloop0"),
        ).vec(vv));
        // Software-pipelined loops: P3 is false for the first K iterations.
        for k in [1u32, 2, 3] {
            let n = count;
            let true_iters = n.saturating_sub(k);
            let lpcfg_left = k.saturating_sub(n);
            let p3_end = if n >= k { 0xff } else { 0 };
            out.push(Case::new(
                format!("sp{k}loop0_n{count}"),
                format!("{{ p3 = sp{k}loop0(1f, #{count}) }}\n1:\n{{ if (p3) r8 = add(r8, #1); r9 = add(r9, #1) }}:endloop0"),
            ).vec(v().set(8, 0).set(9, 0).exp(8, true_iters).exp(9, n).exp(ST_LC0, 1)
                .ignore(ST_SA0)
                .exp(ST_P, (d_p() & 0x00ff_ffff) | (p3_end << 24))
                .exp(ST_USR, lpcfg_left << 8)));
        }
    }
    // loop0 clears LPCFG left by an earlier sp loop.
    out.push(Case::new("loop0_clears_lpcfg",
        "{ loop0(1f, #2) }\n1:\n{ r8 = add(r8, #1) }:endloop0")
        .vec(v().set(ST_USR, 0x0300).set(8, 0).exp(8, 2).exp(ST_USR, 0).exp(ST_LC0, 1).ignore(ST_SA0)));

    // Nested loops.
    for (outer, inner) in [(1u32, 1u32), (2, 1), (1, 2), (2, 3), (3, 2), (4, 4)] {
        let tag = format!("o{outer}_i{inner}");
        let total = outer * inner;
        // Separate end packets.
        out.push(Case::new(
            format!("nested_endloop0_endloop1_{tag}"),
            format!("{{ loop1(2f, #{outer}) }}\n2:\n{{ loop0(1f, #{inner}) }}\n1:\n{{ r8 = add(r8, #1) }}:endloop0\n{{ r9 = add(r9, #1) }}:endloop1"),
        ).vec(v().set(8, 0).set(9, 0).exp(8, total).exp(9, outer)
            .exp(ST_LC0, 1).exp(ST_LC1, 1).ignore(ST_SA0).ignore(ST_SA1)));
        // One packet ends both loops.
        out.push(Case::new(
            format!("nested_endloop01_{tag}"),
            format!("{{ loop1(2f, #{outer}) }}\n2:\n{{ loop0(1f, #{inner}) }}\n1:\n{{ r8 = add(r8, #1) }}:endloop01"),
        ).vec(v().set(8, 0).exp(8, total)
            .exp(ST_LC0, 1).exp(ST_LC1, 1).ignore(ST_SA0).ignore(ST_SA1)));
    }

    // Branch inside the body, skipping to the last packet.
    out.push(Case::new("loop_forward_jump_to_last_packet",
        "{ loop0(1f, #4) }\n1:\n{ p0 = cmp.eq(r8, #1) }\n{ if (p0) jump 2f }\n{ r9 = add(r9, #1) }\n2:\n{ r8 = add(r8, #1) }:endloop0")
        .vec(v().set(8, 0).set(9, 0).exp(8, 4).exp(9, 3).exp(ST_LC0, 1).ignore(ST_SA0)
            .exp(ST_P, (d_p() & !0xff) | 0)));
    // Leaving a loop early keeps LC0 where it was.
    out.push(Case::new("loop_early_exit_lc0",
        "{ loop0(1f, #5) }\n1:\n{ r8 = add(r8, #1) }\n{ p0 = cmp.eq(r8, #3) }\n{ if (p0) jump 9f }\n{ r9 = add(r9, #1) }:endloop0\n9:")
        .vec(v().set(8, 0).set(9, 0).exp(8, 3).exp(9, 2).exp(ST_LC0, 3).ignore(ST_SA0)
            .exp(ST_P, (d_p() & !0xff) | 0xff)));
    // A call inside the body does not disturb the loop registers.
    out.push(Case::new("loop_body_calls_subroutine",
        "{ loop0(1f, #3) }\n1:\n{ call 10f }\n{ r9 = add(r9, #1) }:endloop0\n{ jump 99f }\n10:\n{ r8 = add(r8, #1); jumpr r31 }\n99:")
        .vec(v().set(8, 0).set(9, 0).exp(8, 3).exp(9, 3).exp(ST_LC0, 1).ignore(ST_SA0).ignore(ST_LR)));
    // Endloop packet with a compare feeding a .new consumer.
    out.push(Case::new("loop_endloop_packet_pnew",
        "{ loop0(1f, #4) }\n1:\n{ p0 = cmp.gt(r8, #1); if (p0.new) r10 = add(r10, #1); r8 = add(r8, #1) }:endloop0")
        .vec(v().set(8, 0).set(10, 0).exp(8, 4).exp(10, 2).exp(ST_LC0, 1).ignore(ST_SA0)
            .exp(ST_P, (d_p() & !0xff) | 0xff)));
    // Dual stores and a load in the endloop packet.
    out.push(Case::new("loop_endloop_packet_dual_mem",
        "{ loop0(1f, #3) }\n1:\n{ memw(r20++#4) = r8; r9 = memw(r21++#4); r8 = add(r8, #1) }:endloop0")
        .vec(v().set_win(20, BASE).set_win(21, BASE + 0x40).set(8, 0)
            .exp(8, 3).exp_win(20, BASE + 12).exp_win(21, BASE + 0x40 + 12)
            .exp(9, dval(WB + 0x48, 4) as u32)
            .exp_mem(WB, &[0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0])
            .exp(ST_LC0, 1).ignore(ST_SA0)));
}

fn d_p() -> u32 {
    def_state(ST_P)
}

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    dual_jump_cases(&mut out);
    compound_cases(&mut out);
    call_cases(&mut out);
    frame_cases(&mut out);
    loop_cases(&mut out);
    out
}
