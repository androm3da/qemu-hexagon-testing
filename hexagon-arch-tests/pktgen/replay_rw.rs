// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Data TLB misses taken by a packet, and the packet's replay.
//!
//! Architectural rules (TLB-miss-RW and BADVA behaviour):
//!  * A packet that misses commits nothing: registers, predicates, USR and
//!    memory are as they were before it.  Software maps the page and the
//!    packet is restarted, so each of its effects happens exactly once, no
//!    matter how many times it had to be restarted.
//!  * The cause is 0x70 for a read and 0x71 for a write; ELR names the
//!    packet.
//!  * If both memory slots hold valid (not predicate-cancelled) accesses,
//!    BADVA0/BADVA1 hold both effective addresses, SSR.V0 = V1 = 1 and BVS
//!    names the faulting slot - slot 1 has priority.  If only one slot holds
//!    a valid access only its BADVA/V are written (the other keeps its
//!    value, V cleared) and BVS names that slot.
//!  * A predicate-cancelled access never misses.
//!
//! Known deviation: the spec has the BADVA register of a valid but
//! non-faulting slot hold that slot's effective address (BADVA behaviour,
//! scenario 1) and says the exception point is the packet itself; on
//! hexagon-sim 21.0.04 that other slot's BADVA is stale, and a store in one
//! slot is already visible in memory when a load in the other slot of the
//! same packet misses.  Those two details are not asserted here (see `no_partial_check`).
//!
//! The window straddles a page boundary at offset 0x100: offsets below it
//! are on page 0, offsets from it on page 1.  The harness leaves the
//! chosen pages unmapped and satisfies a page only on its Nth miss.

use super::*;

const PAGE: usize = 0x100; // window offset of the page boundary

fn v() -> Vector {
    Vector::new()
}

/// Window offset of the page-`p` data area used by an op's base register.
fn area(p: usize) -> u32 {
    (p * PAGE + 0x80) as u32
}

/// An operation with its address and its architectural effect.
struct Op {
    name: &'static str,
    /// Instruction text; `{b}` is the base register.
    text: &'static str,
    /// Offset from the base the access hits.
    disp: i32,
    /// The access is a store (cause 0x71) rather than a load (0x70).
    store: bool,
    /// Extra instructions the op needs in the packet (producers).
    extra: &'static str,
    /// Post-increment amount applied to the base.
    inc: u32,
    /// Registers a load writes, with the access width in bytes and signedness.
    ld: Option<(usize, usize, bool)>,
    /// Bytes a store writes (size, data register).
    st: Option<(usize, usize)>,
}

fn ops() -> Vec<Op> {
    let mut o = Vec::new();
    for (m, sz, sg) in [("memb", 1usize, true), ("memub", 1, false), ("memh", 2, true),
                        ("memuh", 2, false), ("memw", 4, false), ("memd", 8, false)] {
        let dst = if sz == 8 { "r3:2" } else { "r2" };
        o.push(Op { name: Box::leak(format!("ld_{m}").into_boxed_str()),
            text: Box::leak(format!("{dst} = {m}({{b}}+#8)").into_boxed_str()),
            disp: 8, store: false, extra: "", inc: 0, ld: Some((2, sz, sg)), st: None });
        o.push(Op { name: Box::leak(format!("ldpi_{m}").into_boxed_str()),
            text: Box::leak(format!("{dst} = {m}({{b}}++#{sz})").into_boxed_str()),
            disp: 0, store: false, extra: "", inc: sz as u32, ld: Some((2, sz, sg)), st: None });
    }
    for (m, sz) in [("memb", 1usize), ("memh", 2), ("memw", 4), ("memd", 8)] {
        let src = if sz == 8 { "r1:0" } else { "r0" };
        o.push(Op { name: Box::leak(format!("st_{m}").into_boxed_str()),
            text: Box::leak(format!("{m}({{b}}+#8) = {src}").into_boxed_str()),
            disp: 8, store: true, extra: "", inc: 0, ld: None, st: Some((sz, 0)) });
        o.push(Op { name: Box::leak(format!("stpi_{m}").into_boxed_str()),
            text: Box::leak(format!("{m}({{b}}++#{sz}) = {src}").into_boxed_str()),
            disp: 0, store: true, extra: "", inc: sz as u32, ld: None, st: Some((sz, 0)) });
    }
    // A memop reads then writes; the miss is taken on the read.
    o.push(Op { name: "memop_w", text: "memw({b}+#8) += r8", disp: 8, store: false,
        extra: "", inc: 0, ld: None, st: None });
    o.push(Op { name: "sti_w", text: "memw({b}+#8) = #-5", disp: 8, store: true,
        extra: "", inc: 0, ld: None, st: None });
    o.push(Op { name: "nvst_w", text: "memw({b}+#8) = r5.new", disp: 8, store: true,
        extra: "r5 = add(r0, r1)", inc: 0, ld: None, st: None });
    o.push(Op { name: "nvstpi_h", text: "memh({b}++#2) = r5.new", disp: 0, store: true,
        extra: "r5 = add(r0, r1)", inc: 2, ld: None, st: None });
    o
}

