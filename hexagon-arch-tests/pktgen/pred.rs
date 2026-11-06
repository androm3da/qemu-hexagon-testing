// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Predicated instructions and predicate-register semantics in a packet.
//!
//! Architectural rules exercised:
//!  * `if (p)` reads the predicate as it was before the packet; `if (p.new)`
//!    reads the value computed by a compare in the same packet.
//!  * A predicated-false instruction has no architectural effect at all: no
//!    register write, no memory access, and no post-increment of its base.
//!  * Several instructions may write the same predicate in a packet; the
//!    results are ANDed.  Several predicated instructions may write the same
//!    GPR if their predicates are complementary (exactly one executes).
//!  * A predicate-cancelled store neither writes memory nor is seen by an
//!    overlapping load of the same packet.
//!
//! All expected values are derived from those rules and confirmed on
//! hexagon-sim.

use super::*;

#[derive(Clone, Copy, PartialEq)]
enum Sense {
    P,
    NotP,
    PNew,
    NotPNew,
}

const SENSES: [Sense; 4] = [Sense::P, Sense::NotP, Sense::PNew, Sense::NotPNew];

impl Sense {
    fn text(self) -> &'static str {
        match self {
            Sense::P => "if (p0)",
            Sense::NotP => "if (!p0)",
            Sense::PNew => "if (p0.new)",
            Sense::NotPNew => "if (!p0.new)",
        }
    }
    fn complement(self) -> Sense {
        match self {
            Sense::P => Sense::NotP,
            Sense::NotP => Sense::P,
            Sense::PNew => Sense::NotPNew,
            Sense::NotPNew => Sense::PNew,
        }
    }
    fn is_new(self) -> bool {
        matches!(self, Sense::PNew | Sense::NotPNew)
    }
    fn tag(self) -> &'static str {
        match self {
            Sense::P => "p",
            Sense::NotP => "np",
            Sense::PNew => "pnew",
            Sense::NotPNew => "npnew",
        }
    }
}

/// The predicate scenario for one case.
#[derive(Clone, Copy)]
struct Scenario {
    sense: Sense,
    /// Whether the instruction under test executes.
    cond: bool,
    /// For old-value senses: also write p0 in the packet (to the opposite
    /// value) so that the instruction must use the OLD value.
    overwrite: bool,
}

impl Scenario {
    fn tag(&self) -> String {
        format!(
            "{}_{}{}",
            self.sense.tag(),
            if self.cond { "T" } else { "F" },
            if self.overwrite { "_ow" } else { "" }
        )
    }
    fn has_producer(&self) -> bool {
        self.sense.is_new() || self.overwrite
    }
    /// The value of p0 as read by an old-value predicate.
    /// Value of p0 after the packet, as a boolean.
    fn p_new(&self) -> bool {
        if self.sense.is_new() {
            // `.new` senses: cond = p_new (P) / !p_new (NotPNew).
            match self.sense {
                Sense::PNew => self.cond,
                _ => !self.cond,
            }
        } else {
            // Old-value senses: p_new is the opposite of p_old.
            !self.p_old()
        }
    }
    fn p_old(&self) -> bool {
        match self.sense {
            Sense::P => self.cond,
            Sense::NotP => !self.cond,
            // `.new` senses: arrange the old value to be the opposite so a
            // wrongly-read old value would give the wrong answer.
            _ => !self.p_new(),
        }
    }
    /// Text of the producer instruction, if the packet needs one.
    fn producer(&self) -> Option<&'static str> {
        if self.has_producer() {
            Some("p0 = cmp.eq(r10, r11)")
        } else {
            None
        }
    }
    fn vector(&self) -> Vector {
        let mut v = Vector::new();
        // P input: p0 byte = old value; p1..p3 distinct and must survive.
        let pin = (if self.p_old() { 0xff } else { 0 }) | 0x0000_a500u32 | 0x00ff_0000;
        v = v.set(ST_P, pin);
        if self.has_producer() {
            v = v.set(10, 5).set(11, if self.p_new() { 5 } else { 6 });
            let pout = (pin & !0xff) | if self.p_new() { 0xff } else { 0 };
            v = v.exp(ST_P, pout);
        }
        v
    }
    /// Packet text with `body` (instruction text containing `{P}` for the
    /// predicate prefix) plus the producer.
    fn packet(&self, body: &str) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(p) = self.producer() {
            parts.push(p.to_string());
        }
        parts.push(body.replace("{P}", self.sense.text()));
        format!("{{ {} }}", parts.join("; "))
    }
}

