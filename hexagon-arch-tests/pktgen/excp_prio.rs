// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Several faults in one packet: which is taken, what BADVA/SSR report, and
//! that nothing of the packet survives.
//!
//! Rules (exception priority, BADVA behaviour, precise-exception text):
//!  * If both slots fault, the slot 1 exception is taken first (it has
//!    priority) and the slot 0 fault is not reported unless the packet is
//!    restarted and reaches it.
//!  * Within one access, a misaligned address (0x20 load / 0x21 store) is
//!    reported ahead of a TLB miss (0x70 / 0x71).
//!  * A TLB miss restarts the packet once the page is mapped; a misaligned
//!    access is an error the handler resumes past.  Either way the faulting
//!    packet has changed nothing: registers, predicates, USR and memory.
//!  * SSR.BVS names the slot reported; with two valid memory accesses
//!    V0 = V1 = 1; BADVA of the reported slot is its effective address.
//!
//! Known deviation, as in replay_rw: hexagon-sim leaves a store of the
//! packet visible when a load of the same packet faults, and the BADVA of
//! the slot that did not fault is stale; neither is asserted.

use super::*;

const PAGE: usize = 0x100;

fn v() -> Vector {
    Vector::new()
}

fn area(p: usize) -> u32 {
    (p * PAGE + 0x80) as u32
}

#[derive(Clone, Copy, PartialEq)]
enum Fault {
    Ok,
    /// Page unmapped.
    Miss,
    /// Address not aligned to the access size.
    Misal,
    /// Both.
    MisalMiss,
}

impl Fault {
    fn tag(self) -> &'static str {
        match self {
            Fault::Ok => "ok",
            Fault::Miss => "miss",
            Fault::Misal => "misal",
            Fault::MisalMiss => "misalmiss",
        }
    }
    fn misal(self) -> bool {
        matches!(self, Fault::Misal | Fault::MisalMiss)
    }
    fn miss(self) -> bool {
        matches!(self, Fault::Miss | Fault::MisalMiss)
    }
}