/// Side effects that must happen exactly once.
struct Side {
    text: &'static str,
    set: fn(Vector) -> Vector,
    exp: fn(Vector) -> Vector,
}

fn sides() -> Vec<Side> {
    vec![
        Side { text: "r4 = add(r4, #1)", set: |v| v, exp: |v| v.exp(4, d(4).wrapping_add(1)) },
        Side { text: "p1 = cmp.eq(r12, r12)", set: |v| v.set(ST_P, 0), exp: |v| v.exp(ST_P, 0xff00) },
        Side { text: "r6 = add(r10, r11):sat",
            set: |v| v.set(10, 0x7fff_ffff).set(11, 1),
            exp: |v| v.exp(6, 0x7fff_ffff).exp(ST_USR, 1) },
    ]
}

fn data_word(off: usize, sz: usize) -> u64 {
    dval(off, sz)
}

fn ext(val: u64, sz: usize, signed: bool) -> u64 {
    match (sz, signed) {
        (1, true) => val as u8 as i8 as i32 as u32 as u64,
        (2, true) => val as u16 as i16 as i32 as u32 as u64,
        _ => val,
    }
}

/// Expected state change of `op` with base at window offset `base`.
fn effect(mut vv: Vector, op: &Op, base: u32, base_reg: usize) -> Vector {
    let ea = (base as i32 + op.disp) as usize;
    if let Some((reg, sz, sg)) = op.ld {
        let val = ext(data_word(ea, sz), sz, sg);
        vv = vv.exp(reg, val as u32);
        if sz == 8 {
            vv = vv.exp(reg + 1, (val >> 32) as u32);
        }
    }
    if let Some((sz, _)) = op.st {
        let data: u64 = if sz == 8 { (d(1) as u64) << 32 | d(0) as u64 } else { d(0) as u64 };
        vv = vv.exp_mem(ea, &data.to_le_bytes()[..sz]);
    } else if op.name == "memop_w" {
        vv = vv.exp_mem(ea, &le32((data_word(ea, 4) as u32).wrapping_add(d(8))));
    } else if op.name == "sti_w" {
        vv = vv.exp_mem(ea, &le32(-5i32 as u32));
    } else if op.name == "nvst_w" {
        vv = vv.exp(5, d(0).wrapping_add(d(1))).exp_mem(ea, &le32(d(0).wrapping_add(d(1))));
    } else if op.name == "nvstpi_h" {
        let val = d(0).wrapping_add(d(1));
        vv = vv.exp(5, val).exp_mem(ea, &le32(val)[..2]);
    }
    if op.inc != 0 {
        vv = vv.exp_win(base_reg, base + op.inc);
    }
    vv
}