/// One effect of an instruction.
#[derive(Clone)]
enum Eff {
    Reg(usize, u32),
    RegWin(usize, u32),
    Mem(usize, Vec<u8>),
}

fn apply(mut v: Vector, effs: &[Eff]) -> Vector {
    for e in effs {
        v = match e {
            Eff::Reg(r, x) => v.exp(*r, *x),
            Eff::RegWin(r, o) => v.exp_win(*r, *o),
            Eff::Mem(o, b) => v.exp_mem(*o, b),
        };
    }
    v
}

const WB: usize = BASE as usize;

struct Kind {
    name: String,
    /// Text with `{P}`.
    text: String,
    /// Effects when the instruction executes.
    when: Vec<Eff>,
    /// Effects regardless of the predicate (e.g. a new-value producer).
    always: Vec<Eff>,
}

fn load_val(size: usize, signed: bool, off: usize) -> u64 {
    let raw = dval(off, size);
    match (size, signed) {
        (1, true) => raw as u8 as i8 as i32 as u32 as u64,
        (2, true) => raw as u16 as i16 as i32 as u32 as u64,
        _ => raw,
    }
}

fn kinds() -> Vec<Kind> {
    let mut k = Vec::new();
    let (d0, d1) = (d(0), d(1));

    k.push(Kind { name: "add".into(), text: "{P} r2 = add(r0, r1)".into(),
        when: vec![Eff::Reg(2, d0.wrapping_add(d1))], always: vec![] });
    k.push(Kind { name: "combine".into(), text: "{P} r3:2 = combine(r0, r1)".into(),
        when: vec![Eff::Reg(3, d0), Eff::Reg(2, d1)], always: vec![] });
    k.push(Kind { name: "imm".into(), text: "{P} r2 = #-7".into(),
        when: vec![Eff::Reg(2, -7i32 as u32)], always: vec![] });
    k.push(Kind { name: "immext".into(), text: "{P} r2 = ##0x12345678".into(),
        when: vec![Eff::Reg(2, 0x1234_5678)], always: vec![] });
    k.push(Kind { name: "zxtb".into(), text: "{P} r2 = zxtb(r0)".into(),
        when: vec![Eff::Reg(2, d0 & 0xff)], always: vec![] });

    let loads: [(&str, usize, bool); 6] = [
        ("memb", 1, true), ("memub", 1, false), ("memh", 2, true),
        ("memuh", 2, false), ("memw", 4, false), ("memd", 8, false),
    ];
    for (m, size, signed) in loads {
        let off = WB + 8;
        let val = load_val(size, signed, off);
        let dst = if size == 8 { "r3:2".to_string() } else { "r2".to_string() };
        let mut regs = vec![Eff::Reg(2, val as u32)];
        if size == 8 {
            regs.push(Eff::Reg(3, (val >> 32) as u32));
        }
        k.push(Kind { name: format!("ld_{m}"), text: format!("{{P}} {dst} = {m}(r20+#8)"),
            when: regs.clone(), always: vec![] });
        // Post-increment: loads from the (old) base, then bumps it.
        let off0 = WB;
        let val0 = load_val(size, signed, off0);
        let mut regs_pi0 = vec![Eff::Reg(2, val0 as u32)];
        if size == 8 {
            regs_pi0.push(Eff::Reg(3, (val0 >> 32) as u32));
        }
        regs_pi0.push(Eff::RegWin(20, BASE + size as u32));
        k.push(Kind { name: format!("ldpi_{m}"), text: format!("{{P}} {dst} = {m}(r20++#{size})"),
            when: regs_pi0, always: vec![] });
    }
    let stores: [(&str, usize); 4] = [("memb", 1), ("memh", 2), ("memw", 4), ("memd", 8)];
    for (m, size) in stores {
        let src = if size == 8 { "r1:0" } else { "r0" };
        let bytes: Vec<u8> = {
            let v = if size == 8 { ((d1 as u64) << 32) | d0 as u64 } else { d0 as u64 };
            (0..size).map(|i| (v >> (8 * i)) as u8).collect()
        };
        k.push(Kind { name: format!("st_{m}"), text: format!("{{P}} {m}(r20+#8) = {src}"),
            when: vec![Eff::Mem(WB + 8, bytes.clone())], always: vec![] });
        k.push(Kind { name: format!("stpi_{m}"), text: format!("{{P}} {m}(r20++#{size}) = {src}"),
            when: vec![Eff::Mem(WB, bytes.clone()), Eff::RegWin(20, BASE + size as u32)], always: vec![] });
    }
    // Store immediates.
    for (m, size) in [("memb", 1usize), ("memh", 2), ("memw", 4)] {
        let val: i32 = -5;
        let bytes: Vec<u8> = (0..size).map(|i| ((val as u32) >> (8 * i)) as u8).collect();
        k.push(Kind { name: format!("sti_{m}"), text: format!("{{P}} {m}(r20+#8) = #-5"),
            when: vec![Eff::Mem(WB + 8, bytes)], always: vec![] });
    }
    // New-value stores: the producer always executes.
    for (m, size) in [("memb", 1usize), ("memh", 2), ("memw", 4)] {
        let val = d0.wrapping_add(d1);
        let bytes: Vec<u8> = (0..size).map(|i| (val >> (8 * i)) as u8).collect();
        k.push(Kind { name: format!("stnv_{m}"),
            text: format!("r2 = add(r0, r1); {{P}} {m}(r20+#8) = r2.new"),
            when: vec![Eff::Mem(WB + 8, bytes.clone())],
            always: vec![Eff::Reg(2, val)] });
        k.push(Kind { name: format!("stnvpi_{m}"),
            text: format!("r2 = add(r0, r1); {{P}} {m}(r20++#{size}) = r2.new"),
            when: vec![Eff::Mem(WB, bytes), Eff::RegWin(20, BASE + size as u32)],
            always: vec![Eff::Reg(2, val)] });
    }
    k
}

