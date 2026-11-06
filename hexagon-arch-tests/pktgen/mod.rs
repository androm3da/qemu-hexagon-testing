// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Host-side generator for the table-driven packet-semantics tests.
//!
//! build.rs calls `suites()` and, for each `Suite`, writes
//!
//!   * `<suite>.gen.S`  - one function per case containing the packet text
//!     (assembled later by the SDK assembler), plus the `pkt_fns` address
//!     table, and
//!   * `<suite>.tbl`    - the binary case table read by src/pkt/mod.rs.
//!
//! A `Case` is a body of assembly plus one or more input `Vector`s.  A
//! vector lists only the machine state that differs from the defaults in
//! src/pkt/defaults.rs, and only the state that is expected to differ from
//! the input afterwards.  Everything else is checked to be unchanged.
//!
//! Expected values are always derived from the architectural rule that the
//! case is testing (cited in the case's comments) and validated against
//! hexagon-sim, never from QEMU's implementation.

#![allow(dead_code)]

#[path = "../src/pkt/defaults.rs"]
mod defaults;
pub use defaults::*;

pub mod addrmode;
pub mod alias;
pub mod cof;
pub mod newval;
pub mod pred;
pub mod excp_prio;
pub mod hlx;
pub mod hmx;
pub mod hvx;
pub mod hvx_mem;
pub mod priv_;
pub mod regs;
pub mod replay_x;
pub mod replay_rw;
pub mod shuffle;
pub mod usr;

use std::collections::BTreeMap;

/// A value that is either absolute or an offset from the window base.
#[derive(Clone, Copy, Debug)]
pub enum Val {
    Abs(u32),
    Win(u32),
    /// Do not check this slot (used for code addresses).
    Ignore,
    /// Offset from the VTCM base.
    Vt(u32),
    /// Bit-reversed-domain form of a window address: the lower 16 bits of
    /// `window_base + off` bit-reversed, upper bits unchanged, then `add`
    /// added (32-bit).  This is the register value a `:brev` access with
    /// effective address `window_base + off` needs.
    Brev { off: u32, add: u32 },
}

impl From<u32> for Val {
    fn from(v: u32) -> Val {
        Val::Abs(v)
    }
}

/// Which handler an expected exception event comes from.
#[derive(Clone, Copy, Debug)]
pub enum EvKind {
    /// Data TLB miss (read: cause 0x70, write: 0x71).
    Rw = 1,
    /// Instruction TLB miss.
    X = 2,
    /// Precise error.
    Err = 3,
}

/// An exception the packet is expected to take (in order).
#[derive(Clone, Copy, Debug)]
pub struct Ev {
    pub kind: EvKind,
    pub cause: u32,
    /// Expected SSR[BVS, V1, V0] as (v0, v1, bvs), or None to leave unchecked.
    pub flags: Option<(bool, bool, bool)>,
    /// Expected BADVA0 / BADVA1 as window offsets, or None for unchecked
    /// (e.g. "unchanged").
    pub badva0: Option<u32>,
    pub badva1: Option<u32>,
    pub badva0_unchanged: bool,
    pub badva1_unchanged: bool,
}

impl Ev {
    pub fn new(kind: EvKind, cause: u32) -> Ev {
        Ev { kind, cause, flags: None, badva0: None, badva1: None, badva0_unchanged: false, badva1_unchanged: false }
    }
    pub fn flags(mut self, v0: bool, v1: bool, bvs: bool) -> Ev {
        self.flags = Some((v0, v1, bvs));
        self
    }
    pub fn va0(mut self, off: u32) -> Ev {
        self.badva0 = Some(off);
        self
    }
    pub fn va1(mut self, off: u32) -> Ev {
        self.badva1 = Some(off);
        self
    }
    /// BADVA0 holds `off`, asserted only with `PKT_STRICT` (the architectural
    /// value that hexagon-sim leaves stale).
    pub fn va0_strict(self, off: u32) -> Ev {
        if strict() {
            self.va0(off)
        } else {
            self
        }
    }
    /// BADVA1 holds `off`, asserted only with `PKT_STRICT`.
    pub fn va1_strict(self, off: u32) -> Ev {
        if strict() {
            self.va1(off)
        } else {
            self
        }
    }
    /// BADVA0 keeps the value it had before the exception.
    pub fn va0_unchanged(mut self) -> Ev {
        self.badva0_unchanged = true;
        self
    }
    /// BADVA1 keeps the value it had before the exception.
    pub fn va1_unchanged(mut self) -> Ev {
        self.badva1_unchanged = true;
        self
    }
}

