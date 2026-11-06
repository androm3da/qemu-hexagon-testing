// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Addressing modes that depend on a second register (M0/M1, CS0/CS1, GP,
//! index registers), and what happens when the packet also writes it.
//!
//! Rules (reference simulator + packet semantics): the effective address and
//! the post-update of the base are computed from the registers as they were
//! before the packet, so a write to Mu, CSu, GP or an index register in the
//! same packet only affects later packets.  Circular addressing: with K = 0
//! and length >= 4 (M[16:0]), the buffer is [CS, CS+length); the access uses
//! the old pointer and the new pointer is `ptr + inc`, minus `length` if it
//! reaches the end, plus `length` if it falls below the start.  Bit-reversed
//! addressing accesses `brev16(ptr[15:0])` with the upper half unchanged and
//! then adds Mu to the (un-reversed) pointer.

use super::*;

const WB: usize = BASE as usize;

fn v() -> Vector {
    Vector::new()
}

fn mn(size: usize, ld: bool, signed: bool) -> &'static str {
    match (size, ld, signed) {
        (1, true, true) => "memb",
        (1, true, false) => "memub",
        (1, false, _) => "memb",
        (2, true, true) => "memh",
        (2, true, false) => "memuh",
        (2, false, _) => "memh",
        (4, ..) => "memw",
        (8, ..) => "memd",
        _ => unreachable!(),
    }
}

fn ext(val: u64, size: usize, signed: bool) -> u64 {
    match (size, signed) {
        (1, true) => val as u8 as i8 as i32 as u32 as u64,
        (2, true) => val as u16 as i16 as i32 as u32 as u64,
        _ => val,
    }
}

fn circ_add(ptr: u32, inc: i32, start: u32, len: u32) -> u32 {
    let end = start + len;
    let np = (ptr as i64 + inc as i64) as i64;
    let np = if np >= end as i64 {
        np - len as i64
    } else if np < start as i64 {
        np + len as i64
    } else {
        np
    };
    np as u32
}

fn rev16(x: u32) -> u32 {
    (x as u16).reverse_bits() as u32
}

fn circ_cases(out: &mut Vec<Case>) {
    // Buffer: 0x20 bytes at window offset 0x100.
    let start = 0x100u32;
    let len = 0x20u32;
    for size in [1usize, 2, 4, 8] {
        let scale = size as i32;
        // s4 immediate scaled by the access size.
        let incs: Vec<i32> = (-8..8).map(|i| i * scale).filter(|i| i.unsigned_abs() < len).collect();
        for ld in [true, false] {
            for &inc in &incs {
                let signed = size < 4;
                let insn = if ld {
                    let dst = if size == 8 { "r3:2" } else { "r2" };
                    format!("{dst} = {}(r20++#{inc}:circ(m0))", mn(size, true, signed))
                } else {
                    let src = if size == 8 { "r1:0" } else { "r0" };
                    format!("{}(r20++#{inc}:circ(m0)) = {src}", mn(size, false, false))
                };
                let mut case = Case::new(
                    format!("circ_{}{}_inc{:+}", if ld { "ld" } else { "st" }, size, inc),
                    format!("{{ {insn} }}"),
                );
                let mut p = start;
                while p < start + len {
                    let np = circ_add(p, inc, start, len);
                    let mut vv = v().set_win(20, p).set(ST_M0, len).set_win(ST_CS0, start)
                        .exp_win(20, np);
                    if ld {
                        let raw = dval(p as usize, size);
                        let val = ext(raw, size, signed);
                        vv = vv.exp(2, val as u32);
                        if size == 8 {
                            vv = vv.exp(3, (val >> 32) as u32);
                        }
                    } else {
                        let data: Vec<u8> = if size == 8 {
                            let x = ((d(1) as u64) << 32) | d(0) as u64;
                            x.to_le_bytes().to_vec()
                        } else {
                            d(0).to_le_bytes()[..size].to_vec()
                        };
                        vv = vv.exp_mem(p as usize, &data);
                    }
                    case = case.vec(vv);
                    p += size as u32;
                }
                out.push(case);
            }
        }
    }

    // Writes to M0/CS0 in the same packet affect only later packets.
    let cases: [(&str, &str); 2] = [
        ("m0_write", "{ m0 = r5; r2 = memw(r20++#4:circ(m0)) }"),
        ("cs0_write", "{ cs0 = r5; r2 = memw(r20++#4:circ(m0)) }"),
    ];
    for (name, body) in cases {
        let mut case = Case::new(format!("circ_{name}_same_packet"), body.to_string());
        for p in [start, start + 0x10, start + 0x1c] {
            let np = circ_add(p, 4, start, len);
            let mut vv = v().set_win(20, p).set(ST_M0, len).set_win(ST_CS0, start)
                .exp_win(20, np).exp(2, dval(p as usize, 4) as u32);
            // The new value written by the packet.
            if name == "m0_write" {
                vv = vv.set(5, 0x8).exp(ST_M0, 0x8);
            } else {
                vv = vv.set_win(5, 0x40).exp_win(ST_CS0, 0x40);
            }
            case = case.vec(vv);
        }
        out.push(case);
    }
    // Two circular accesses using different Mu/CSu in one packet.
    {
        let mut case = Case::new("circ_two_buffers_one_packet",
            "{ r2 = memw(r20++#4:circ(m0)); memw(r21++#-4:circ(m1)) = r0 }");
        let (s1, l1) = (0x140u32, 0x10u32);
        for (p0, p1) in [(0x11cu32, 0x140u32), (0x100, 0x14c), (0x108, 0x144)] {
            let np0 = circ_add(p0, 4, start, len);
            let np1 = circ_add(p1, -4, s1, l1);
            case = case.vec(v().set_win(20, p0).set_win(21, p1)
                .set(ST_M0, len).set_win(ST_CS0, start)
                .set(ST_M1, l1).set_win(ST_CS1, s1)
                .exp_win(20, np0).exp_win(21, np1)
                .exp(2, dval(p0 as usize, 4) as u32)
                .exp_mem(p1 as usize, &le32(d(0))));
        }
        out.push(case);
    }
}

