// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Two memory accesses in one packet whose addresses overlap.
//!
//! Architectural rule (memory-ordering chapter, "thread as an observer"; and
//! hexagon-sim observed): the two accesses of a packet are unordered unless
//! they overlap and at least one is a store, in which case the slot 1 access
//! happens first and the slot 0 access second - so a slot 0 load sees the
//! bytes of an overlapping slot 1 store, while a slot 1 load never sees a
//! slot 0 store.  Which instruction lands in which slot is the assembler's
//! choice: without `:mem_noshuf` it puts a load before (slot 1) a store
//! regardless of source order, memops (slot 0 only) go last, and otherwise
//! source order decides (first in source = slot 1).  `:mem_noshuf` makes
//! source order decisive.  The model below executes the two accesses
//! sequentially in that order, byte by byte; hexagon-sim confirms it.
//!
//! The matrix is every (kind, size) of op A at a fixed address against every
//! (kind, size) of op B at every naturally aligned address around it
//! (before, overlapping every byte, after), with and without `:mem_noshuf`,
//! and with four addressing forms.

use super::*;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Ld,
    St,
    Memop,
}

#[derive(Clone, Copy)]
struct Op {
    kind: Kind,
    size: usize,
    /// Window byte offset accessed.
    addr: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Separate base registers, `+#0`.
    Sep,
    /// One base register, different immediates.
    Shared,
    /// Separate base registers, post-increment by the access size.
    PostInc,
    /// Absolute addresses (`##sym+off`).
    Abs,
}

const A_ADDR: usize = 0x80;
// Store data / load destination / memop operand registers.
const A_DATA: usize = 0; // r0 (r1:0)
const B_DATA: usize = 2; // r2 (r3:2)
const A_DST: usize = 4; // r4 (r5:4)
const B_DST: usize = 6; // r6 (r7:6)
const MEMOP_OPERAND: usize = 8;
const A_BASE: usize = 20;
const B_BASE: usize = 21;

const DATA: [u32; 9] = [
    0xA1B2_C3D4,
    0xE5F6_0718,
    0x2B3C_4D5E,
    0x6F70_8192,
    0,
    0,
    0,
    0,
    0x8F9F_AFCF,
];

fn mem_name(size: usize, signed: bool, ld: bool) -> &'static str {
    match (size, signed && ld) {
        (1, true) => "memb",
        (1, false) => {
            if ld {
                "memub"
            } else {
                "memb"
            }
        }
        (2, true) => "memh",
        (2, false) => {
            if ld {
                "memuh"
            } else {
                "memh"
            }
        }
        (4, _) => "memw",
        (8, _) => "memd",
        _ => unreachable!(),
    }
}

/// Address operand text for an op.
fn addr_text(op: &Op, is_a: bool, mode: Mode) -> String {
    let base = if is_a { A_BASE } else { B_BASE };
    match mode {
        Mode::Sep => format!("(r{base}+#0)"),
        Mode::Shared => format!("(r{A_BASE}+#{})", op.addr as i32 - A_ADDR as i32),
        Mode::PostInc if op.kind != Kind::Memop => format!("(r{base}++#{})", op.size),
        Mode::PostInc => format!("(r{base}+#0)"),
        Mode::Abs if op.kind != Kind::Memop => format!("(##PKT_WINDOW+{})", op.addr),
        Mode::Abs => format!("(r{base}+#0)"),
    }
}

fn insn(op: &Op, is_a: bool, mode: Mode) -> String {
    let data = if is_a { A_DATA } else { B_DATA };
    let dst = if is_a { A_DST } else { B_DST };
    let a = addr_text(op, is_a, mode);
    match op.kind {
        Kind::Ld => {
            let m = mem_name(op.size, is_a, true);
            if op.size == 8 {
                format!("r{}:{} = {m}{a}", dst + 1, dst)
            } else {
                format!("r{dst} = {m}{a}")
            }
        }
        Kind::St => {
            let m = mem_name(op.size, true, false);
            if op.size == 8 {
                format!("{m}{a} = r{}:{}", data + 1, data)
            } else {
                format!("{m}{a} = r{data}")
            }
        }
        Kind::Memop => {
            let m = mem_name(op.size, true, false);
            format!("{m}{a} += r{MEMOP_OPERAND}")
        }
    }
}

fn read_le(mem: &[u8], addr: usize, n: usize) -> u64 {
    let mut v = 0u64;
    for i in 0..n {
        v |= (mem[addr + i] as u64) << (8 * i);
    }
    v
}

fn write_le(mem: &mut [u8], addr: usize, n: usize, v: u64) {
    for i in 0..n {
        mem[addr + i] = (v >> (8 * i)) as u8;
    }
}

fn extend(v: u64, size: usize, signed: bool) -> u64 {
    match (size, signed) {
        (1, true) => v as u8 as i8 as i64 as u64 & 0xffff_ffff,
        (2, true) => v as u16 as i16 as i64 as u64 & 0xffff_ffff,
        _ => v,
    }
}