#[derive(Clone, Copy)]
struct Slot {
    store: bool,
    fault: Fault,
    page: usize,
}

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    let faults = [Fault::Ok, Fault::Miss, Fault::Misal, Fault::MisalMiss];
    for a_store in [false, true] {
        for b_store in [false, true] {
            for &fa in &faults {
                for &fb in &faults {
                    for (pa, pb) in [(0usize, 1usize), (1, 0), (0, 0)] {
                        if fa == Fault::Ok && fb == Fault::Ok {
                            continue;
                        }
                        // One page cannot be both mapped and unmapped.
                        if pa == pb && (fa.miss() != fb.miss()) && fa != Fault::Ok && fb != Fault::Ok {
                            continue;
                        }
                        if pa == pb && fa.miss() != fb.miss() {
                            // Ok-slot shares an unmapped page: that slot
                            // faults too; skip the ambiguous combination.
                            continue;
                        }
                        let a = Slot { store: a_store, fault: fa, page: pa };
                        let b = Slot { store: b_store, fault: fb, page: pb };
                        let mut unmapped = 0u32;
                        if fa.miss() {
                            unmapped |= 1 << pa;
                        }
                        if fb.miss() {
                            unmapped |= 1 << pb;
                        }
                        let a_ea = area(pa) + 8 + fa.misal() as u32;
                        let b_ea = area(pb) + 0x10 + fb.misal() as u32;
                        let a_ins = if a.store { "memw(r20+#8) = r0" } else { "r2 = memw(r20+#8)" };
                        let b_ins = if b.store { "memw(r21+#16) = r1" } else { "r3 = memw(r21+#16)" };
                        // Side effects that must not survive an abandoned packet.
                        let body = format!(
                            "{{ {a_ins}; {b_ins} }}:mem_noshuf"
                        );
                        let mut vv = v().set_win(20, area(pa) + fa.misal() as u32)
                            .set_win(21, area(pb) + fb.misal() as u32);

                        // Walk the replays.
                        let mut mapped = [unmapped & 1 == 0, unmapped & 2 == 0];
                        let mut abandoned = false;
                        let mut guard = 0;
                        loop {
                            guard += 1;
                            assert!(guard < 10);
                            // Slot 1 first.
                            if a.fault.misal() {
                                vv = vv.ev(Ev::new(EvKind::Err, if a.store { 0x21 } else { 0x20 })
                                    .flags(true, true, true).va1(a_ea).va0_strict(b_ea));
                                abandoned = true;
                                break;
                            }
                            if a.fault.miss() && !mapped[pa] {
                                vv = vv.ev(Ev::new(EvKind::Rw, if a.store { 0x71 } else { 0x70 })
                                    .flags(true, true, true).va1(a_ea).va0_strict(b_ea));
                                mapped[pa] = true;
                                continue;
                            }
                            if b.fault.misal() {
                                vv = vv.ev(Ev::new(EvKind::Err, if b.store { 0x21 } else { 0x20 })
                                    .flags(true, true, false).va0(b_ea).va1_strict(a_ea));
                                abandoned = true;
                                break;
                            }
                            if b.fault.miss() && !mapped[pb] {
                                vv = vv.ev(Ev::new(EvKind::Rw, if b.store { 0x71 } else { 0x70 })
                                    .flags(true, true, false).va0(b_ea).va1_strict(a_ea));
                                mapped[pb] = true;
                                continue;
                            }
                            break;
                        }
                        if !abandoned {
                            // Both accesses went through (only reachable
                            // when misaligned accesses are absent).
                            if a.store {
                                vv = vv.exp_mem(a_ea as usize, &le32(d(0)));
                            } else {
                                vv = vv.exp(2, dval(a_ea as usize, 4) as u32);
                            }
                            if b.store {
                                vv = vv.exp_mem(b_ea as usize, &le32(d(1)));
                            } else {
                                vv = vv.exp(3, dval(b_ea as usize, 4) as u32);
                            }
                        }
                        // hexagon-sim lets the store of an abandoned packet
                        // through when the other slot's load faults; the spec
                        // says the packet changed nothing.  Don't assert on the
                        // store's bytes.
                        if abandoned && a.store != b.store && !strict() {
                            let (st_ea, st_misal) = if a.store { (a_ea, fa.misal()) } else { (b_ea, fb.misal()) };
                            let _ = st_misal;
                            vv = vv.ignore_mem(st_ea as usize, 8);
                        }
                        let mut case = Case::new(
                            format!(
                                "prio_{}{}_{}{}_p{}p{}",
                                if a.store { "st" } else { "ld" }, fa.tag(),
                                if b.store { "st" } else { "ld" }, fb.tag(), pa, pb
                            ),
                            body,
                        ).data_replay(unmapped, 1).resume_at_end().elr_at(PROLOGUE).vec(vv);
                        if a.store != b.store {
                            case = case.no_partial_check();
                        }
                        out.push(case);
                    }
                }
            }
        }
    }

    // ---- A misaligned slot 1 load abandons a packet whose slot 0 memop must not run
    // hexagon-sim 21.0.04 runs the memop anyway (sim_bugs_nov5/): PKT_STRICT only.
    for (p1, p0) in [(0usize, 1usize), (1, 0), (0, 0)].into_iter().filter(|_| strict()) {
        let a_ea = area(p1) + 8 + 1;
        let op_ea = area(p0) + 0x10;
        let vv = v().set_win(20, area(p1) + 1).set_win(21, area(p0))
            .ev(Ev::new(EvKind::Err, 0x20).flags(true, true, true).va1(a_ea).va0_strict(op_ea));
        out.push(Case::new(format!("misal_ld_abandons_memop_p{p1}p{p0}"),
            "{ r3 = memw(r20+#8); memw(r21+#16) += r8 }".to_string())
            .data_replay(0, 1).resume_at_end().elr_at(PROLOGUE).vec(vv));
    }

    // ---- A single misaligned access: nothing else in the packet survives ---
    let side = "r4 = add(r4, #1); p1 = cmp.eq(r12, r12); r6 = add(r10, r11):sat";
    for (name, ins, cause, ea_off, store) in [
        ("ld_w", "r2 = memw(r20+#8)", 0x20u32, 1u32, false),
        ("ld_h", "r2 = memh(r20+#8)", 0x20, 1, false),
        ("ld_d", "r3:2 = memd(r20+#8)", 0x20, 4, false),
        ("ldpi_w", "r2 = memw(r20++#4)", 0x20, 2, false),
        ("st_w", "memw(r20+#8) = r0", 0x21, 1, true),
        ("st_h", "memh(r20+#8) = r0", 0x21, 1, true),
        ("st_d", "memd(r20+#8) = r1:0", 0x21, 4, true),
        ("stpi_w", "memw(r20++#4) = r0", 0x21, 2, true),
    ] {
        let _ = store;
        let base = area(0);
        let disp = if ins.contains("++") { 0 } else { 8 };
        let ea = base + ea_off + disp;
        let vv = v().set_win(20, base + ea_off).set(ST_P, 0).set(10, 0x7fff_ffff).set(11, 1)
            .ev(Ev::new(EvKind::Err, cause).flags(true, false, false).va0(ea).va1_unchanged());
        out.push(Case::new(format!("single_misal_{name}_with_side_effects"),
            format!("{{ {ins}; {side} }}"))
            .data_replay(0, 1).resume_at_end().elr_at(PROLOGUE).vec(vv));
    }
    out
}