fn rw_cause(store: bool) -> u32 {
    if store {
        0x71
    } else {
        0x70
    }
}

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();

    // ---- A single access, page 0 or page 1 unmapped, replayed 1-3 times ----
    let all_sides = sides();
    for op in ops() {
        for page in 0..2usize {
            for mbm in 1..=3u32 {
                for with_side in [false, true] {
                    let base = area(page);
                    let mut parts: Vec<String> = Vec::new();
                    if !op.extra.is_empty() {
                        parts.push(op.extra.to_string());
                    }
                    parts.push(op.text.replace("{b}", "r20"));
                    let nside = if with_side { 4 - parts.len() } else { 0 };
                    for s in all_sides.iter().take(nside) {
                        parts.push(s.text.to_string());
                    }
                    let body = format!("{{ {} }}", parts.join("; "));
                    let mut vv = v().set_win(20, base);
                    for s in all_sides.iter().take(nside) {
                        vv = (s.set)(vv);
                    }
                    vv = effect(vv, &op, base, 20);
                    for s in all_sides.iter().take(nside) {
                        vv = (s.exp)(vv);
                    }
                    // Memory is addressed through r20; an SSR/BADVA check
                    // per miss.
                    let ea = (base as i32 + op.disp) as u32;
                    for _ in 0..mbm {
                        vv = vv.ev(Ev::new(EvKind::Rw, rw_cause(op.store))
                            .flags(true, false, false).va0(ea).va1_unchanged());
                    }
                    out.push(Case::new(
                        format!("replay_{}_p{}_m{}{}", op.name, page, mbm, if with_side { "_side" } else { "" }),
                        body,
                    ).data_replay(1 << page, mbm).elr_at(PROLOGUE).vec(vv));
                }
            }
        }
    }

    // ---- Two accesses on (possibly) different pages -------------------------
    // `:mem_noshuf` makes the first access slot 1, the second slot 0; the
    // slot 1 miss is taken first.
    #[derive(Clone, Copy)]
    struct Acc {
        store: bool,
        page: usize,
        off: i32,
    }
    for a_store in [false, true] {
        for b_store in [false, true] {
            for pa in 0..2usize {
                for pb in 0..2usize {
                    for unmapped in 1..=3u32 {
                        for mbm in 1..=2u32 {
                            let a = Acc { store: a_store, page: pa, off: 8 };
                            let b = Acc { store: b_store, page: pb, off: 0x10 };
                            let a_ea = area(pa) + 8;
                            let b_ea = area(pb) + 0x10;
                            if a_ea == b_ea {
                                continue;
                            }
                            let a_ins = if a.store { "memw(r20+#8) = r0" } else { "r2 = memw(r20+#8)" };
                            let b_ins = if b.store { "memw(r21+#16) = r1" } else { "r3 = memw(r21+#16)" };
                            let body = format!("{{ {a_ins}; {b_ins} }}:mem_noshuf");
                            let mut vv = v().set_win(20, area(pa)).set_win(21, area(pb));
                            if a.store {
                                vv = vv.exp_mem(a_ea as usize, &le32(d(0)));
                            } else {
                                vv = vv.exp(2, data_word(a_ea as usize, 4) as u32);
                            }
                            if b.store {
                                vv = vv.exp_mem(b_ea as usize, &le32(d(1)));
                            } else {
                                vv = vv.exp(3, data_word(b_ea as usize, 4) as u32);
                            }
                            // Simulate the misses.
                            let mut mapped = [unmapped & 1 == 0, unmapped & 2 == 0];
                            let mut cnt = [0u32; 2];
                            loop {
                                let (hit, ea, store, slot1) = if !mapped[pa] {
                                    (pa, a_ea, a.store, true)
                                } else if !mapped[pb] {
                                    (pb, b_ea, b.store, false)
                                } else {
                                    break;
                                };
                                // Only the faulting slot's BADVA is checked: the
                                // spec also has the other valid slot's BADVA hold
                                // its effective address, but hexagon-sim leaves it
                                // stale (see the module comment).
                                let mut e = Ev::new(EvKind::Rw, rw_cause(store)).flags(true, true, slot1);
                                e = if slot1 { e.va1(ea) } else { e.va0(ea) };
                                // The other valid slot's BADVA holds its address too.
                                e = if slot1 { e.va0_strict(b_ea) } else { e.va1_strict(a_ea) };
                                vv = vv.ev(e);
                                cnt[hit] += 1;
                                if cnt[hit] >= mbm {
                                    mapped[hit] = true;
                                }
                            }
                            let mut case = Case::new(
                                format!("replay_dual_{}{}_p{}p{}_u{}_m{}",
                                    if a.store { "st" } else { "ld" }, if b.store { "st" } else { "ld" },
                                    pa, pb, unmapped, mbm),
                                body,
                            ).data_replay(unmapped, mbm).elr_at(PROLOGUE).vec(vv);
                            // hexagon-sim leaves a store of the packet visible when
                            // a load of the same packet misses (it still replays
                            // correctly); the spec says the exception point is
                            // the packet itself.
                            if a.store != b.store {
                                case = case.no_partial_check();
                            }
                            out.push(case);
                        }
                    }
                }
            }
        }
    }

    // ---- A non-idempotent slot 0 access next to a slot 1 access that misses ---
    // A memop (read-modify-write) must be applied exactly once however many
    // times the packet is restarted.
    struct Memop {
        name: &'static str,
        text: &'static str,
        sz: usize,
        f: fn(u32) -> u32,
    }
    let memops = [
        Memop { name: "w_add", text: "memw(r21+#16) += r8", sz: 4, f: |x| x.wrapping_add(d(8)) },
        Memop { name: "b_sub", text: "memb(r21+#16) -= r8", sz: 1, f: |x| (x as u8).wrapping_sub(d(8) as u8) as u32 },
        Memop { name: "h_setbit", text: "memh(r21+#16) = setbit(#3)", sz: 2, f: |x| (x as u16 | 8) as u32 },
        Memop { name: "w_add_imm", text: "memw(r21+#16) += #5", sz: 4, f: |x| x.wrapping_add(5) },
    ];
    // hexagon-sim 21.0.04 applies the memop once per attempt (see
    // sim_bugs_nov5/), so these cases exist only with PKT_STRICT=1.
    for mo in memops.iter().filter(|_| strict()) {
        // (A memop is a store, so the slot 1 access can only be a load.)
        for slot1_store in [false] {
            for (p1, p0) in [(0usize, 1usize), (1, 0), (0, 0)] {
                for mbm in 1..=2u32 {
                    let a_ea = area(p1) + 8;
                    let op_ea = area(p0) + 0x10;
                    let a_ins = if slot1_store { "memw(r20+#8) = r0" } else { "r3 = memw(r20+#8)" };
                    let old = data_word(op_ea as usize, mo.sz) as u32;
                    let new = (mo.f)(old);
                    let mut vv = v().set_win(20, area(p1)).set_win(21, area(p0));
                    vv = if slot1_store {
                        vv.exp_mem(a_ea as usize, &le32(d(0)))
                    } else {
                        vv.exp(3, data_word(a_ea as usize, 4) as u32)
                    };
                    // Same page: the memop's own access misses too.
                    vv = vv.exp_mem(op_ea as usize, &new.to_le_bytes()[..mo.sz]);
                    for _ in 0..mbm {
                        vv = vv.ev(Ev::new(EvKind::Rw, if slot1_store { 0x71 } else { 0x70 })
                            .flags(true, true, true).va1(a_ea).va0_strict(op_ea));
                    }
                    out.push(Case::new(
                        format!("replay_{}_then_memop_{}_p{p1}p{p0}_m{mbm}", if slot1_store { "st" } else { "ld" }, mo.name),
                        format!("{{ {a_ins}; {} }}", mo.text),
                    ).data_replay(1 << p1, mbm).elr_at(PROLOGUE).no_partial_check().vec(vv));
                }
            }
        }
    }

    // ---- Cancelled accesses never miss; a lone valid slot sets only its own
    // BADVA --------------------------------------------------------------------
    for (name, body, setup) in [
        ("pred_store", "{ if (p0) memw(r20+#8) = r0; r4 = add(r4, #1) }", 0usize),
        ("pred_load_pi", "{ if (p0) r2 = memw(r20++#4); r4 = add(r4, #1) }", 1),
        ("pred_nstore", "{ if (!p0) memw(r20+#8) = r0; r4 = add(r4, #1) }", 2),
    ] {
        let mut vv = v().set_win(20, area(0)).exp(4, d(4).wrapping_add(1));
        vv = match setup {
            2 => vv.set(ST_P, 0xff),
            _ => vv.set(ST_P, 0),
        };
        out.push(Case::new(format!("replay_cancelled_{name}"), body)
            .data_replay(3, 1).elr_at(PROLOGUE).vec(vv));
    }
    out.push(Case::new("replay_cancelled_pnew",
        "{ p0 = cmp.eq(r5, r6); if (p0.new) r2 = memw(r20++#4) }")
        .data_replay(3, 1).elr_at(PROLOGUE)
        .vec(v().set_win(20, area(0)).set(5, 1).set(6, 2).set(ST_P, 0xff)
            .exp(ST_P, 0xff00 & 0x0000 | 0x0000_0000)));

    // Slot 1 cancelled, slot 0 faults.
    out.push(Case::new("replay_slot1_cancelled_slot0_faults",
        "{ if (p0) memw(r20+#8) = r0; r3 = memw(r21+#16) }:mem_noshuf")
        .data_replay(2, 1).elr_at(PROLOGUE)
        .vec(v().set_win(20, area(0)).set_win(21, area(1)).set(ST_P, 0)
            .exp(3, data_word(area(1) as usize + 0x10, 4) as u32)
            .ev(Ev::new(EvKind::Rw, 0x70).flags(true, false, false)
                .va0(area(1) + 0x10).va1_unchanged())));
    // Slot 0 cancelled, slot 1 faults.
    out.push(Case::new("replay_slot0_cancelled_slot1_faults",
        "{ r3 = memw(r21+#16); if (p0) memw(r20+#8) = r0 }:mem_noshuf")
        .data_replay(2, 1).elr_at(PROLOGUE)
        .vec(v().set_win(20, area(0)).set_win(21, area(1)).set(ST_P, 0)
            .exp(3, data_word(area(1) as usize + 0x10, 4) as u32)
            .ev(Ev::new(EvKind::Rw, 0x70).flags(false, true, true)
                .va1(area(1) + 0x10).va0_unchanged())));

    // ---- Replays of packets that also change control flow / loop state -----
    for mbm in 1..=3u32 {
        let base = area(0);
        let mut vv = v().set_win(20, base).set(8, 0).exp(8, 3).exp(ST_LC0, 1).ignore(ST_SA0)
            .exp_win(20, base + 12);
        for i in 0..3u32 {
            vv = vv.exp_mem((base + 4 * i) as usize, &le32(i));
        }
        for _ in 0..mbm {
            vv = vv.ev(Ev::new(EvKind::Rw, 0x71).flags(true, false, false).va0(base).va1_unchanged());
        }
        out.push(Case::new(format!("replay_in_loop_m{mbm}"),
            "{ loop0(1f, #3) }\n1:\n{ memw(r20++#4) = r8; r8 = add(r8, #1) }:endloop0")
            .data_replay(1, mbm).vec(vv));
    }
    for mbm in 1..=2u32 {
        let base = area(0);
        let mut vv = v().set_win(20, base).ignore(ST_LR).ignore(6).exp(7, 0)
            .exp_mem((base + 8) as usize, &le32(d(0)));
        for _ in 0..mbm {
            vv = vv.ev(Ev::new(EvKind::Rw, 0x71).flags(true, false, false)
                .va0(base + 8).va1_unchanged());
        }
        out.push(Case::new(format!("replay_call_with_store_m{mbm}"),
            "{ call 1f; memw(r20+#8) = r0; r6 = ##1f }\n1:\n{ r7 = sub(r31, r6) }")
            .data_replay(1, mbm).vec(vv));
    }
    out
}
