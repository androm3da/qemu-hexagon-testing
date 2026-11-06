// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Runner for the table-driven packet-semantics tests.
//!
//! The packets under test are assembled by the Hexagon SDK assembler (see
//! build.rs and pktgen/), not by rustc, so that every encoding comes from one
//! toolchain and HMX/HLX are available.  For each case the generator emits
//!
//!   * a function holding the packet(s) (symbol table `pkt_fns`), and
//!   * a record in a binary table (`include_bytes!`d by the test binary)
//!     naming the case and listing, per input vector, the state overrides
//!     and the expected state afterwards.
//!
//! `run_table` sets up the machine state for each vector, runs the case via
//! the `pkt_run` trampoline (asm/pkt/pkt_run.S), and compares *all* captured
//! state and the whole memory window with the expectation.

#![allow(dead_code)]

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, Ordering};
use hexagon_arch_tests::*;

mod defaults;
pub use defaults::*;
pub mod replay;
use replay::*;

/// Mirror of the layout in asm/pkt/pkt_common.h.  The trampoline uses 64-bit
/// accesses, so the structure must be 8-byte aligned.
#[repr(C, align(8))]
pub struct PktCtx {
    pub inp: [u32; NSTATE],
    pub out: [u32; NSTATE],
    pub func: u32,
    pub ret: u32,
    pub save: [u32; 10],
    /// Address of the HVX exchange buffer (pkt_run_hvx only).
    pub vptr: u32,
}

/// HVX exchange buffer: 128-byte slots; v0..v31 in, v0..v31 out, q0..q3 in,
/// q0..q3 out (see asm/pkt/pkt_run.S).
#[repr(C, align(128))]
pub struct HvxBuf(pub UnsafeCell<[u8; 72 * VLEN]>);
unsafe impl Sync for HvxBuf {}

#[no_mangle]
pub static PKT_HVX: HvxBuf = HvxBuf(UnsafeCell::new([0; 72 * VLEN]));

fn hvx_slot(i: usize) -> *mut u8 {
    unsafe { (PKT_HVX.0.get() as *mut u8).add(i * VLEN) }
}

#[repr(C, align(512))]
pub struct Window(pub UnsafeCell<[u8; WIN_BYTES]>);
unsafe impl Sync for Window {}

#[no_mangle]
pub static PKT_WINDOW: Window = Window(UnsafeCell::new([0; WIN_BYTES]));

extern "C" {
    fn pkt_run(ctx: *mut PktCtx);
    fn pkt_run_hvx(ctx: *mut PktCtx);
    /// Function addresses and byte lengths, one per case, emitted by the
    /// generator.
    static pkt_fns: [u32; 0];
    static pkt_lens: [u32; 0];
}

/// Case modes (bit set).
pub const MODE_DATA_REPLAY: u32 = 1;
pub const MODE_CODE_REPLAY: u32 = 2;
pub const MODE_RESUME_AT_END: u32 = 4;
pub const MODE_NO_PARTIAL_CHECK: u32 = 8;
pub const MODE_HVX: u32 = 16;
/// SSR[XE,XA,XE2] and CCR are put back by the exception hooks.
pub const MODE_RESTORE_ENABLES: u32 = 32;
/// The case uses the VTCM window.
pub const MODE_VTCM: u32 = 64;

/// Value of an unchecked expectation field.
pub const NONE: u32 = 0xffff_ffff;
/// Expectation: the BADVA register keeps the value it was primed with.
pub const UNCHANGED: u32 = 0xffff_fffe;

/// SSR.XE (31), SSR.XA (29:27), SSR.XE2 (26).
const SSR_XE_MASK: u32 = 0xfc00_0000 & !(1 << 30);

/// Size of the wrapper epilogue emitted by PKT_CASE_END (three packets).
const EPILOGUE_BYTES: u32 = 12;
/// Size of the wrapper prologue emitted by PKT_CASE_BEGIN (one packet).
const PROLOGUE_BYTES: u32 = 12;