fn brev_cases(out: &mut Vec<Case>) {
    let start = 0x100u32;
    for size in [1usize, 2, 4, 8] {
        // Step bits 11..13 (EA bits 4..2) for b/h/w, bits 10..12 (EA bits
        // 5..3) for d, so every access stays naturally aligned.
        let inc = if size == 8 { 0x400u32 } else { 0x800 };
        for ld in [true, false] {
            // Eight consecutive accesses, each into/out of its own register.
            let mut lines = Vec::new();
            for j in 0..8usize {
                let line = if ld {
                    let (dst, ..) = if size == 8 { (format!("r{}:{}", 9 + 2 * j, 8 + 2 * j), 0) } else { (format!("r{}", 8 + j), 0) };
                    format!("{{ {dst} = {}(r20++m1:brev) }}", mn(size, true, size < 4))
                } else {
                    let src = if size == 8 { format!("r{}:{}", 2 * j + 1, 2 * j) } else { format!("r{j}") };
                    format!("{{ {}(r20++m1:brev) = {src} }}", mn(size, false, false))
                };
                lines.push(line);
            }
            // d loads need 16 registers; keep to r8..r23 and avoid r20/r21.
            if ld && size == 8 {
                lines.clear();
                for j in 0..8usize {
                    // r1:0 .. r15:14 are free in this case.
                    lines.push(format!("{{ r{}:{} = memd(r20++m1:brev) }}", 2 * j + 1, 2 * j));
                }
            }
            let mut vv = v().set_brev(20, start).set(ST_M1, inc);
            let mut mem: Vec<u8> = (0..WIN_BYTES).map(def_mem_byte).collect();
            for j in 0..8usize {
                // The pointer register before access j holds dom(start) + j*inc.
                let dom = rev16(((j as u32) * inc) & 0xffff); // EA offset contribution
                let _ = dom;
                // EA = brev16(low16(dom0) + j*inc) with dom0 = rev16(base+start).
                // Offsets are base-independent because the window is 512-aligned
                // and the stepped bits are above bit 8 of EA... computed below.
            }
            // EA offset of step j: the stepped domain bits are the reversed
            // low EA bits (below the window's 512-byte alignment), so
            // offset_j = start + rev16(j * inc).
            for j in 0..8usize {
                let ea = start + rev16((j as u32) * inc);
                if ld {
                    let raw = dval(ea as usize, size);
                    let val = ext(raw, size, size < 4);
                    if size == 8 {
                        vv = vv.exp(2 * j, val as u32).exp(2 * j + 1, (val >> 32) as u32);
                    } else {
                        vv = vv.exp(8 + j, val as u32);
                    }
                } else if size == 8 {
                    let x = ((d(2 * j + 1) as u64) << 32) | d(2 * j) as u64;
                    for (k, b) in x.to_le_bytes().iter().enumerate() {
                        mem[ea as usize + k] = *b;
                    }
                } else {
                    for k in 0..size {
                        mem[ea as usize + k] = (d(j) >> (8 * k)) as u8;
                    }
                }
            }
            vv = vv.exp_brev(20, start, 8 * inc);
            if !ld {
                vv = vv.exp_window(&mem);
            }
            out.push(Case::new(
                format!("brev_{}{}", if ld { "ld" } else { "st" }, size),
                lines.join("\n"),
            ).vec(vv));
        }
    }
    // Writing M1 in the same packet: the access uses the old increment.
    out.push(Case::new("brev_m1_write_same_packet",
        "{ m1 = r5; r2 = memw(r20++m1:brev) }\n{ r3 = memw(r20++m1:brev) }")
        .vec({
            let start = 0x100u32;
            let inc_old = 0x2000u32;
            let inc_new = 0x4000u32;
            // First access at start with the old increment, then pointer
            // moves by inc_old; the second access uses the new increment.
            let ea1 = start + rev16(inc_old);
            v().set_brev(20, start).set(ST_M1, inc_old).set(5, inc_new)
                .exp(ST_M1, inc_new)
                .exp(2, dval(start as usize, 4) as u32)
                .exp(3, dval(ea1 as usize, 4) as u32)
                .exp_brev(20, start, inc_old + inc_new)
        }));
}

