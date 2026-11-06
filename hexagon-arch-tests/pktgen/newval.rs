// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! `.new` operands: new-value stores, new-value compare-and-jump, and
//! predicates consumed with `.new`.
//!
//! Architectural rule (hexagon-sim observed; the packet is decoded so the
//! consumer is paired with its producer wherever it appears in the packet):
//! a `.new` operand reads the value produced by an instruction of the SAME
//! packet, while every other operand of every instruction - including a
//! store's address registers - still reads the value from before the packet.
//! The source order of a packet is arbitrary, so every permutation of each
//! packet is generated: the assembler keeps the order it is given wherever
//! slot rules allow, which makes consumer-before-producer encodings.

use super::*;

const WB: usize = BASE as usize;

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

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

fn words(insn: &str) -> usize {
    if insn.contains("##") {
        2
    } else {
        1
    }
}

#[derive(Clone)]
struct Item {
    text: String,
    /// Marks the instruction as part of the producer or consumer group so
    /// reordering stays interesting; informational only.
    tag: &'static str,
}

// ---------------------------------------------------------------------------
// New-value stores
// ---------------------------------------------------------------------------

/// A value producer: instructions plus the effect they have.
struct Prod {
    name: &'static str,
    /// Instructions of the producer (one or two).
    insns: Vec<&'static str>,
    /// Register carrying the new value (named `rN.new` by the consumer).
    reg: usize,
    /// Resulting value.
    val: u32,
    setup: fn(Vector) -> Vector,
    eff: fn(Vector) -> Vector,
}

fn id(v: Vector) -> Vector {
    v
}

fn prods() -> Vec<Prod> {
    let (d0, d1) = (d(0), d(1));
    vec![
        Prod { name: "add", insns: vec!["r2 = add(r0, r1)"], reg: 2,
            val: d0.wrapping_add(d1), setup: id, eff: id },
        Prod { name: "imm", insns: vec!["r2 = #-9"], reg: 2, val: -9i32 as u32, setup: id, eff: id },
        Prod { name: "immext", insns: vec!["r2 = ##0x7654321"], reg: 2, val: 0x765_4321, setup: id, eff: id },
        Prod { name: "mpyi", insns: vec!["r2 = mpyi(r0, r1)"], reg: 2,
            val: d0.wrapping_mul(d1), setup: id, eff: id },
        Prod { name: "ldw", insns: vec!["r2 = memw(r21+#0)"], reg: 2,
            val: dval(WB + 0x40, 4) as u32, setup: id, eff: id },
        Prod { name: "ldub", insns: vec!["r2 = memub(r21+#1)"], reg: 2,
            val: dval(WB + 0x41, 1) as u32, setup: id, eff: id },
        Prod { name: "ldb", insns: vec!["r2 = memb(r21+#1)"], reg: 2,
            val: dval(WB + 0x41, 1) as u8 as i8 as i32 as u32, setup: id, eff: id },
        Prod { name: "ldpi", insns: vec!["r2 = memw(r21++#4)"], reg: 2,
            val: dval(WB + 0x40, 4) as u32, setup: id,
            eff: |v| v.exp_win(21, BASE + 0x40 + 4) },
    ]
}

struct Cons {
    name: &'static str,
    /// Text with `{n}` for the new-value register.
    text: &'static str,
    size: usize,
    /// Window offset written.
    addr: usize,
    eff: fn(Vector) -> Vector,
}

fn conss() -> Vec<Cons> {
    vec![
        Cons { name: "b", text: "memb(r20+#4) = r{n}.new", size: 1, addr: WB + 4, eff: id },
        Cons { name: "h", text: "memh(r20+#4) = r{n}.new", size: 2, addr: WB + 4, eff: id },
        Cons { name: "w", text: "memw(r20+#4) = r{n}.new", size: 4, addr: WB + 4, eff: id },
        Cons { name: "w_pi", text: "memw(r20++#4) = r{n}.new", size: 4, addr: WB,
            eff: |v| v.exp_win(20, BASE + 4) },
        Cons { name: "b_pi", text: "memb(r20++#1) = r{n}.new", size: 1, addr: WB,
            eff: |v| v.exp_win(20, BASE + 1) },
        Cons { name: "h_pi", text: "memh(r20++#2) = r{n}.new", size: 2, addr: WB,
            eff: |v| v.exp_win(20, BASE + 2) },
        Cons { name: "w_idx", text: "memw(r20+r4<<#2) = r{n}.new", size: 4, addr: WB + 12, eff: id },
        Cons { name: "h_idx", text: "memh(r20+r4<<#1) = r{n}.new", size: 2, addr: WB + 6, eff: id },
        Cons { name: "w_abs", text: "memw(##PKT_WINDOW+160) = r{n}.new", size: 4, addr: 160, eff: id },
    ]
}