/// `PKT_STRICT=1` asserts the architectural behaviour that hexagon-sim
/// 21.0.04 is known to deviate from (see sim_bugs_nov5/).  Those assertions
/// are off by default so the suite passes on the reference simulator.
pub fn strict() -> bool {
    std::env::var("PKT_STRICT").is_ok()
}

pub const NONE: u32 = 0xffff_ffff;
/// Bytes the case wrapper (PKT_CASE_BEGIN) occupies ahead of the body: two
/// packets of two and one words.
pub const PROLOGUE: u32 = 12;
pub const MODE_DATA_REPLAY: u32 = 1;
pub const MODE_CODE_REPLAY: u32 = 2;
pub const MODE_RESUME_AT_END: u32 = 4;
/// Do not assert that memory is untouched at each exception (a known
/// hexagon-sim deviation from the spec for some packets).
pub const MODE_NO_PARTIAL_CHECK: u32 = 8;
pub const MODE_HVX: u32 = 16;
pub const MODE_RESTORE_ENABLES: u32 = 32;
pub const MODE_VTCM: u32 = 64;

/// One input/expected-output pair for a case.
#[derive(Clone, Default)]
pub struct Vector {
    ins: BTreeMap<u32, Val>,
    outs: BTreeMap<u32, Val>,
    in_mem: Vec<(usize, Vec<u8>)>,
    out_mem: Vec<(usize, Vec<u8>)>,
    ignore_mem: Vec<usize>,
    events: Vec<Ev>,
    /// HVX registers: 0..31 = v0..v31, 32..35 = q0..q3 (as 0xff/0x00 bytes).
    vin: BTreeMap<u32, Vec<u8>>,
    vout: BTreeMap<u32, Vec<u8>>,
    vignore: Vec<u32>,
    vt_in: Vec<(usize, Vec<u8>)>,
    vt_out: Vec<(usize, Vec<u8>)>,
    vt_ignore: Vec<usize>,
}

impl Vector {
    pub fn new() -> Vector {
        Vector::default()
    }

    /// Input value of state slot `id` (register number or `ST_*`).  The slot
    /// is expected to be unchanged unless `exp` says otherwise.
    pub fn set(mut self, id: usize, v: u32) -> Vector {
        self.ins.insert(id as u32, Val::Abs(v));
        self
    }

    /// Input register `id` points `off` bytes into the memory window.
    pub fn set_win(mut self, id: usize, off: u32) -> Vector {
        self.ins.insert(id as u32, Val::Win(off));
        self
    }

    /// Expect state slot `id` to hold `v` after the case.
    pub fn exp(mut self, id: usize, v: u32) -> Vector {
        self.outs.insert(id as u32, Val::Abs(v));
        self
    }

    /// Do not check state slot `id` (it holds a code address).
    pub fn ignore(mut self, id: usize) -> Vector {
        self.outs.insert(id as u32, Val::Ignore);
        self
    }

    /// Input register `id` is the bit-reversed-domain pointer for window
    /// offset `off`.
    pub fn set_brev(mut self, id: usize, off: u32) -> Vector {
        self.ins.insert(id as u32, Val::Brev { off, add: 0 });
        self
    }

    /// Expect register `id` to be the bit-reversed-domain pointer for window
    /// offset `off` plus `add`.
    pub fn exp_brev(mut self, id: usize, off: u32, add: u32) -> Vector {
        self.outs.insert(id as u32, Val::Brev { off, add });
        self
    }

    /// Expect register `id` to point `off` bytes into the window.
    pub fn exp_win(mut self, id: usize, off: u32) -> Vector {
        self.outs.insert(id as u32, Val::Win(off));
        self
    }

    /// Input register `id` points `off` bytes into the VTCM.
    pub fn set_vt(mut self, id: usize, off: u32) -> Vector {
        self.ins.insert(id as u32, Val::Vt(off));
        self
    }