/// Base address of the window the current vector uses: the always-mapped
/// `PKT_WINDOW`, or the page-straddling replay window.
static ACTIVE_BASE: AtomicU32 = AtomicU32::new(0);

pub fn window_base() -> u32 {
    ACTIVE_BASE.load(Ordering::Relaxed)
}

/// Fill the window with the default byte pattern.
pub fn window_reset() {
    let w = window_base() as *mut u32;
    for i in 0..WIN_WORDS {
        let b = i * 4;
        let v = (def_mem_byte(b) as u32)
            | ((def_mem_byte(b + 1) as u32) << 8)
            | ((def_mem_byte(b + 2) as u32) << 16)
            | ((def_mem_byte(b + 3) as u32) << 24);
        unsafe { core::ptr::write_volatile(w.add(i), v) };
    }
}

fn window_word(i: usize) -> u32 {
    unsafe { core::ptr::read_volatile((window_base() as *const u32).add(i)) }
}

fn window_set_word(i: usize, v: u32) {
    unsafe { core::ptr::write_volatile((window_base() as *mut u32).add(i), v) };
}

/// Cache attribute (CCC) of the VTCM mapping.
const VTCM_ATTR: u32 = 4;
/// TLB index of the VTCM mapping.
const IDX_VTCM: u32 = 1;

fn map_vtcm() {
    let lo = (TLB_PERM_XWRU << 28) | (VTCM_ATTR << 24) | (((VTCM_VA >> 20) & 0x7fff) << 9) | 0x10;
    tlb_write(make_tlb_hi(VTCM_VA >> 20, 0, true), lo, IDX_VTCM);
    isync();
}

fn unmap_vtcm() {
    tlb_invalidate(IDX_VTCM);
}

fn vt_word(i: usize) -> u32 {
    unsafe { core::ptr::read_volatile((VTCM_VA as *const u32).add(i)) }
}

fn vt_set_word(i: usize, v: u32) {
    unsafe { core::ptr::write_volatile((VTCM_VA as *mut u32).add(i), v) };
}

fn vt_reset() {
    for i in 0..VT_WORDS {
        let b = i * 4;
        vt_set_word(
            i,
            (def_vt_byte(b) as u32)
                | ((def_vt_byte(b + 1) as u32) << 8)
                | ((def_vt_byte(b + 2) as u32) << 16)
                | ((def_vt_byte(b + 3) as u32) << 24),
        );
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u32(&mut self) -> u32 {
        let b = &self.buf[self.pos..self.pos + 4];
        self.pos += 4;
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    }
    fn str(&mut self) -> &'a str {
        let n = self.u32() as usize;
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += (n + 3) & !3;
        // The generator only emits ASCII.
        unsafe { core::str::from_utf8_unchecked(s) }
    }
}

/// Register value for a bit-reversed access to `base + (v & 0xffff)`,
/// plus `(v >> 16) * 0x200`.
fn brev_domain(base: u32, v: u32) -> u32 {
    let ea = base.wrapping_add(v & 0xffff);
    let dom = (ea & 0xffff_0000) | (ea as u16).reverse_bits() as u32;
    dom.wrapping_add((v >> 16) << 9)
}

const MAGIC: u32 = 0x504B_5432; // "PKT2"
/// Stop printing individual mismatches after this many (all are counted).
const MAX_REPORTS: u32 = match option_env!("PKT_MAX_REPORTS") {
    Some(v) => parse_u32(v),
    None => 40,
};

const fn parse_u32(s: &str) -> u32 {
    let b = s.as_bytes();
    let mut i = 0;
    let mut n = 0u32;
    while i < b.len() {
        n = n * 10 + (b[i] - b'0') as u32;
        i += 1;
    }
    n
}