/// Kinds with two predicated instructions using complementary predicates
/// (text has `{P}` and `{C}`; exactly one executes).
struct Pair {
    name: &'static str,
    text: &'static str,
    when_p: Vec<Eff>,
    when_c: Vec<Eff>,
}

fn pairs() -> Vec<Pair> {
    let (d0, d1) = (d(0), d(1));
    let ld = dval(WB, 4) as u32;
    vec![
        Pair { name: "imm_imm", text: "{P} r2 = #1; {C} r2 = #2",
            when_p: vec![Eff::Reg(2, 1)], when_c: vec![Eff::Reg(2, 2)] },
        Pair { name: "alu_ld", text: "{P} r2 = add(r0, r1); {C} r2 = memw(r20+#0)",
            when_p: vec![Eff::Reg(2, d0.wrapping_add(d1))], when_c: vec![Eff::Reg(2, ld)] },
        Pair { name: "ld_st", text: "{P} r2 = memw(r20+#0); {C} memw(r21+#0) = r0",
            when_p: vec![Eff::Reg(2, ld)], when_c: vec![Eff::Mem(WB + 0x40, d0.to_le_bytes().to_vec())] },
        Pair { name: "st_st_same_addr", text: "{P} memw(r20+#0) = r0; {C} memw(r20+#0) = r1",
            when_p: vec![Eff::Mem(WB, d0.to_le_bytes().to_vec())],
            when_c: vec![Eff::Mem(WB, d1.to_le_bytes().to_vec())] },
        Pair { name: "ldpi_ldpi", text: "{P} r2 = memw(r20++#4); {C} r2 = memw(r20++#8)",
            when_p: vec![Eff::Reg(2, ld), Eff::RegWin(20, BASE + 4)],
            when_c: vec![Eff::Reg(2, ld), Eff::RegWin(20, BASE + 8)] },
        Pair { name: "stpi_stpi", text: "{P} memw(r20++#4) = r0; {C} memw(r21++#4) = r1",
            when_p: vec![Eff::Mem(WB, d0.to_le_bytes().to_vec()), Eff::RegWin(20, BASE + 4)],
            when_c: vec![Eff::Mem(WB + 0x40, d1.to_le_bytes().to_vec()), Eff::RegWin(21, BASE + 0x40 + 4)] },
        Pair { name: "pairs_halves", text: "{P} r3:2 = combine(r0, r1); {C} r2 = r4",
            when_p: vec![Eff::Reg(3, d0), Eff::Reg(2, d1)], when_c: vec![Eff::Reg(2, d(4))] },
    ]
}