    /// Expect register `id` to point `off` bytes into the VTCM.
    pub fn exp_vt(mut self, id: usize, off: u32) -> Vector {
        self.outs.insert(id as u32, Val::Vt(off));
        self
    }

    /// Initial VTCM bytes at byte offset `off`.
    pub fn vt_mem(mut self, off: usize, bytes: &[u8]) -> Vector {
        self.vt_in.push((off, bytes.to_vec()));
        self
    }

    /// Expected VTCM bytes at byte offset `off` after the case.
    pub fn exp_vt_mem(mut self, off: usize, bytes: &[u8]) -> Vector {
        self.vt_out.push((off, bytes.to_vec()));
        self
    }

    /// Do not check VTCM bytes `off..off+len` after the case.
    pub fn ignore_vt(mut self, off: usize, len: usize) -> Vector {
        if strict() {
            return self;
        }
        let mut w = off / 4;
        while w * 4 < off + len {
            self.vt_ignore.push(w);
            w += 1;
        }
        self
    }

    /// Initial window bytes at byte offset `off`.
    pub fn mem(mut self, off: usize, bytes: &[u8]) -> Vector {
        self.in_mem.push((off, bytes.to_vec()));
        self
    }

    /// Expected window bytes at byte offset `off` after the case.
    pub fn exp_mem(mut self, off: usize, bytes: &[u8]) -> Vector {
        self.out_mem.push((off, bytes.to_vec()));
        self
    }

    /// Do not check the window bytes `off..off+len` after the case (they are
    /// rounded out to whole words).
    pub fn ignore_mem(mut self, off: usize, len: usize) -> Vector {
        if strict() {
            return self;
        }
        let mut w = off / 4;
        while w * 4 < off + len {
            self.ignore_mem.push(w);
            w += 1;
        }
        self
    }

    /// Input value of vector register `n` (128 bytes).
    pub fn setv(mut self, n: usize, bytes: &[u8]) -> Vector {
        assert_eq!(bytes.len(), VLEN);
        self.vin.insert(n as u32, bytes.to_vec());
        self
    }

    /// Expected value of vector register `n` after the case.
    pub fn expv(mut self, n: usize, bytes: &[u8]) -> Vector {
        assert_eq!(bytes.len(), VLEN);
        self.vout.insert(n as u32, bytes.to_vec());
        self
    }

    /// Do not check vector register `n` after the case.
    pub fn ignorev(mut self, n: usize) -> Vector {
        if strict() {
            return self;
        }
        self.vignore.push(n as u32);
        self
    }

    /// Input value of predicate register `n`: one bool per vector byte.
    pub fn setq(mut self, n: usize, bits: &[bool]) -> Vector {
        assert_eq!(bits.len(), VLEN);
        self.vin.insert(32 + n as u32, bits.iter().map(|&b| if b { 0xff } else { 0 }).collect());
        self
    }

    /// Expected predicate register `n` after the case.
    pub fn expq(mut self, n: usize, bits: &[bool]) -> Vector {
        assert_eq!(bits.len(), VLEN);
        self.vout.insert(32 + n as u32, bits.iter().map(|&b| if b { 0xff } else { 0 }).collect());
        self
    }

    /// Expect this exception next (in order).
    pub fn ev(mut self, e: Ev) -> Vector {
        self.events.push(e);
        self
    }

    /// Build the full initial window image (defaults plus `mem`).
    pub fn in_window(&self) -> Vec<u8> {
        let mut w: Vec<u8> = (0..WIN_BYTES).map(def_mem_byte).collect();
        for (off, b) in &self.in_mem {
            w[*off..*off + b.len()].copy_from_slice(b);
        }
        w
    }

    /// Replace the entire expected window image.
    pub fn exp_window(mut self, img: &[u8]) -> Vector {
        assert_eq!(img.len(), WIN_BYTES);
        self.out_mem.clear();
        self.out_mem.push((0, img.to_vec()));
        self
    }

    fn expected_window(&self) -> Vec<u8> {
        let mut w = self.in_window();
        for (off, b) in &self.out_mem {
            w[*off..*off + b.len()].copy_from_slice(b);
        }
        w
    }
}