fn state_name(id: usize, buf: &mut [u8; 8]) -> &str {
    let n: &str = match id {
        ST_P => "p3:0",
        ST_USR => "usr",
        ST_M0 => "m0",
        ST_M1 => "m1",
        ST_CS0 => "cs0",
        ST_CS1 => "cs1",
        ST_GP => "gp",
        ST_LR => "lr",
        ST_SA0 => "sa0",
        ST_LC0 => "lc0",
        ST_SA1 => "sa1",
        ST_LC1 => "lc1",
        _ => {
            buf[0] = b'r';
            let mut n = 1;
            if id >= 10 {
                buf[n] = b'0' + (id / 10) as u8;
                n += 1;
            }
            buf[n] = b'0' + (id % 10) as u8;
            n += 1;
            return unsafe { core::str::from_utf8_unchecked(&buf[..n]) };
        }
    };
    n
}


/// An expected exception event (see pktgen::Ev).
#[derive(Clone, Copy)]
struct ExpEv {
    kind: u32,
    cause: u32,
    flags: u32,
    badva0: u32,
    badva1: u32,
}

const NO_EV: ExpEv = ExpEv { kind: 0, cause: 0, flags: 0, badva0: NONE, badva1: NONE };

/// Compare the logged events with `exp`; returns the number of mismatches
/// reported.  `elr_exp` is the expected ELR of every event (or `NONE`).
fn check_events(name: &str, vi: u32, label: &str, exp: &[ExpEv], elr_exp: &dyn Fn(usize) -> u32, base: u32, reports: &mut u32) -> bool {
    let s = st();
    let mut bad = false;
    let mut say = |bad: &mut bool, reports: &mut u32, args: core::fmt::Arguments| {
        *bad = true;
        record_error();
        if *reports < MAX_REPORTS {
            println!("FAIL {} [{}] {}", name, label, args);
        }
        *reports += 1;
    };
    let _ = vi;
    if s.nev as usize != exp.len() {
        say(&mut bad, reports, format_args!("exception count: got {} expected {}", s.nev, exp.len()));
        for i in 0..(s.nev as usize).min(MAX_EVENTS) {
            let e = s.events[i];
            if *reports < MAX_REPORTS {
                println!("     event {}: kind {} cause 0x{:x} elr 0x{:08x} ssr 0x{:08x} badva0 0x{:08x} badva1 0x{:08x}",
                    i, e.kind, e.cause, e.elr, e.ssr, e.badva0, e.badva1);
            }
        }
        return bad;
    }
    for (i, x) in exp.iter().enumerate() {
        let e = s.events[i];
        if e.kind != x.kind || e.cause != x.cause {
            say(&mut bad, reports, format_args!("event {}: kind/cause got {}/0x{:x} expected {}/0x{:x}", i, e.kind, e.cause, x.kind, x.cause));
            continue;
        }
        let want_elr = elr_exp(i);
        if want_elr != NONE && e.elr != want_elr {
            say(&mut bad, reports, format_args!("event {}: ELR got 0x{:08x} expected 0x{:08x}", i, e.elr, want_elr));
        }
        if x.flags & 1 != 0 {
            let got = (e.ssr >> 20) & 7;
            let want = (x.flags >> 1) & 7;
            if got != want {
                say(&mut bad, reports, format_args!("event {}: SSR[BVS,V1,V0] got {:03b} expected {:03b}", i, got, want));
            }
        }
        if x.badva0 == UNCHANGED {
            if e.badva0 != BADVA0_PRIME {
                say(&mut bad, reports, format_args!("event {}: BADVA0 changed to 0x{:08x}, expected unchanged", i, e.badva0));
            }
        } else if x.badva0 != NONE && e.badva0 != base.wrapping_add(x.badva0) {
            say(&mut bad, reports, format_args!("event {}: BADVA0 got 0x{:08x} expected 0x{:08x}", i, e.badva0, base.wrapping_add(x.badva0)));
        }
        if x.badva1 == UNCHANGED {
            if e.badva1 != BADVA1_PRIME {
                say(&mut bad, reports, format_args!("event {}: BADVA1 changed to 0x{:08x}, expected unchanged", i, e.badva1));
            }
        } else if x.badva1 != NONE && e.badva1 != base.wrapping_add(x.badva1) {
            say(&mut bad, reports, format_args!("event {}: BADVA1 got 0x{:08x} expected 0x{:08x}", i, e.badva1, base.wrapping_add(x.badva1)));
        }
    }
    bad
}