fn misc_cases(out: &mut Vec<Case>) {
    // Register post-increment with M0: old M0 is used.
    out.push(Case::new("postinc_m0_write_same_packet",
        "{ m0 = r5; r2 = memw(r20++m0) }")
        .vec(v().set_win(20, BASE).set(ST_M0, 8).set(5, 0x20)
            .exp(ST_M0, 0x20).exp_win(20, BASE + 8)
            .exp(2, dval(WB, 4) as u32)));
    out.push(Case::new("postinc_m1_store_write_same_packet",
        "{ m1 = r5; memw(r20++m1) = r0 }")
        .vec(v().set_win(20, BASE).set(ST_M1, 12).set(5, 0x20)
            .exp(ST_M1, 0x20).exp_win(20, BASE + 12)
            .exp_mem(WB, &le32(d(0)))));
    // GP-relative: old GP is the base.
    out.push(Case::new("gp_relative_load_gp_write_same_packet",
        "{ gp = r5; r2 = memw(gp+#8) }")
        .vec(v().set_win(ST_GP, BASE).set_win(5, BASE + 0x40)
            .exp_win(ST_GP, BASE + 0x40).exp(2, dval(WB + 8, 4) as u32)));
    out.push(Case::new("gp_relative_store_gp_write_same_packet",
        "{ gp = r5; memw(gp+#8) = r0 }")
        .vec(v().set_win(ST_GP, BASE).set_win(5, BASE + 0x40)
            .exp_win(ST_GP, BASE + 0x40).exp_mem(WB + 8, &le32(d(0)))));
    for (mnm, size) in [("memb", 1usize), ("memh", 2), ("memw", 4), ("memd", 8)] {
        let dst = if size == 8 { "r3:2" } else { "r2" };
        let raw = dval(WB + 16, size);
        let mut vv = v().set_win(ST_GP, BASE);
        if size == 8 {
            vv = vv.exp(2, raw as u32).exp(3, (raw >> 32) as u32);
        } else {
            vv = vv.exp(2, ext(raw, size, true) as u32);
        }
        out.push(Case::new(format!("gp_relative_ld_{mnm}"),
            format!("{{ {dst} = {mnm}(gp+#16) }}")).vec(vv));
    }
    // Index register written in the same packet: old value addresses.
    out.push(Case::new("indexed_load_index_write_same_packet",
        "{ r4 = #8; r2 = memw(r20+r4<<#2) }")
        .vec(v().set_win(20, BASE).set(4, 3).exp(4, 8)
            .exp(2, dval(WB + 12, 4) as u32)));
    out.push(Case::new("indexed_store_index_write_same_packet",
        "{ r4 = #8; memw(r20+r4<<#2) = r0 }")
        .vec(v().set_win(20, BASE).set(4, 3).exp(4, 8)
            .exp_mem(WB + 12, &le32(d(0)))));
    // Absolute-set load writes the base register; others read the old one.
    out.push(Case::new("abs_set_load_base_read_same_packet",
        "{ r2 = memw(r20=##PKT_WINDOW+256); r3 = r20 }")
        .vec(v().set_win(20, BASE).exp_win(20, 256).exp(2, dval(256, 4) as u32)
            .exp_win(3, BASE)));
    out.push(Case::new("abs_plus_index_load",
        "{ r4 = #1; r2 = memw(r5<<#2+##PKT_WINDOW+256) }")
        .vec(v().set(5, 3).set(4, 0).exp(4, 1).exp(2, dval(256 + 12, 4) as u32)));
}

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    circ_cases(&mut out);
    brev_cases(&mut out);
    misc_cases(&mut out);
    out
}