fn base_vector(sc: &Scenario) -> Vector {
    sc.vector()
        .set_win(20, BASE)
        .set_win(21, BASE + 0x40)
}

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();

    // ---- Single predicated instruction: sense x value x overwrite ----------
    let scenarios: Vec<Scenario> = {
        let mut s = Vec::new();
        for sense in SENSES {
            for cond in [true, false] {
                if sense.is_new() {
                    s.push(Scenario { sense, cond, overwrite: false });
                } else {
                    s.push(Scenario { sense, cond, overwrite: false });
                    s.push(Scenario { sense, cond, overwrite: true });
                }
            }
        }
        s
    };
    for k in kinds() {
        for sc in &scenarios {
            let v = apply(base_vector(sc), &k.always);
            let v = if sc.cond { apply(v, &k.when) } else { v };
            out.push(Case::new(format!("{}_{}", k.name, sc.tag()), sc.packet(&k.text)).vec(v));
        }
    }

    // ---- Complementary predicated pairs -----------------------------------
    for pr in pairs() {
        for sc in &scenarios {
            let text = pr
                .text
                .replace("{P}", sc.sense.text())
                .replace("{C}", sc.sense.complement().text());
            let mut parts = Vec::new();
            if let Some(p) = sc.producer() {
                parts.push(p.to_string());
            }
            parts.push(text);
            let body = format!("{{ {} }}", parts.join("; "));
            let v = base_vector(sc);
            let v = apply(v, if sc.cond { &pr.when_p } else { &pr.when_c });
            out.push(Case::new(format!("pair_{}_{}", pr.name, sc.tag()), body).vec(v));
        }
    }

    // ---- Two predicated stores to overlapping bytes, independent predicates
    for (sa, sb) in [(4usize, 4usize), (4, 1), (1, 4), (2, 4), (4, 2), (8, 4), (4, 8)] {
        for off in [-3i32, -1, 0, 1, 2, 3, 4] {
            for pa in [true, false] {
                for pb in [true, false] {
                    let mn = |s: usize| match s { 1 => "memb", 2 => "memh", 4 => "memw", _ => "memd" };
                    let sr = |s: usize, first: bool| -> String {
                        match (s, first) {
                            (8, true) => "r1:0".into(),
                            (8, false) => "r3:2".into(),
                            (_, true) => "r0".into(),
                            (_, false) => "r2".into(),
                        }
                    };
                    // B's address must be naturally aligned.
                    let boff = (WB as i32 + off) as usize;
                    if boff % sb != 0 {
                        continue;
                    }
                    let body = format!(
                        "{{ if (p1) {}(r20+#0) = {}; if (p2) {}(r21+#0) = {} }}",
                        mn(sa), sr(sa, true), mn(sb), sr(sb, false)
                    );
                    let pin = (if pa { 0xff00 } else { 0 }) | (if pb { 0xff_0000 } else { 0 });
                    let mut v = Vector::new()
                        .set_win(20, BASE)
                        .set_win(21, BASE + off as u32)
                        .set(2, 0x1122_3344).set(3, 0x5566_7788)
                        .set(ST_P, pin);
                    let mut mem: Vec<u8> = (0..WIN_BYTES).map(def_mem_byte).collect();
                    // Execution order: first in source = slot 1 = first.
                    let a_val = if sa == 8 { ((d(1) as u64) << 32) | d(0) as u64 } else { d(0) as u64 };
                    let b_val = if sb == 8 { (0x5566_7788u64 << 32) | 0x1122_3344 } else { 0x1122_3344u64 };
                    if pa {
                        for i in 0..sa {
                            mem[WB + i] = (a_val >> (8 * i)) as u8;
                        }
                    }
                    if pb {
                        for i in 0..sb {
                            mem[boff + i] = (b_val >> (8 * i)) as u8;
                        }
                    }
                    v = v.exp_window(&mem);
                    out.push(Case::new(
                        format!("pst_{}_{}_@{:+}_{}{}", sa, sb, off, if pa { "T" } else { "F" }, if pb { "T" } else { "F" }),
                        body,
                    ).vec(v));
                }
            }
        }
    }

    // ---- Cancelled store versus an overlapping load in the same packet -----
    // :mem_noshuf puts the store in slot 1 and the load in slot 0, so a store
    // that executes is forwarded to the load; a cancelled one must not be.
    for sense in [Sense::P, Sense::PNew] {
        for cond in [true, false] {
            for (ss, ls) in [(4usize, 4usize), (1, 4), (4, 1), (2, 4), (8, 4), (4, 8), (2, 2)] {
                for off in [-4i32, -3, -2, -1, 0, 1, 2, 3, 4] {
                    let laddr = WB as i32 + off;
                    if laddr < 0 || laddr as usize % ls != 0 {
                        continue;
                    }
                    let laddr = laddr as usize;
                    let sc = Scenario { sense, cond, overwrite: false };
                    let sm = match ss { 1 => "memb", 2 => "memh", 4 => "memw", _ => "memd" };
                    let lm = match ls { 1 => "memub", 2 => "memuh", 4 => "memw", _ => "memd" };
                    let sr = if ss == 8 { "r1:0" } else { "r0" };
                    let ld = if ls == 8 { "r5:4" } else { "r4" };
                    let text = format!("{{P}} {sm}(r20+#0) = {sr}; {ld} = {lm}(r21+#0)");
                    let body = format!("{}:mem_noshuf", sc.packet(&text));
                    let mut mem: Vec<u8> = (0..WIN_BYTES).map(def_mem_byte).collect();
                    if cond {
                        let val = if ss == 8 { ((d(1) as u64) << 32) | d(0) as u64 } else { d(0) as u64 };
                        for i in 0..ss {
                            mem[WB + i] = (val >> (8 * i)) as u8;
                        }
                    }
                    // The load executes after the store (slot 0).
                    let mut lv = 0u64;
                    for i in 0..ls {
                        lv |= (mem[laddr + i] as u64) << (8 * i);
                    }
                    let mut v = base_vector(&sc).set_win(21, laddr as u32);
                    v = v.exp(4, lv as u32);
                    if ls == 8 {
                        v = v.exp(5, (lv >> 32) as u32);
                    }
                    v = v.exp_window(&mem);
                    out.push(Case::new(
                        format!("cancel_st{}_ld{}_@{:+}_{}", ss, ls, off, sc.tag()), body).vec(v));
                }
            }
        }
    }

    // ---- Predicate auto-AND -----------------------------------------------
    // Several compares writing p0 in one packet: p0 = AND of the results.
    let pin = 0x00a5_5a00u32;
    for mask in 0..8u32 {
        // r0..r5 pairs: compare i is true iff its bit is set in `mask`.
        let t = |i: u32| mask & (1 << i) != 0;
        let cmp = |i: u32| format!("p0 = cmp.eq(r{}, r{})", 2 * i, 2 * i + 1);
        let mut v = Vector::new().set(ST_P, pin);
        for i in 0..3u32 {
            v = v.set(2 * i as usize, 7).set(2 * i as usize + 1, if t(i) { 7 } else { 8 });
        }
        let p3 = t(0) && t(1) && t(2);
        let p2 = t(0) && t(1);
        out.push(Case::new(format!("autoand2_{mask:03b}"),
            format!("{{ {}; {} }}", cmp(0), cmp(1)))
            .vec({
                let mut v = Vector::new().set(ST_P, pin);
                for i in 0..2u32 {
                    v = v.set(2 * i as usize, 7).set(2 * i as usize + 1, if t(i) { 7 } else { 8 });
                }
                v.exp(ST_P, (pin & !0xff) | if p2 { 0xff } else { 0 })
            }));
        out.push(Case::new(format!("autoand3_{mask:03b}"),
            format!("{{ {}; {}; {} }}", cmp(0), cmp(1), cmp(2)))
            .vec(v.exp(ST_P, (pin & !0xff) | if p3 { 0xff } else { 0 })));
        // Consumer of the ANDed value.
        let mut v = Vector::new().set(ST_P, pin);
        for i in 0..2u32 {
            v = v.set(2 * i as usize, 7).set(2 * i as usize + 1, if t(i) { 7 } else { 8 });
        }
        v = v.exp(ST_P, (pin & !0xff) | if p2 { 0xff } else { 0 });
        v = if p2 { v.exp(8, 1) } else { v.exp(9, 2) };
        out.push(Case::new(format!("autoand2_consume_{mask:03b}"),
            format!("{{ {}; {}; if (p0.new) r8 = #1; if (!p0.new) r9 = #2 }}", cmp(0), cmp(1)))
            .vec(v));
    }
    // Different predicates written in one packet do not interfere.
    out.push(Case::new("two_preds_independent",
        "{ p0 = cmp.eq(r0, r1); p1 = cmp.eq(r2, r3); p2 = cmp.gt(r4, r5); p3 = cmp.gt(r6, r7) }")
        .vec(Vector::new().set(ST_P, 0)
            .set(0, 1).set(1, 1).set(2, 1).set(3, 2)
            .set(4, 9).set(5, 3).set(6, 3).set(7, 9)
            .exp(ST_P, 0x00ff_00ff)));
    // Logical predicate ops read old values and also auto-AND.
    out.push(Case::new("pred_and_reads_old",
        "{ p0 = and(p1, p2); p1 = and(p0, p2) }")
        .vec(Vector::new().set(ST_P, 0x00ff_ff00)
            // old p0=0, p1=ff, p2=ff.  p0 = p1&p2 = ff; p1 = p0&p2 = 0 (old p0).
            .exp(ST_P, 0x00ff_00ff)));
    out.push(Case::new("pred_or_not_swap",
        "{ p0 = or(p1, !p2); p1 = not(p0) }")
        .vec(Vector::new().set(ST_P, 0x0000_ff00)
            // old p0=0,p1=ff,p2=0: p0 = ff | !0 = ff; p1 = !old p0 = ff.
            .exp(ST_P, 0x0000_ffff)));
    out.push(Case::new("pred_autoand_with_logical",
        "{ p0 = cmp.eq(r0, r1); p0 = and(p1, p2) }")
        .vec(Vector::new().set(ST_P, 0x00ff_ff00).set(0, 4).set(1, 4)
            // cmp true (ff) AND (p1&p2 = ff) = ff
            .exp(ST_P, 0x00ff_ffff)));
    out.push(Case::new("pred_autoand_with_logical_false",
        "{ p0 = cmp.eq(r0, r1); p0 = and(p1, p2) }")
        .vec(Vector::new().set(ST_P, 0x00ff_ff00).set(0, 4).set(1, 5)
            .exp(ST_P, 0x00ff_ff00)));
    out
}
