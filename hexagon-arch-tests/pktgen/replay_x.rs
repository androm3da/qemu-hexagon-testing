// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Instruction TLB misses taken while fetching a packet, with the page
//! boundary falling at every word offset of the code.
//!
//! The harness copies each case across a 4K boundary of an unmapped region
//! once per word offset, so every packet of it - including the wrapper
//! around the packet under test - straddles the boundary in some run.
//!
//! Rules (TLB-miss-X): the fetch of a packet whose first page is missing
//! raises 0x60; a packet that starts in a mapped page and continues into a
//! missing one raises 0x61 (the packet itself is restarted, ELR names it);
//! after the handler maps the page the packet re-executes and everything it
//! does happens exactly once.  A packet that starts exactly on the boundary
//! raises 0x60 for the new page.  The harness derives the expected cause
//! and ELR of each miss from the copied code's own packet boundaries.
//!
//! Only straight-line first passes are used (taken branches could skip the
//! packet the expected sequence is derived from).

use super::*;

fn v() -> Vector {
    Vector::new()
}

struct Body {
    name: &'static str,
    text: &'static str,
    vec: Vector,
}

fn bodies() -> Vec<Body> {
    let (d0, d1) = (d(0), d(1));
    let wb = BASE as usize;
    vec![
        Body { name: "four_alu", text: "{ r8 = #1; r9 = #2; r10 = #3; r11 = #4 }",
            vec: v().exp(8, 1).exp(9, 2).exp(10, 3).exp(11, 4) },
        Body { name: "two_ext", text: "{ r8 = ##0x11223344; r9 = ##0x55667788 }",
            vec: v().exp(8, 0x1122_3344).exp(9, 0x5566_7788) },
        Body { name: "ext_plus_two", text: "{ r8 = ##0x11223344; r9 = add(r0, r1); r10 = sub(r1, r0) }",
            vec: v().exp(8, 0x1122_3344).exp(9, d0.wrapping_add(d1)).exp(10, d1.wrapping_sub(d0)) },
        Body { name: "three_packets",
            text: "{ r8 = #1; r9 = #2; r10 = #3; r11 = #4 }\n{ r12 = ##0x12345678; r13 = add(r8, r9) }\n{ r14 = ##0x9abcdef0; r15 = ##0x0fedcba9 }\n{ r1 = add(r1, #1) }",
            vec: v().exp(8, 1).exp(9, 2).exp(10, 3).exp(11, 4).exp(12, 0x1234_5678)
                .exp(13, 3).exp(14, 0x9abc_def0).exp(15, 0x0fed_cba9)
                .exp(1, d1.wrapping_add(1)) },
        Body { name: "store_load", text: "{ memw(r20+#0) = r0; r2 = memw(r21+#0) }:mem_noshuf",
            vec: v().set_win(20, BASE).set_win(21, BASE + 0x40)
                .exp_mem(wb, &le32(d0)).exp(2, dval(wb + 0x40, 4) as u32) },
        Body { name: "dual_store_postinc", text: "{ memw(r20++#4) = r0; memd(r21++#8) = r3:2; r4 = add(r4, #1) }",
            vec: v().set_win(20, BASE).set_win(21, BASE + 0x40)
                .exp_win(20, BASE + 4).exp_win(21, BASE + 0x48).exp(4, d(4).wrapping_add(1))
                .exp_mem(wb, &le32(d0)).exp_mem(wb + 0x40, &le64((d(3) as u64) << 32 | d(2) as u64)) },
        Body { name: "nv_store", text: "{ r5 = add(r0, r1); memw(r20+#0) = r5.new; r6 = r5 }",
            vec: v().set_win(20, BASE).exp(5, d0.wrapping_add(d1)).exp(6, d(5))
                .exp_mem(wb, &le32(d0.wrapping_add(d1))) },
        Body { name: "pnew", text: "{ p0 = cmp.eq(r5, r5); if (p0.new) r2 = #7; if (!p0.new) r3 = #8 }",
            vec: v().set(ST_P, 0).exp(ST_P, 0xff).exp(2, 7) },
        Body { name: "pred_store_not_taken", text: "{ if (p0) memw(r20+#0) = r0; r4 = add(r4, #1) }",
            vec: v().set_win(20, BASE).set(ST_P, 0).exp(4, d(4).wrapping_add(1)) },
        Body { name: "sat_usr", text: "{ r6 = add(r10, r11):sat; r4 = add(r4, #1) }",
            vec: v().set(10, 0x7fff_ffff).set(11, 1).exp(6, 0x7fff_ffff).exp(ST_USR, 1)
                .exp(4, d(4).wrapping_add(1)) },
        Body { name: "jump_not_taken",
            text: "{ p0 = cmp.eq(r5, r6); if (p0.new) jump:t 1f }\n{ r10 = #1 }\n1:",
            vec: v().set(5, 1).set(6, 2).set(ST_P, 0).exp(10, 1) },
        Body { name: "loop_single_packet",
            text: "{ loop0(1f, #3) }\n1:\n{ r8 = add(r8, #1); r9 = add(r9, #2) }:endloop0",
            vec: v().set(8, 0).set(9, 0).exp(8, 3).exp(9, 6).exp(ST_LC0, 1).ignore(ST_SA0) },
        Body { name: "loop_multi_packet",
            text: "{ loop0(1f, #3) }\n1:\n{ r8 = add(r8, #1) }\n{ r9 = ##0x1000; r10 = add(r10, #1) }\n{ r11 = add(r11, #2); memw(r20++#4) = r8 }:endloop0",
            vec: v().set_win(20, BASE).set(8, 0).set(10, 0).set(11, 0)
                .exp(8, 3).exp(9, 0x1000).exp(10, 3).exp(11, 6).exp(ST_LC0, 1).ignore(ST_SA0)
                .exp_win(20, BASE + 12)
                .exp_mem(wb, &[1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0]) },
        Body { name: "call_next",
            text: "{ call 1f }\n1:\n{ r10 = #1 }",
            vec: v().ignore(ST_LR).exp(10, 1) },
    ]
}

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for b in bodies() {
        for (unmapped, mbm) in [(3u32, 1u32), (1, 1), (2, 1), (3, 2), (2, 2)] {
            out.push(Case::new(format!("fetch_{}_u{}_m{}", b.name, unmapped, mbm), b.text)
                .code_replay(unmapped, mbm).vec(b.vec.clone()));
        }
    }
    out
}