/// One packet sequence plus the vectors it is run with.
pub struct Case {
    pub name: String,
    pub body: String,
    pub vecs: Vec<Vector>,
    /// `MODE_*` bits.
    pub mode: u32,
    /// Replay pages (bit i = page i) that start out unmapped.
    pub unmapped: u32,
    /// Number of misses of a page before the handler maps it.
    pub miss_before_map: u32,
    /// Expected ELR of every event, as a byte offset from the function start.
    pub elr_off: u32,
}

impl Case {
    pub fn new(name: impl Into<String>, body: impl Into<String>) -> Case {
        Case {
            name: name.into(),
            body: body.into(),
            vecs: Vec::new(),
            mode: 0,
            unmapped: 0,
            miss_before_map: 1,
            elr_off: NONE,
        }
    }
    /// Run in the page-straddling replay window with `unmapped` pages
    /// initially absent from the TLB, mapped on the `mbm`th miss.
    pub fn data_replay(mut self, unmapped: u32, mbm: u32) -> Case {
        self.mode |= MODE_DATA_REPLAY;
        self.unmapped = unmapped;
        self.miss_before_map = mbm;
        self
    }
    /// Copy the case across a page boundary at every word offset, with the
    /// `unmapped` code pages absent, mapped on the `mbm`th miss.
    pub fn code_replay(mut self, unmapped: u32, mbm: u32) -> Case {
        self.mode |= MODE_CODE_REPLAY;
        self.unmapped = unmapped;
        self.miss_before_map = mbm;
        self
    }
    /// The case uses the VTCM window (mapped, compared after the case).
    pub fn vtcm(mut self) -> Case {
        self.mode |= MODE_VTCM;
        self
    }
    /// The case clears coprocessor enables; the exception hooks (and the
    /// harness afterwards) put SSR[XE,XA,XE2] and CCR back.
    pub fn restore_enables(mut self) -> Case {
        self.mode |= MODE_RESTORE_ENABLES;
        self
    }
    /// The case uses HVX state (v0..v31, q0..q3 are loaded and compared).
    pub fn hvx(mut self) -> Case {
        self.mode |= MODE_HVX;
        self
    }
    /// Skip the memory-untouched-at-exception check for this case.
    pub fn no_partial_check(mut self) -> Case {
        if !strict() {
            self.mode |= MODE_NO_PARTIAL_CHECK;
        }
        self
    }
    /// Precise errors resume at the case epilogue.
    pub fn resume_at_end(mut self) -> Case {
        self.mode |= MODE_RESUME_AT_END;
        self
    }
    /// Expect every event's ELR at `off` bytes into the case function.
    pub fn elr_at(mut self, off: u32) -> Case {
        self.elr_off = off;
        self
    }
    pub fn vec(mut self, v: Vector) -> Case {
        self.vecs.push(v);
        self
    }
}

pub struct Suite {
    /// Test binary name, e.g. "test_pkt_regs".
    pub name: &'static str,
    pub cases: Vec<Case>,
}

/// All table-driven suites.
pub fn suites() -> Vec<Suite> {
    vec![
        Suite { name: "test_pkt_regs", cases: regs::cases() },
        Suite { name: "test_pkt_newval", cases: newval::cases() },
        Suite { name: "test_pkt_pred", cases: pred::cases() },
        Suite { name: "test_pkt_mem_alias", cases: alias::cases() },
        Suite { name: "test_pkt_cof", cases: cof::cases() },
        Suite { name: "test_pkt_shuffle", cases: shuffle::cases() },
        Suite { name: "test_pkt_usr", cases: usr::cases() },
        Suite { name: "test_pkt_addrmode", cases: addrmode::cases() },
        Suite { name: "test_pkt_replay_rw", cases: replay_rw::cases() },
        Suite { name: "test_pkt_replay_x", cases: replay_x::cases() },
        Suite { name: "test_pkt_excp_prio", cases: excp_prio::cases() },
        Suite { name: "test_pkt_priv", cases: priv_::cases() },
        Suite { name: "test_pkt_hlx", cases: hlx::cases() },
        Suite { name: "test_pkt_hmx", cases: hmx::cases() },
        Suite { name: "test_pkt_hvx", cases: hvx::cases() },
        Suite { name: "test_pkt_hvx_mem", cases: hvx_mem::cases() },
    ]
}

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn push_str(out: &mut Vec<u8>, s: &str) {
    assert!(s.is_ascii());
    push_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn enc_val(id: u32, v: Val) -> (u32, u32) {
    match v {
        Val::Abs(x) => (id, x),
        Val::Win(x) => (id | WIN_REL, x),
        Val::Vt(x) => (id | VT_REL, x),
        Val::Ignore => (id | IGNORE, 0),
        Val::Brev { off, add } => {
            assert!(off < 0x10000 && add % 0x200 == 0 && (add >> 9) < 0x10000);
            (id | BREV, off | ((add >> 9) << 16))
        }
    }
}

/// One-line rendering of a body for failure messages.
fn one_line(body: &str) -> String {
    let mut parts = Vec::new();
    for l in body.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with("//") || l.starts_with('#') {
            continue;
        }
        parts.push(l.to_string());
    }
    parts.join(" ")
}

