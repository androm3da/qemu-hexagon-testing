// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! HVX vector memory accesses that miss in the TLB, and are replayed.
//!
//! Rules (coprocessor memory chapter, TLB-miss-RW and BADVA behaviour):
//!  * An aligned vector access is a single memory operation (slot 0).
//!  * An unaligned one (VMEMU) is two: an aligned transaction in slot 0 and
//!    one in slot 1 at the next vector.  With the 4K page boundary 128-byte
//!    aligned, a VMEMU at the end of a page therefore touches both pages.
//!    Slot 1 has priority, so its page is reported first; with both slots
//!    valid V0 = V1 = 1 and BVS names the one reported.  For address
//!    exceptions BADVA0 = VA and BADVA1 = VA + vector size.
//!  * The packet restarts when the page is mapped and each effect (vector
//!    result, store, scalar side effects) happens exactly once.
//!
//! The window offset 0x100 is the page boundary, so the vector at window
//! offset 0x80 ends the first page.

use super::hvx::{dv, vaddw};
use super::*;

fn v() -> Vector {
    Vector::new()
}

const WB: usize = BASE as usize; // 0x80: last 128-byte block of page 0

fn win_bytes(off: usize) -> Vec<u8> {
    dmem(off, VLEN)
}

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();

    // ---- Aligned vector access: one valid slot ----------------------------
    for page in 0..2usize {
        let base = if page == 0 { BASE } else { 0x100 + 0x80 };
        for mbm in 1..=2u32 {
            for store in [false, true] {
                let ins = if store { "vmem(r20+#0) = v2" } else { "v1 = vmem(r20+#0)" };
                let mut vv = v().set_win(20, base).exp(4, d(4).wrapping_add(1));
                vv = if store {
                    vv.exp_mem(base as usize, &dv(2))
                } else {
                    vv.expv(1, &win_bytes(base as usize))
                };
                for _ in 0..mbm {
                    vv = vv.ev(Ev::new(EvKind::Rw, if store { 0x71 } else { 0x70 })
                        .flags(true, false, false).va0(base).va1_unchanged());
                }
                out.push(Case::new(
                    format!("hvx_replay_{}_p{page}_m{mbm}", if store { "vmem_st" } else { "vmem_ld" }),
                    format!("{{ {ins}; r4 = add(r4, #1) }}"),
                ).hvx().data_replay(1 << page, mbm).elr_at(PROLOGUE).vec(vv));
            }
        }
    }

    // ---- VMEMU: two transactions ---------------------------------------------
    // (window offset of the access, the pages its two blocks lie on)
    for (blk, pages) in [(WB as u32, (0usize, 1usize)), (0x100, (1, 1))] {
        for o in [1u32, 2, 64, 127] {
            let va = blk + o;
            for store in [false, true] {
                for unmapped in 1..=3u32 {
                    for mbm in 1..=2u32 {
                        // Pages the two transactions use; skip masks that leave
                        // both pages mapped for this access.
                        let touched = (1u32 << pages.0) | (1u32 << pages.1);
                        if unmapped & touched == 0 {
                            continue;
                        }
                        let mut mem: Vec<u8> = (0..WIN_BYTES).map(def_mem_byte).collect();
                        let ins = if store { "vmemu(r20+#0) = v2" } else { "v1 = vmemu(r20+#0)" };
                        let mut vv = v().set_win(20, va).exp(4, d(4).wrapping_add(1));
                        if store {
                            mem[va as usize..va as usize + VLEN].copy_from_slice(&dv(2));
                            vv = vv.exp_window(&mem);
                        } else {
                            vv = vv.expv(1, &win_bytes(va as usize));
                        }
                        // Replays: slot 1 (second block, page `pages.1`) first,
                        // then slot 0.
                        let mut mapped = [unmapped & 1 == 0, unmapped & 2 == 0];
                        let mut cnt = [0u32; 2];
                        loop {
                            let (slot1, page) = if !mapped[pages.1] {
                                (true, pages.1)
                            } else if !mapped[pages.0] {
                                (false, pages.0)
                            } else {
                                break;
                            };
                            let mut e = Ev::new(EvKind::Rw, if store { 0x71 } else { 0x70 })
                                .flags(true, true, slot1);
                            e = if slot1 { e.va1(va + 128) } else { e.va0(va) };
                            vv = vv.ev(e);
                            cnt[page] += 1;
                            if cnt[page] >= mbm {
                                mapped[page] = true;
                            }
                        }
                        out.push(Case::new(
                            format!("hvx_replay_vmemu_{}_@{va:#x}_u{unmapped}_m{mbm}", if store { "st" } else { "ld" }),
                            format!("{{ {ins}; r4 = add(r4, #1) }}"),
                        ).hvx().data_replay(unmapped, mbm).elr_at(PROLOGUE).vec(vv));
                    }
                }
            }
        }
    }

    // ---- A vector load with an ALU op and a scalar store (two slots) --------
    for page_v in 0..2usize {
        for page_s in 0..2usize {
            for unmapped in 1..=3u32 {
                let vbase = if page_v == 0 { BASE } else { 0x180 };
                let sbase = if page_s == 0 { BASE + 0x10 } else { 0x190 };
                if vbase == sbase {
                    continue;
                }
                // `:mem_noshuf`: first is slot 1.
                let body = "{ v1 = vmem(r20+#0); memw(r21+#0) = r0; v3.w = vadd(v4.w, v5.w) }:mem_noshuf";
                let mut vv = v().set_win(20, vbase).set_win(21, sbase)
                    .expv(1, &win_bytes(vbase as usize)).expv(3, &vaddw(&dv(4), &dv(5)))
                    .exp_mem(sbase as usize, &le32(d(0)));
                let mut mapped = [unmapped & 1 == 0, unmapped & 2 == 0];
                loop {
                    let (slot1, ea, pg) = if !mapped[page_v] {
                        (true, vbase, page_v)
                    } else if !mapped[page_s] {
                        (false, sbase, page_s)
                    } else {
                        break;
                    };
                    let cause = if slot1 { 0x70 } else { 0x71 };
                    let mut e = Ev::new(EvKind::Rw, cause).flags(true, true, slot1);
                    e = if slot1 { e.va1(ea) } else { e.va0(ea) };
                    vv = vv.ev(e);
                    mapped[pg] = true;
                }
                // A store in the packet is visible when the vector load misses
                // on hexagon-sim (see replay_rw); don't assert on it.
                out.push(Case::new(
                    format!("hvx_replay_vld_sst_pv{page_v}_ps{page_s}_u{unmapped}"),
                    body,
                ).hvx().data_replay(unmapped, 1).elr_at(PROLOGUE).no_partial_check().vec(vv));
            }
        }
    }
    out
}