/// Fillers that share the packet and must be unaffected.
struct Fill {
    name: &'static str,
    text: &'static str,
    eff: fn(Vector) -> Vector,
    setup: fn(Vector) -> Vector,
}

fn fills() -> Vec<Fill> {
    vec![
        Fill { name: "A", text: "r8 = #1", eff: |v| v.exp(8, 1), setup: id },
        Fill { name: "E", text: "r9 = ##0x11223344", eff: |v| v.exp(9, 0x1122_3344), setup: id },
        Fill { name: "C", text: "p1 = cmp.eq(r12, r13)",
            eff: |v| v.exp(ST_P, 0x0000_ff00 | 0x0000_0000),
            setup: |v| v.set(12, 5).set(13, 5).set(ST_P, 0) },
    ]
}

fn nv_store_cases(out: &mut Vec<Case>) {
    let fillsets: Vec<Vec<usize>> = vec![vec![], vec![0], vec![1], vec![2], vec![0, 2]];
    let fl = fills();
    for p in prods() {
        for c in conss() {
            for fs in &fillsets {
                // Build the instruction list.
                let mut items: Vec<Item> = Vec::new();
                for t in &p.insns {
                    items.push(Item { text: t.to_string(), tag: "P" });
                }
                let ctext = c.text.replace("{n}", &p.reg.to_string());
                items.push(Item { text: ctext, tag: "C" });
                for &f in fs {
                    items.push(Item { text: fl[f].text.to_string(), tag: "F" });
                }
                let w: usize = items.iter().map(|i| words(&i.text)).sum();
                if items.len() > 4 || w > 4 {
                    continue;
                }
                for (pi, order) in perms(&items).into_iter().enumerate() {
                    let body = format!(
                        "{{ {} }}",
                        order.iter().map(|i| i.text.as_str()).collect::<Vec<_>>().join("; ")
                    );
                    let mut v = Vector::new().set_win(20, BASE).set_win(21, BASE + 0x40).set(4, 3);
                    v = (p.setup)(v);
                    for &f in fs {
                        v = (fl[f].setup)(v);
                    }
                    v = (p.eff)(v);
                    v = (c.eff)(v);
                    for &f in fs {
                        v = (fl[f].eff)(v);
                    }
                    // A producer that is not the stored register's own file
                    // keeps its result in the register as well.
                    if p.reg == 2 || p.reg == 3 {
                        v = v.exp(p.reg, p.val);
                    }
                    let bytes: Vec<u8> = (0..c.size).map(|i| (p.val >> (8 * i)) as u8).collect();
                    v = v.exp_mem(c.addr, &bytes);
                    let sign = |i: &Item| i.tag;
                    let _ = sign;
                    out.push(Case::new(
                        format!("nvst_{}_{}_f{}_o{}", p.name, c.name,
                            fs.iter().map(|&f| fl[f].name).collect::<String>(), pi),
                        body,
                    ).vec(v));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// New-value store address/data separation
// ---------------------------------------------------------------------------

fn nv_store_special(out: &mut Vec<Case>) {
    let (d0, d1) = (d(0), d(1));
    // Index register is also the producer: the address uses the OLD value,
    // the stored data the NEW one.
    let old_idx = 2u32;
    let new_idx = old_idx.wrapping_add(d1);
    for order in [
        "{ r4 = add(r4, r1); memw(r20+r4<<#2) = r4.new }",
        "{ memw(r20+r4<<#2) = r4.new; r4 = add(r4, r1) }",
    ] {
        out.push(Case::new("nvst_index_is_producer", order)
            .vec(Vector::new().set_win(20, BASE).set(4, old_idx)
                .exp(4, new_idx)
                .exp_mem(WB + 4 * old_idx as usize, &le32(new_idx))));
    }
    // Base is the producer's source and destination of an unrelated write.
    for order in [
        "{ r5 = add(r4, #1); memw(r20+#0) = r5.new; r4 = r5 }",
        "{ memw(r20+#0) = r5.new; r4 = r5; r5 = add(r4, #1) }",
    ] {
        out.push(Case::new("nvst_producer_dest_also_read", order)
            .vec(Vector::new().set_win(20, BASE).set(4, 10)
                .exp(5, 11).exp(4, d(5))
                .exp_mem(WB, &le32(11))));
    }
    let _ = (d0,);
}

// ---------------------------------------------------------------------------
// New-value compare-and-jump
// ---------------------------------------------------------------------------

struct JCond {
    name: &'static str,
    /// Text with `{n}` for `rN.new`.
    text: &'static str,
    eval: fn(u32, u32) -> bool,
}

fn jconds() -> Vec<JCond> {
    vec![
        JCond { name: "eq_r", text: "cmp.eq({n}, r3)", eval: |n, o| n == o },
        JCond { name: "gt_r", text: "cmp.gt({n}, r3)", eval: |n, o| (n as i32) > (o as i32) },
        JCond { name: "gtu_r", text: "cmp.gtu({n}, r3)", eval: |n, o| n > o },
        JCond { name: "gt_r_rev", text: "cmp.gt(r3, {n})", eval: |n, o| (o as i32) > (n as i32) },
        JCond { name: "gtu_r_rev", text: "cmp.gtu(r3, {n})", eval: |n, o| o > n },
        JCond { name: "eq_m1", text: "cmp.eq({n}, #-1)", eval: |n, _| n == 0xffff_ffff },
        JCond { name: "eq_7", text: "cmp.eq({n}, #7)", eval: |n, _| n == 7 },
        JCond { name: "gt_m1", text: "cmp.gt({n}, #-1)", eval: |n, _| (n as i32) > -1 },
        JCond { name: "gt_7", text: "cmp.gt({n}, #7)", eval: |n, _| (n as i32) > 7 },
        JCond { name: "gtu_7", text: "cmp.gtu({n}, #7)", eval: |n, _| n > 7 },
        JCond { name: "tstbit0", text: "tstbit({n}, #0)", eval: |n, _| n & 1 != 0 },
    ]
}

pub fn jump_cases(out: &mut Vec<Case>) {
    // Producers for the jump: ALU with the value from r0, load with the value
    // from memory, post-increment load.
    #[derive(Clone, Copy)]
    enum PK {
        Add,
        Ld,
        LdPi,
    }
    let pks: [(&str, &str, PK); 3] = [
        ("add", "r2 = add(r0, r1)", PK::Add),
        ("ldw", "r2 = memw(r21+#0)", PK::Ld),
        ("ldpi", "r2 = memw(r21++#4)", PK::LdPi),
    ];
    let vals: [u32; 7] = [0, 1, 7, 8, 0xffff_ffff, 0x8000_0000, 5];
    for (pn, ptext, pk) in pks {
        for jc in jconds() {
            for neg in [false, true] {
                for hint in [":t", ":nt"] {
                    let cond = jc.text.replace("{n}", "r2.new");
                    let cond = if neg { format!("!{cond}") } else { cond };
                    // Both source orders: jump after and before the producer.
                    for first_jump in [false, true] {
                        let jump = format!("if ({cond}) jump{hint} 1f");
                        let packet = if first_jump {
                            format!("{{ {jump}; {ptext} }}")
                        } else {
                            format!("{{ {ptext}; {jump} }}")
                        };
                        let body = format!("{packet}\n{{ r9 = #1 }}\n1:");
                        let mut case = Case::new(
                            format!("nvjump_{pn}_{}{}_{}_{}", if neg { "n" } else { "" }, jc.name,
                                &hint[1..], if first_jump { "jf" } else { "pf" }),
                            body,
                        );
                        for &val in &vals {
                            for other in [val, val.wrapping_add(1), val.wrapping_sub(1), 0] {
                                let mut v = Vector::new().set(3, other);
                                match pk {
                                    PK::Add => {
                                        v = v.set(0, val).set(1, 0);
                                    }
                                    PK::Ld => {
                                        v = v.set_win(21, BASE + 0x40).mem(WB + 0x40, &le32(val));
                                    }
                                    PK::LdPi => {
                                        v = v.set_win(21, BASE + 0x40).mem(WB + 0x40, &le32(val))
                                            .exp_win(21, BASE + 0x44);
                                    }
                                }
                                v = v.exp(2, val);
                                let t = (jc.eval)(val, other) != neg;
                                if !t {
                                    v = v.exp(9, 1);
                                }
                                case = case.vec(v);
                            }
                        }
                        out.push(case);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Predicate producers consumed with .new
// ---------------------------------------------------------------------------

/// A predicate-producing instruction.  The logical predicate operations
/// (`and`/`or`/`xor`/`not`/...) act bitwise on the whole 8-bit predicates;
/// compares produce 0xff or 0x00; `vcmp` produces a per-lane mask.  A
/// consumer tests bit 0.
struct PProd {
    name: &'static str,
    text: &'static str,
    /// Value of the produced p0 byte from r0..r3 and the input P bytes.
    eval: fn(&[u32; 4], &[u8; 4]) -> u8,
}

fn b(x: bool) -> u8 {
    if x {
        0xff
    } else {
        0
    }
}

fn pprods() -> Vec<PProd> {
    vec![
        PProd { name: "eq", text: "p0 = cmp.eq(r0, r1)", eval: |r, _| b(r[0] == r[1]) },
        PProd { name: "neq", text: "p0 = !cmp.eq(r0, r1)", eval: |r, _| b(r[0] != r[1]) },
        PProd { name: "gt", text: "p0 = cmp.gt(r0, r1)", eval: |r, _| b((r[0] as i32) > (r[1] as i32)) },
        PProd { name: "ngt", text: "p0 = !cmp.gt(r0, r1)", eval: |r, _| b((r[0] as i32) <= (r[1] as i32)) },
        PProd { name: "gtu", text: "p0 = cmp.gtu(r0, r1)", eval: |r, _| b(r[0] > r[1]) },
        PProd { name: "eq_i", text: "p0 = cmp.eq(r0, #7)", eval: |r, _| b(r[0] == 7) },
        PProd { name: "gt_i", text: "p0 = cmp.gt(r0, #7)", eval: |r, _| b((r[0] as i32) > 7) },
        PProd { name: "gtu_i", text: "p0 = cmp.gtu(r0, #7)", eval: |r, _| b(r[0] > 7) },
        PProd { name: "tstbit", text: "p0 = tstbit(r0, #3)", eval: |r, _| b(r[0] & 8 != 0) },
        PProd { name: "bitsset", text: "p0 = bitsset(r0, r1)", eval: |r, _| b(r[0] & r[1] == r[1]) },
        PProd { name: "bitsclr", text: "p0 = bitsclr(r0, r1)", eval: |r, _| b(r[0] & r[1] == 0) },
        PProd { name: "bitsclr_i", text: "p0 = bitsclr(r0, #5)", eval: |r, _| b(r[0] & 5 == 0) },
        PProd { name: "eq64", text: "p0 = cmp.eq(r1:0, r3:2)",
            eval: |r, _| b(r[0] == r[2] && r[1] == r[3]) },
        PProd { name: "gt64", text: "p0 = cmp.gt(r1:0, r3:2)",
            eval: |r, _| b((((r[1] as u64) << 32 | r[0] as u64) as i64) > (((r[3] as u64) << 32 | r[2] as u64) as i64)) },
        PProd { name: "gtu64", text: "p0 = cmp.gtu(r1:0, r3:2)",
            eval: |r, _| b(((r[1] as u64) << 32 | r[0] as u64) > ((r[3] as u64) << 32 | r[2] as u64)) },
        PProd { name: "and", text: "p0 = and(p1, p2)", eval: |_, p| p[1] & p[2] },
        PProd { name: "or", text: "p0 = or(p1, p2)", eval: |_, p| p[1] | p[2] },
        PProd { name: "xor", text: "p0 = xor(p1, p2)", eval: |_, p| p[1] ^ p[2] },
        PProd { name: "not", text: "p0 = not(p1)", eval: |_, p| !p[1] },
        PProd { name: "and_n", text: "p0 = and(p1, !p2)", eval: |_, p| p[1] & !p[2] },
        PProd { name: "any8", text: "p0 = any8(p1)", eval: |_, p| b(p[1] != 0) },
        PProd { name: "all8", text: "p0 = all8(p1)", eval: |_, p| b(p[1] == 0xff) },
        PProd { name: "vcmpb", text: "p0 = vcmpb.eq(r1:0, r3:2)",
            eval: |r, _| {
                let a = (r[1] as u64) << 32 | r[0] as u64;
                let c = (r[3] as u64) << 32 | r[2] as u64;
                let mut m = 0u8;
                for i in 0..8 {
                    if (a >> (8 * i)) as u8 == (c >> (8 * i)) as u8 {
                        m |= 1 << i;
                    }
                }
                m
            } },
        PProd { name: "vcmph", text: "p0 = vcmph.gt(r1:0, r3:2)",
            eval: |r, _| {
                let a = (r[1] as u64) << 32 | r[0] as u64;
                let c = (r[3] as u64) << 32 | r[2] as u64;
                let mut m = 0u8;
                for i in 0..4 {
                    let x = (a >> (16 * i)) as u16 as i16;
                    let y = (c >> (16 * i)) as u16 as i16;
                    if x > y {
                        m |= 3 << (2 * i);
                    }
                }
                m
            } },
    ]
}

fn pnew_cases(out: &mut Vec<Case>) {
    let rsets: [[u32; 4]; 6] = [
        [7, 7, 7, 7],
        [8, 7, 0x100, 7],
        [0xffff_fff9, 8, 0x8000_0000, 1],
        [0x1122_3344, 0x1122_3344, 0x5566_7788, 0x1122_3344],
        [0xff00_ff00, 0x00ff_00ff, 0xff00_ff00, 0xff00_ff00],
        [0x0000_0005, 0x0000_0005, 0, 0],
    ];
    let psets: [[u8; 4]; 5] = [[0, 0, 0, 0], [0, 0xff, 0xff, 0], [0, 0xff, 0x00, 0xa5], [0x5a, 0xfe, 0x01, 0xff], [0, 0xf0, 0x3c, 1]];
    let cons: [(&str, &str); 3] = [
        ("alu", "if (p0.new) r4 = #1; if (!p0.new) r5 = #2"),
        ("st", "if (p0.new) memw(r20+#0) = r6"),
        ("ld", "if (!p0.new) r4 = memw(r20+#0)"),
    ];
    for pp in pprods() {
        for (cn, ctext) in cons {
            let mut case = Case::new(
                format!("pnew_{}_{}", pp.name, cn),
                format!("{{ {}; {} }}", pp.text, ctext),
            );
            for r in &rsets {
                for p in &psets {
                    let p0 = (pp.eval)(r, p);
                    let t = p0 & 1 != 0;
                    let pin = u32::from_le_bytes(*p);
                    let pout = (pin & !0xff) | p0 as u32;
                    let mut v = Vector::new().set_win(20, BASE).set(ST_P, pin).set(6, 0xabcd_0123);
                    for (i, &x) in r.iter().enumerate() {
                        v = v.set(i, x);
                    }
                    v = v.exp(ST_P, pout);
                    match cn {
                        "alu" => {
                            v = if t { v.exp(4, 1) } else { v.exp(5, 2) };
                        }
                        "st" => {
                            if t {
                                v = v.exp_mem(WB, &le32(0xabcd_0123));
                            }
                        }
                        _ => {
                            if !t {
                                v = v.exp(4, dval(WB, 4) as u32);
                            }
                        }
                    }
                    case = case.vec(v);
                }
            }
            out.push(case);
        }
    }
}

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    nv_store_cases(&mut out);
    nv_store_special(&mut out);
    jump_cases(&mut out);
    pnew_cases(&mut out);
    out
}