/// Serialise a suite as (assembly source, binary table).
pub fn emit(suite: &Suite) -> (String, Vec<u8>) {
    let mut s = String::new();
    s.push_str("/* Generated by pktgen; do not edit. */\n");
    s.push_str("#include \"pkt_common.h\"\n\n    .text\n");
    for (i, c) in suite.cases.iter().enumerate() {
        let sym = format!("pkt_case_{i}");
        s.push_str(&format!("    /* {} */\n", c.name));
        s.push_str(&format!("    PKT_CASE_BEGIN {sym}\n"));
        for l in c.body.lines() {
            s.push_str("    ");
            s.push_str(l.trim_end());
            s.push('\n');
        }
        s.push_str(&format!("    PKT_CASE_END {sym}\n\n"));
    }
    s.push_str("    .section .rodata\n    .p2align 2\n    .globl pkt_fns\npkt_fns:\n");
    for i in 0..suite.cases.len() {
        s.push_str(&format!("    .word pkt_case_{i}\n"));
    }
    s.push_str("    .globl pkt_lens\npkt_lens:\n");
    for i in 0..suite.cases.len() {
        s.push_str(&format!("    .word pkt_case_{i}_end - pkt_case_{i}\n"));
    }

    let mut t = Vec::new();
    push_u32(&mut t, 0x504B_5432);
    push_u32(&mut t, suite.cases.len() as u32);
    for c in &suite.cases {
        assert!(!c.vecs.is_empty(), "case {} has no vectors", c.name);
        push_str(&mut t, &c.name);
        push_str(&mut t, &one_line(&c.body));
        push_u32(&mut t, c.mode);
        push_u32(&mut t, c.unmapped);
        push_u32(&mut t, c.miss_before_map);
        push_u32(&mut t, c.elr_off);
        push_u32(&mut t, c.vecs.len() as u32);
        for v in &c.vecs {
            let win_in = v.in_window();
            let win_out = v.expected_window();
            // Inputs: explicit state, then window words that differ from
            // the default pattern.
            let mut ins: Vec<(u32, u32)> = v.ins.iter().map(|(k, x)| enc_val(*k, *x)).collect();
            for w in 0..WIN_WORDS {
                let cur = word(&win_in, w);
                let dflt = default_word(w);
                if cur != dflt {
                    ins.push((MEM_ID_BASE + w as u32, cur));
                }
            }
            let vt_def: Vec<u8> = (0..VT_BYTES).map(def_vt_byte).collect();
            let mut vt_i = vt_def.clone();
            for (off, b) in &v.vt_in {
                vt_i[*off..*off + b.len()].copy_from_slice(b);
            }
            let mut vt_o = vt_i.clone();
            for (off, b) in &v.vt_out {
                vt_o[*off..*off + b.len()].copy_from_slice(b);
            }
            for w in 0..VT_WORDS {
                if word(&vt_i, w) != word(&vt_def, w) {
                    ins.push((VT_ID_BASE + w as u32, word(&vt_i, w)));
                }
            }
            let mut outs: Vec<(u32, u32)> = v.outs.iter().map(|(k, x)| enc_val(*k, *x)).collect();
            for w in 0..VT_WORDS {
                if word(&vt_o, w) != word(&vt_i, w) {
                    outs.push((VT_ID_BASE + w as u32, word(&vt_o, w)));
                }
            }
            for &w in &v.vt_ignore {
                outs.push((IGNORE | (VT_ID_BASE + w as u32), 0));
            }
            for w in 0..WIN_WORDS {
                let cur = word(&win_out, w);
                if cur != word(&win_in, w) {
                    outs.push((MEM_ID_BASE + w as u32, cur));
                }
            }
            push_u32(&mut t, ins.len() as u32);
            for (a, b) in ins {
                push_u32(&mut t, a);
                push_u32(&mut t, b);
            }
            push_u32(&mut t, v.vin.len() as u32);
            for (n, bytes) in &v.vin {
                push_u32(&mut t, *n);
                t.extend_from_slice(bytes);
            }
            for &w in &v.ignore_mem {
                outs.push((IGNORE | (MEM_ID_BASE + w as u32), 0));
            }
            // Mutation check: PKT_MUTATE=1 flips one bit of one expected value
            // per vector; every vector that has one must then fail.
            if std::env::var("PKT_MUTATE").is_ok() {
                if let Some(e) = outs.iter_mut().find(|(id, _)| id & (IGNORE | BREV | WIN_REL | VT_REL) == 0) {
                    e.1 ^= 1;
                }
            }
            push_u32(&mut t, outs.len() as u32);
            for (a, b) in outs {
                push_u32(&mut t, a);
                push_u32(&mut t, b);
            }
            push_u32(&mut t, (v.vout.len() + v.vignore.len()) as u32);
            for (n, bytes) in &v.vout {
                push_u32(&mut t, *n);
                t.extend_from_slice(bytes);
            }
            for n in &v.vignore {
                push_u32(&mut t, *n | IGNORE);
                t.extend_from_slice(&[0u8; VLEN]);
            }
            push_u32(&mut t, v.events.len() as u32);
            for e in &v.events {
                push_u32(&mut t, e.kind as u32);
                push_u32(&mut t, e.cause);
                let fl = match e.flags {
                    None => 0,
                    Some((v0, v1, bvs)) => 1 | ((v0 as u32) << 1) | ((v1 as u32) << 2) | ((bvs as u32) << 3),
                };
                push_u32(&mut t, fl);
                push_u32(&mut t, if e.badva0_unchanged { 0xffff_fffe } else { e.badva0.unwrap_or(NONE) });
                push_u32(&mut t, if e.badva1_unchanged { 0xffff_fffe } else { e.badva1.unwrap_or(NONE) });
                push_u32(&mut t, 0);
            }
        }
    }
    (s, t)
}