fn data_reg_val(idx: usize, size: usize) -> u64 {
    if size == 8 {
        ((DATA[idx + 1] as u64) << 32) | DATA[idx] as u64
    } else {
        DATA[idx] as u64
    }
}

/// Whether A (first in source) is executed before B (i.e. is in slot 1).
fn a_first(a: &Op, b: &Op, noshuf: bool) -> bool {
    if b.kind == Kind::Memop {
        return true;
    }
    if a.kind == Kind::Memop {
        return false;
    }
    if noshuf {
        return true;
    }
    // Without :mem_noshuf a load is placed ahead of a store.
    !(a.kind == Kind::St && b.kind == Kind::Ld)
}

fn build_vector(a: &Op, b: &Op, noshuf: bool, mode: Mode) -> Vector {
    let mut v = Vector::new();
    for (i, &x) in DATA.iter().enumerate() {
        if x != 0 {
            v = v.set(i, x);
        }
    }
    v = v.set_win(A_BASE, if mode == Mode::Shared || mode == Mode::Sep || mode == Mode::PostInc || mode == Mode::Abs { a.addr as u32 } else { 0 });
    if mode != Mode::Shared {
        v = v.set_win(B_BASE, b.addr as u32);
    }

    let mut mem: Vec<u8> = (0..WIN_BYTES).map(def_mem_byte).collect();
    let mut regs: Vec<(usize, u32)> = Vec::new();
    let first_a = a_first(a, b, noshuf);
    let order: [(&Op, bool); 2] = if first_a { [(a, true), (b, false)] } else { [(b, false), (a, true)] };
    for (op, is_a) in order {
        match op.kind {
            Kind::Ld => {
                let val = extend(read_le(&mem, op.addr, op.size), op.size, is_a);
                let dst = if is_a { A_DST } else { B_DST };
                regs.push((dst, val as u32));
                if op.size == 8 {
                    regs.push((dst + 1, (val >> 32) as u32));
                }
            }
            Kind::St => {
                let data = if is_a { A_DATA } else { B_DATA };
                write_le(&mut mem, op.addr, op.size, data_reg_val(data, op.size));
            }
            Kind::Memop => {
                let old = read_le(&mem, op.addr, op.size);
                let new = old.wrapping_add(DATA[MEMOP_OPERAND] as u64);
                write_le(&mut mem, op.addr, op.size, new);
            }
        }
    }
    for (r, x) in regs {
        v = v.exp(r, x);
    }
    if mode == Mode::PostInc {
        if a.kind != Kind::Memop {
            v = v.exp_win(A_BASE, (a.addr + a.size) as u32);
        }
        if b.kind != Kind::Memop {
            v = v.exp_win(B_BASE, (b.addr + b.size) as u32);
        }
    }
    v.exp_window(&mem)
}

fn tag(k: Kind) -> &'static str {
    match k {
        Kind::Ld => "ld",
        Kind::St => "st",
        Kind::Memop => "memop",
    }
}

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    let kinds = [Kind::Ld, Kind::St, Kind::Memop];
    for &ka in &kinds {
        for &kb in &kinds {
            // A memop is a slot-0-only store, so it can only share a packet
            // with a load.
            let memop_ok = |x: Kind, y: Kind| x != Kind::Memop || y == Kind::Ld;
            if !memop_ok(ka, kb) || !memop_ok(kb, ka) {
                continue;
            }
            let sizes = |k: Kind| -> &'static [usize] {
                if k == Kind::Memop {
                    &[1, 2, 4]
                } else {
                    &[1, 2, 4, 8]
                }
            };
            for &sa in sizes(ka) {
                for &sb in sizes(kb) {
                    let a = Op { kind: ka, size: sa, addr: A_ADDR };
                    let mut pos = 0x78;
                    while pos + sb <= 0x90 {
                        let b = Op { kind: kb, size: sb, addr: pos };
                        for noshuf in [false, true] {
                            // Under :mem_noshuf source order is slot order
                            // (first = slot 1), and a memop needs slot 0.
                            if noshuf && ka == Kind::Memop {
                                continue;
                            }
                            for (mi, mode) in
                                [Mode::Sep, Mode::Shared, Mode::PostInc, Mode::Abs].iter().enumerate()
                            {
                                let ia = insn(&a, true, *mode);
                                let ib = insn(&b, false, *mode);
                                let sfx = if noshuf { ":mem_noshuf" } else { "" };
                                let body = format!("{{ {ia}; {ib} }}{sfx}");
                                let name = format!(
                                    "{}{}_{}{}_@{:+}_{}_m{}",
                                    tag(ka),
                                    sa,
                                    tag(kb),
                                    sb,
                                    pos as i32 - A_ADDR as i32,
                                    if noshuf { "noshuf" } else { "shuf" },
                                    mi
                                );
                                out.push(
                                    Case::new(name, body).vec(build_vector(&a, &b, noshuf, *mode)),
                                );
                            }
                        }
                        pos += sb;
                    }
                }
            }
        }
    }
    out
}