/// Walk the packets of `code` (as loaded at `dest`) and return the
/// (start, end) addresses of the first packet that ends after `boundary`.
fn packet_over(code: &[u32], dest: u32, boundary: u32) -> (u32, u32) {
    let mut i = 0;
    while i < code.len() {
        let mut j = i;
        loop {
            let w = code[j];
            j += 1;
            let pb = (w >> 14) & 3;
            if pb == 3 || pb == 0 || j >= code.len() {
                break;
            }
        }
        let (a, b) = (dest + 4 * i as u32, dest + 4 * j as u32);
        if b > boundary {
            return (a, b);
        }
        i = j;
    }
    (0, 0)
}

/// Run every case in `blob`, return the number of failed cases.
pub fn run_table(suite: &str, blob: &'static [u8]) -> u32 {
    let mut rd = Reader { buf: blob, pos: 0 };
    assert_eq!(rd.u32(), MAGIC, "bad table");
    let ncases = rd.u32() as usize;
    let mut ctx = PktCtx {
        inp: [0; NSTATE],
        out: [0; NSTATE],
        func: 0,
        ret: 0,
        save: [0; 10],
        vptr: PKT_HVX.0.get() as u32,
    };
    let mut reports = 0u32;
    let mut failed_cases = 0u32;
    let mut vectors = 0u32;
    // Defaults, computed once.
    let mut def_regs = [0u32; NSTATE];
    for id in 0..NSTATE {
        def_regs[id] = def_state(id);
    }
    let mut def_win = [0u32; WIN_WORDS];
    for i in 0..WIN_WORDS {
        let b = i * 4;
        def_win[i] = (def_mem_byte(b) as u32)
            | ((def_mem_byte(b + 1) as u32) << 8)
            | ((def_mem_byte(b + 2) as u32) << 16)
            | ((def_mem_byte(b + 3) as u32) << 24);
    }
    let mut exp = [0u32; NSTATE];
    let mut exp_mem = [0u32; WIN_WORDS];
    let mut exp_ev = [NO_EV; MAX_EVENTS];
    let mut hv_exp = [[0u8; VLEN]; 36];
    let mut hv_ignore = [false; 36];
    let mut vt_exp = [0u32; VT_WORDS];
    let mut vt_ign = [false; VT_WORDS];

    // Every table suite runs under the packet-test vectors so exceptions
    // are logged, never silently skipped.
    let old_evb = read_evb();
    write_evb(vectors_addr());

    println!("{}: {} cases", suite, ncases);
    for ci in 0..ncases {
        let name = rd.str();
        let text = rd.str();
        let mode = rd.u32();
        let unmapped = rd.u32();
        let miss_before_map = rd.u32();
        let elr_off = rd.u32();
        let nvec = rd.u32();
        let func = unsafe { *(pkt_fns.as_ptr().add(ci)) };
        let flen = unsafe { *(pkt_lens.as_ptr().add(ci)) };
        let mut case_failed = false;

        for vi in 0..nvec {
            // --- Input state.
            let base = if mode & MODE_DATA_REPLAY != 0 { DATA_WIN_BASE } else { PKT_WINDOW.0.get() as u32 };
            ACTIVE_BASE.store(base, Ordering::Relaxed);
            ctx.inp = def_regs;
            if mode & MODE_DATA_REPLAY != 0 {
                map_data_page(0);
                map_data_page(1);
            }
            {
                let w = window_base() as *mut u32;
                for i in 0..WIN_WORDS {
                    unsafe { core::ptr::write_volatile(w.add(i), def_win[i]) };
                }
            }
            if mode & MODE_VTCM != 0 {
                map_vtcm();
                vt_reset();
            }
            let nin = rd.u32();
            let mut nwin_in = 0u32;
            for _ in 0..nin {
                let id = rd.u32();
                let mut val = rd.u32();
                if id & WIN_REL != 0 {
                    val = val.wrapping_add(base);
                }
                if id & VT_REL != 0 {
                    val = val.wrapping_add(VTCM_VA);
                }
                if id & BREV != 0 {
                    val = brev_domain(base, val);
                }
                let id = id & !(WIN_REL | BREV | VT_REL);
                if id >= VT_ID_BASE {
                    vt_set_word((id - VT_ID_BASE) as usize, val);
                } else if id >= MEM_ID_BASE {
                    window_set_word((id - MEM_ID_BASE) as usize, val);
                    nwin_in += 1;
                } else {
                    ctx.inp[id as usize] = val;
                }
            }
            // HVX inputs.  Slots 0..31 are v0..v31, 64..67 q0..q3 (inputs);
            // outputs land at 32.. and 68..
            if mode & MODE_HVX != 0 {
                hv_ignore = [false; 36];
                for n in 0..32 {
                    for i in 0..VLEN {
                        unsafe { *hvx_slot(n).add(i) = def_vbyte(n, i) };
                    }
                }
                for n in 0..4 {
                    for i in 0..VLEN {
                        unsafe { *hvx_slot(64 + n).add(i) = def_qbyte(n, i) };
                    }
                }
            }
            let nhin = rd.u32();
            for _ in 0..nhin {
                let idx = rd.u32() as usize;
                let slot = if idx < 32 { idx } else { 64 + idx - 32 };
                for w in 0..VLEN / 4 {
                    let x = rd.u32().to_le_bytes();
                    for k in 0..4 {
                        unsafe { *hvx_slot(slot).add(w * 4 + k) = x[k] };
                    }
                }
            }
            // --- Expected state = input state + expected overrides.
            exp = ctx.inp;
            // The window now holds defaults plus the input overrides.
            exp_mem = def_win;
            if nwin_in != 0 {
                for i in 0..WIN_WORDS {
                    exp_mem[i] = window_word(i);
                }
            }
            if mode & MODE_VTCM != 0 {
                for i in 0..VT_WORDS {
                    vt_exp[i] = vt_word(i);
                    vt_ign[i] = false;
                }
            }
            let mut ignore = [false; NSTATE];
            let mut ignore_mem = 0u128;
            // Snapshot of the input image for the partial-commit check.
            let exp_mem_in = exp_mem;
            let nout = rd.u32();
            for _ in 0..nout {
                let id = rd.u32();
                let mut val = rd.u32();
                if id & WIN_REL != 0 {
                    val = val.wrapping_add(base);
                }
                if id & VT_REL != 0 {
                    val = val.wrapping_add(VTCM_VA);
                }
                if id & BREV != 0 {
                    val = brev_domain(base, val);
                }
                let ign = id & IGNORE != 0;
                let id = id & !(WIN_REL | IGNORE | BREV | VT_REL);
                if id >= VT_ID_BASE {
                    let w = (id - VT_ID_BASE) as usize;
                    if ign {
                        vt_ign[w] = true;
                    } else {
                        vt_exp[w] = val;
                    }
                } else if id >= MEM_ID_BASE {
                    if ign {
                        ignore_mem |= 1u128 << (id - MEM_ID_BASE);
                    } else {
                        exp_mem[(id - MEM_ID_BASE) as usize] = val;
                    }
                } else if ign {
                    ignore[id as usize] = true;
                } else {
                    exp[id as usize] = val;
                }
            }
            // Expected HVX state starts as the input.
            if mode & MODE_HVX != 0 {
                for n in 0..32 {
                    for i in 0..VLEN {
                        hv_exp[n][i] = unsafe { *hvx_slot(n).add(i) };
                    }
                }
                for n in 0..4 {
                    for i in 0..VLEN {
                        hv_exp[32 + n][i] = unsafe { *hvx_slot(64 + n).add(i) };
                    }
                }
            }
            let nhout = rd.u32();
            for _ in 0..nhout {
                let raw = rd.u32();
                let idx = (raw & !IGNORE) as usize;
                if raw & IGNORE != 0 {
                    hv_ignore[idx] = true;
                }
                for w in 0..VLEN / 4 {
                    let x = rd.u32().to_le_bytes();
                    for k in 0..4 {
                        hv_exp[idx][w * 4 + k] = x[k];
                    }
                }
            }
            let nev = rd.u32() as usize;
            for i in 0..nev {
                let kind = rd.u32();
                let cause = rd.u32();
                let flags = rd.u32();
                let badva0 = rd.u32();
                let badva1 = rd.u32();
                let _ = rd.u32();
                if i < MAX_EVENTS {
                    exp_ev[i] = ExpEv { kind, cause, flags, badva0, badva1 };
                }
            }

            // --- Replay policy.
            {
                let s = st();
                s.miss_before_map = if miss_before_map == 0 { 1 } else { miss_before_map };
                s.resume_elr = if mode & MODE_RESUME_AT_END != 0 { func + flen - EPILOGUE_BYTES } else { 0 };
                s.check_partial = mode & MODE_DATA_REPLAY != 0 && mode & MODE_NO_PARTIAL_CHECK == 0;
                if s.check_partial {
                    s.in_img = exp_mem_in;
                }
            }

            // How many placements (runs) this vector takes.
            let placements: u32 = if mode & MODE_CODE_REPLAY != 0 { (flen / 4).saturating_sub(1) } else { 1 };
            for pl in 0..placements {
                let mut run_func = func;
                let mut boundary = 0u32;
                let mut dest = 0u32;
                let mut label = [0u8; 0];
                let _ = &mut label;
                if mode & MODE_CODE_REPLAY != 0 {
                    // Copy the case so that the page boundary falls `delta`
                    // bytes into it.
                    let delta = 4 * (pl + 1);
                    boundary = CODE_BASE + 0x1000;
                    dest = boundary - delta;
                    map_code_page(0);
                    map_code_page(1);
                    for w in 0..(flen / 4) {
                        unsafe {
                            let v = core::ptr::read_volatile((func as *const u32).add(w as usize));
                            core::ptr::write_volatile((dest as *mut u32).add(w as usize), v);
                        }
                    }
                    let mut a = dest & !31;
                    while a < dest + flen {
                        dccleaninva(a);
                        a += 32;
                    }
                    ickill();
                    isync();
                    run_func = dest;
                }
                // Make the pages the case wants unmapped so its first
                // access misses.
                if mode & MODE_DATA_REPLAY != 0 {
                    for i in 0..2 {
                        if unmapped & (1 << i) != 0 {
                            unmap_data_page(i);
                        }
                    }
                }
                if mode & MODE_CODE_REPLAY != 0 {
                    for i in 0..2 {
                        if unmapped & (1 << i) != 0 {
                            unmap_code_page(i);
                        }
                    }
                }
                reset_events();
                {
                    let s = st();
                    s.check_regs = mode & (MODE_DATA_REPLAY | MODE_RESUME_AT_END) != 0 && mode & MODE_RESTORE_ENABLES == 0;
                    if mode & MODE_RESTORE_ENABLES != 0 {
                        s.ssr_restore_mask = SSR_XE_MASK;
                        s.ssr_saved = read_ssr();
                        s.ccr_restore = true;
                        s.ccr_saved = read_ccr();
                    } else {
                        s.ssr_restore_mask = 0;
                        s.ccr_restore = false;
                    }
                    s.in_regs.copy_from_slice(&ctx.inp[..16]);
                    s.in_p = ctx.inp[ST_P];
                    s.in_usr = ctx.inp[ST_USR];
                }
                if mode & (MODE_DATA_REPLAY | MODE_RESUME_AT_END) != 0 {
                    prime_badva();
                }

                if option_env!("PKT_TRACE").is_some() {
                    println!("TRACE {} vec {} placement {} func 0x{:08x}", name, vi, pl, run_func);
                }
                // --- Run.
                ctx.func = run_func;
                ctx.out = [0; NSTATE];
                if mode & MODE_HVX != 0 {
                    unsafe { pkt_run_hvx(&mut ctx) };
                } else {
                    unsafe { pkt_run(&mut ctx) };
                }
                vectors += 1;

                // --- Leave the data pages readable for the comparison.
                if mode & MODE_DATA_REPLAY != 0 {
                    map_data_page(0);
                    map_data_page(1);
                }
                st().check_partial = false;
                st().check_regs = false;
                if mode & MODE_RESTORE_ENABLES != 0 {
                    let (m, sv, cc) = (st().ssr_restore_mask, st().ssr_saved, st().ccr_saved);
                    write_ssr((read_ssr() & !m) | (sv & m));
                    write_ccr(cc);
                    isync();
                    st().ssr_restore_mask = 0;
                    st().ccr_restore = false;
                }

                if option_env!("PKT_TRACE").is_some() {
                    let s = st();
                    for i in 0..(s.nev as usize).min(MAX_EVENTS) {
                        let e = s.events[i];
                        println!(
                            "TRACE   event {}: {} cause 0x{:02x} elr 0x{:08x} ssr[BVS,V1,V0]={:03b} badva0 0x{:08x} badva1 0x{:08x}",
                            i,
                            match e.kind { 1 => "tlb-miss-rw", 2 => "tlb-miss-x", _ => "error" },
                            e.cause, e.elr, (e.ssr >> 20) & 7, e.badva0, e.badva1
                        );
                    }
                }
                let mut bad = false;
                let label = if mode & MODE_CODE_REPLAY != 0 { pl } else { vi };
                // --- Exceptions.
                if mode & MODE_CODE_REPLAY != 0 {
                    // Expected fetch misses, derived from the layout.
                    let mbm = st().miss_before_map as usize;
                    let mut n = 0;
                    let mut elrs = [0u32; MAX_EVENTS];
                    let mut push = |cause: u32, elr: u32, n: &mut usize| {
                        for _ in 0..mbm {
                            if *n < MAX_EVENTS {
                                exp_ev[*n] = ExpEv { kind: EV_X, cause, flags: 0, badva0: NONE, badva1: NONE };
                                elrs[*n] = elr;
                            }
                            *n += 1;
                        }
                    };
                    if unmapped & 1 != 0 {
                        push(0x60, dest, &mut n);
                    }
                    if unmapped & 2 != 0 {
                        let code = unsafe { core::slice::from_raw_parts(dest as *const u32, (flen / 4) as usize) };
                        let (a, _) = packet_over(code, dest, boundary);
                        if a < boundary {
                            push(0x61, a, &mut n);
                        } else {
                            push(0x60, a, &mut n);
                        }
                    }
                    let f = |i: usize| elrs[i];
                    bad |= check_events(name, vi, "code", &exp_ev[..n.min(MAX_EVENTS)], &f, 0, &mut reports);
                    if n > MAX_EVENTS {
                        bad = true;
                    }
                } else if mode & (MODE_DATA_REPLAY | MODE_RESUME_AT_END) != 0 {
                    let off = elr_off;
                    let f = |_: usize| if off == NONE { NONE } else { run_func + off };
                    bad |= check_events(name, vi, "events", &exp_ev[..nev.min(MAX_EVENTS)], &f, base, &mut reports);
                } else {
                    // Plain cases: nothing may be raised.  (The error hook
                    // skips the faulting word, which would leave a half-run
                    // packet that merely looks wrong.)
                    bad |= check_events(name, vi, "events", &[], &|_| NONE, base, &mut reports);
                }
                {
                    let s = st();
                    if s.reg_partial != 0 {
                        bad = true;
                        record_error();
                        if reports < MAX_REPORTS {
                            println!("FAIL {} [vec {}] registers changed before the exception was taken ({} diffs, first {})", name, vi, s.reg_partial, s.reg_first);
                        }
                        reports += 1;
                    }
                }
                if mode & MODE_DATA_REPLAY != 0 {
                    let s = st();
                    if s.partial_commits != 0 {
                        bad = true;
                        record_error();
                        if reports < MAX_REPORTS {
                            println!("FAIL {} [vec {}] packet partially committed before replay: {} changed word(s), first at win+0x{:03x}: 0x{:08x} (was 0x{:08x})", name, vi, s.partial_commits, s.partial_first, s.partial_got, s.partial_exp);
                        }
                        reports += 1;
                    }
                    if s.stray != 0 {
                        bad = true;
                        record_error();
                        if reports < MAX_REPORTS {
                            println!("FAIL {} [vec {}] {} miss(es) outside the replay pages", name, vi, s.stray);
                        }
                        reports += 1;
                    }
                }
                let _ = label;

                // --- State.
                for id in 0..NSTATE {
                    if !ignore[id] && ctx.out[id] != exp[id] {
                        bad = true;
                        record_error();
                        if reports < MAX_REPORTS {
                            let mut nb = [0u8; 8];
                            println!(
                                "FAIL {} [vec {}] {}: got 0x{:08x} expected 0x{:08x}",
                                name, vi, state_name(id, &mut nb), ctx.out[id], exp[id]
                            );
                        }
                        reports += 1;
                    }
                }
                if mode & MODE_HVX != 0 {
                    for n in 0..36usize {
                        if hv_ignore[n] {
                            continue;
                        }
                        let slot = if n < 32 { 32 + n } else { 68 + n - 32 };
                        for i in 0..VLEN {
                            let got = unsafe { *hvx_slot(slot).add(i) };
                            if got != hv_exp[n][i] {
                                bad = true;
                                record_error();
                                if reports < MAX_REPORTS {
                                    if n < 32 {
                                        println!("FAIL {} [vec {}] v{}.b[{}]: got 0x{:02x} expected 0x{:02x}", name, vi, n, i, got, hv_exp[n][i]);
                                    } else {
                                        println!("FAIL {} [vec {}] q{}[{}]: got 0x{:02x} expected 0x{:02x}", name, vi, n - 32, i, got, hv_exp[n][i]);
                                    }
                                }
                                reports += 1;
                                break;
                            }
                        }
                    }
                }
                if mode & MODE_VTCM != 0 {
                    for i in 0..VT_WORDS {
                        let got = vt_word(i);
                        if got != vt_exp[i] && !vt_ign[i] {
                            bad = true;
                            record_error();
                            if reports < MAX_REPORTS {
                                println!("FAIL {} [vec {}] vtcm+0x{:04x}: got 0x{:08x} expected 0x{:08x}", name, vi, i * 4, got, vt_exp[i]);
                            }
                            reports += 1;
                        }
                    }
                }
                for i in 0..WIN_WORDS {
                    let got = window_word(i);
                    if got != exp_mem[i] && ignore_mem & (1u128 << i) == 0 {
                        bad = true;
                        record_error();
                        if reports < MAX_REPORTS {
                            println!(
                                "FAIL {} [vec {}] win+0x{:03x}: got 0x{:08x} expected 0x{:08x}",
                                name, vi, i * 4, got, exp_mem[i]
                            );
                        }
                        reports += 1;
                    }
                }
                if bad && !case_failed {
                    case_failed = true;
                    failed_cases += 1;
                    if reports <= MAX_REPORTS {
                        println!("     packet: {}", text);
                    }
                }
                // Restore full mapping for the next vector's setup.
                if mode & MODE_DATA_REPLAY != 0 {
                    unmap_data_page(0);
                    unmap_data_page(1);
                }
                if mode & MODE_CODE_REPLAY != 0 {
                    unmap_code_page(0);
                    unmap_code_page(1);
                }
            }
        }
    }
    write_evb(old_evb);
    println!(
        "{}: {} cases, {} vectors, {} failed cases",
        suite, ncases, vectors, failed_cases
    );
    if reports > MAX_REPORTS {
        println!("({} mismatches not shown)", reports - MAX_REPORTS);
    }
    failed_cases
}