fn word(img: &[u8], w: usize) -> u32 {
    u32::from_le_bytes([img[w * 4], img[w * 4 + 1], img[w * 4 + 2], img[w * 4 + 3]])
}

fn default_word(w: usize) -> u32 {
    let b = w * 4;
    u32::from_le_bytes([
        def_mem_byte(b),
        def_mem_byte(b + 1),
        def_mem_byte(b + 2),
        def_mem_byte(b + 3),
    ])
}

/// Little-endian byte helpers for building expected memory images.
pub fn le32(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}
pub fn le64(v: u64) -> [u8; 8] {
    v.to_le_bytes()
}

/// Default value of general register `i` (what a case sees unless it sets it).
pub fn d(i: usize) -> u32 {
    def_state(i)
}

/// Default window bytes `off..off+len`.
pub fn dmem(off: usize, len: usize) -> Vec<u8> {
    (off..off + len).map(def_mem_byte).collect()
}

/// Default little-endian value of the `n`-byte (1,2,4,8) window item at `off`.
pub fn dval(off: usize, n: usize) -> u64 {
    let mut v = 0u64;
    for i in 0..n {
        v |= (def_mem_byte(off + i) as u64) << (8 * i);
    }
    v
}

/// Window offset used as the primary data area by most cases.  Registers are
/// pointed into the middle of the window so that offsets may go either way.
pub const BASE: u32 = 0x80;
