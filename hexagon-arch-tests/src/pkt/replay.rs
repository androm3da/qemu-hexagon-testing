// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Exception hooks and TLB policy for the packet replay / exception tests.
//!
//! asm/pkt/pkt_vec.S installs an event vector table whose precise-error,
//! instruction-TLB-miss and data-TLB-miss entries call the `pkt_hook_*`
//! functions below with the machine exactly as the faulting packet left it.
//! The hooks log each event (cause, ELR, SSR including V0/V1/BVS, BADVA0/1)
//! and apply the policy the current test vector asked for:
//!
//!   * TLB misses in the replay regions are satisfied only on the Nth miss
//!     of a page (`miss_before_map`), so a packet can be made to replay
//!     several times; other pages stay unmapped.
//!   * Before the page is mapped, the hook checks that no part of a
//!     faulting packet has been committed: every mapped word of the data
//!     window must still hold its initial value.
//!   * Precise errors resume at `resume_elr` (or just after the faulting
//!     word, like crt0's handler, when it is zero).

#![allow(dead_code)]

use core::arch::asm;
use super::WIN_WORDS;
use hexagon_arch_tests::*;

pub const MAX_EVENTS: usize = 16;

/// Which handler logged an event.
pub const EV_RW: u32 = 1;
pub const EV_X: u32 = 2;
pub const EV_ERR: u32 = 3;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Event {
    pub kind: u32,
    pub cause: u32,
    pub ssr: u32,
    pub elr: u32,
    pub badva0: u32,
    pub badva1: u32,
}

const EV0: Event = Event { kind: 0, cause: 0, ssr: 0, elr: 0, badva0: 0, badva1: 0 };

pub const SSR_V0: u32 = 1 << 20;
pub const SSR_V1: u32 = 1 << 21;
pub const SSR_BVS: u32 = 1 << 22;

/// First page of the data replay region.  The 512-byte test window is the
/// last 0x100 bytes of page 0 followed by the first 0x100 bytes of page 1,
/// so window offset 0x100 is the page boundary.
pub const DATA_BASE: u32 = 0x9b90_0000;
/// First page of the code replay region (functions are copied here).
pub const CODE_BASE: u32 = 0x9ba0_0000;
pub const DATA_WIN_BASE: u32 = DATA_BASE + 0x1000 - 0x100;
/// TLB indices owned by the replay regions.  They are below crt0's
/// TLB_FIXED_ENTRIES, so its round-robin miss handler (indices 6 and up) can
/// never recycle them, and the hooks below cannot evict a translation of the
/// image that the default handler installed earlier.
const IDX_DATA: [u32; 2] = [1, 2];
const IDX_CODE: [u32; 2] = [3, 4];
/// Indices used for stray 1:1 fills (a miss outside the replay regions is
/// satisfied rather than retried forever).
const IDX_STRAY: u32 = 120;
static STRAY_NEXT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn map_stray(va: u32) {
    let n = STRAY_NEXT.fetch_add(1, core::sync::atomic::Ordering::Relaxed) % 7;
    tlb_write(
        make_tlb_hi(va >> 20, 0, true),
        make_tlb_lo(va >> 20, TLB_PERM_XWRU, true),
        IDX_STRAY + n,
    );
    isync();
}

pub struct State {
    pub events: [Event; MAX_EVENTS],
    pub nev: u32,
    pub miss_before_map: u32,
    pub data_miss: [u32; 2],
    pub data_mapped: [bool; 2],
    pub code_miss: [u32; 2],
    pub code_mapped: [bool; 2],
    pub resume_elr: u32,
    /// Initial image of the data window, for the partial-commit check.
    pub in_img: [u32; WIN_WORDS],
    /// Check `in_img` against the mapped data pages at each data miss.
    pub check_partial: bool,
    pub partial_commits: u32,
    pub partial_first: u32,
    pub partial_got: u32,
    pub partial_exp: u32,
    /// Misses outside the replay regions.
    pub stray: u32,
    /// Input registers r0..r15, P3:0 and USR of the vector being run.  At
    /// every exception the interrupted registers must still hold them: a
    /// packet that faults has committed nothing.
    pub in_regs: [u32; 16],
    pub in_p: u32,
    pub in_usr: u32,
    pub check_regs: bool,
    pub reg_partial: u32,
    pub reg_first: u32,
    /// SSR bits (e.g. the coprocessor enables) that every hook puts back to
    /// `ssr_saved`, so a case may leave them cleared and still be resumed.
    pub aborted: bool,
    pub ssr_restore_mask: u32,
    pub ssr_saved: u32,
    /// Same for CCR (HLX enable).
    pub ccr_restore: bool,
    pub ccr_saved: u32,
}

static mut ST: State = State {
    events: [EV0; MAX_EVENTS],
    nev: 0,
    miss_before_map: 1,
    data_miss: [0; 2],
    data_mapped: [false; 2],
    code_miss: [0; 2],
    code_mapped: [false; 2],
    resume_elr: 0,
    in_img: [0; WIN_WORDS],
    check_partial: false,
    partial_commits: 0,
    partial_first: 0,
    partial_got: 0,
    partial_exp: 0,
    stray: 0,
    in_regs: [0; 16],
    in_p: 0,
    in_usr: 0,
    check_regs: false,
    reg_partial: 0,
    reg_first: 0,
    aborted: false,
    ssr_restore_mask: 0,
    ssr_saved: 0,
    ccr_restore: false,
    ccr_saved: 0,
};

pub fn st() -> &'static mut State {
    unsafe { &mut *(&raw mut ST) }
}

#[inline(always)]
pub fn read_badva0() -> u32 {
    let v: u32;
    unsafe { asm!("{v} = badva0", v = out(reg) v, options(nomem, nostack)) };
    v
}

#[inline(always)]
pub fn read_badva1() -> u32 {
    let v: u32;
    unsafe { asm!("{v} = badva1", v = out(reg) v, options(nomem, nostack)) };
    v
}

#[inline(always)]
pub fn write_elr(v: u32) {
    unsafe { asm!("elr = {v}", v = in(reg) v, options(nostack)) };
}

#[inline(always)]
pub fn write_badva0(v: u32) {
    unsafe { asm!("badva0 = {v}", v = in(reg) v, options(nostack)) };
}

#[inline(always)]
pub fn write_badva1(v: u32) {
    unsafe { asm!("badva1 = {v}", v = in(reg) v, options(nostack)) };
}

extern "C" {
    static pkt_hook_frame: u32;
}

/// Compare the interrupted registers (saved by the vector entry) with the
/// vector's inputs.
fn check_interrupted_regs() {
    let s = st();
    if !s.check_regs {
        return;
    }
    let fp = unsafe { core::ptr::read_volatile(&raw const pkt_hook_frame) } as *const u32;
    for i in 0..16usize {
        let v = unsafe { *fp.add(i) };
        if v != s.in_regs[i] {
            if s.reg_partial == 0 {
                s.reg_first = i as u32;
            }
            s.reg_partial += 1;
        }
    }
    let (p, usr) = unsafe { (*fp.add(16), *fp.add(17)) };
    if p != s.in_p {
        if s.reg_partial == 0 {
            s.reg_first = 100;
        }
        s.reg_partial += 1;
    }
    if usr != s.in_usr {
        if s.reg_partial == 0 {
            s.reg_first = 101;
        }
        s.reg_partial += 1;
    }
}

/// Sentinels that BADVA0/BADVA1 are primed with, so "unchanged" is
/// observable.
pub const BADVA0_PRIME: u32 = 0xBAD0_0000;
pub const BADVA1_PRIME: u32 = 0xBAD1_0000;

/// Prime BADVA0/1 and clear SSR.{V0,V1,BVS}... except that V0/V1/BVS are set,
/// so that a handler that fails to update them is distinguishable from one
/// that clears them.
pub fn prime_badva() {
    write_badva0(BADVA0_PRIME);
    write_badva1(BADVA1_PRIME);
    write_ssr(read_ssr() | SSR_V0 | SSR_V1 | SSR_BVS);
}

fn log(kind: u32) -> Event {
    check_interrupted_regs();
    let s = st();
    // Read the machine state first, then put the enables back.
    let ev = Event {
        kind,
        cause: read_ssr() & 0xff,
        ssr: read_ssr(),
        elr: read_elr(),
        badva0: read_badva0(),
        badva1: read_badva1(),
    };
    if s.ssr_restore_mask != 0 {
        write_ssr((read_ssr() & !s.ssr_restore_mask) | (s.ssr_saved & s.ssr_restore_mask));
    }
    if s.ccr_restore {
        write_ccr(s.ccr_saved);
    }
    if (s.nev as usize) < MAX_EVENTS {
        s.events[s.nev as usize] = ev;
    }
    s.nev += 1;
    if s.nev > 64 {
        // A fault that never makes progress: leave the exception through an
        // rte into a function that reports it and ends the run (resuming
        // the case is not safe: replaying from a wrong ELR may have
        // re-executed non-idempotent packets).
        s.aborted = true;
        write_elr(pkt_abort as usize as u32);
    }
    ev
}

// ---------------------------------------------------------------------------
// Mapping helpers (usable from the test body and from the hooks)
// ---------------------------------------------------------------------------

fn map_page(va: u32, idx: u32) {
    tlb_write(make_tlb_hi_4k(va), make_tlb_lo_4k(va, TLB_PERM_XWRU, true), idx);
    isync();
}

pub fn map_data_page(i: usize) {
    map_page(DATA_BASE + 0x1000 * i as u32, IDX_DATA[i]);
    st().data_mapped[i] = true;
}

pub fn unmap_data_page(i: usize) {
    tlb_invalidate(IDX_DATA[i]);
    st().data_mapped[i] = false;
}

pub fn map_code_page(i: usize) {
    map_page(CODE_BASE + 0x1000 * i as u32, IDX_CODE[i]);
    st().code_mapped[i] = true;
}

pub fn unmap_code_page(i: usize) {
    tlb_invalidate(IDX_CODE[i]);
    st().code_mapped[i] = false;
}

/// Compare the mapped words of the data window with the initial image.
fn check_window_untouched() {
    let s = st();
    for i in 0..2usize {
        if !s.data_mapped[i] {
            continue;
        }
        let lo = i * (WIN_WORDS / 2);
        for w in lo..lo + WIN_WORDS / 2 {
            let got = unsafe { core::ptr::read_volatile((DATA_WIN_BASE as *const u32).add(w)) };
            if got != s.in_img[w] {
                if s.partial_commits == 0 {
                    s.partial_first = (w * 4) as u32;
                    s.partial_got = got;
                    s.partial_exp = s.in_img[w];
                }
                s.partial_commits += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Hooks (called from pkt_vec.S)
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn pkt_hook_rw() {
    let ev = log(EV_RW);
    let s = st();
    let va = if ev.ssr & SSR_BVS != 0 { ev.badva1 } else { ev.badva0 };
    let page = va.wrapping_sub(DATA_BASE) >> 12;
    if page < 2 {
        let page = page as usize;
        if s.check_partial {
            check_window_untouched();
        }
        s.data_miss[page] += 1;
        if s.data_miss[page] >= s.miss_before_map {
            map_data_page(page);
        }
    } else {
        s.stray += 1;
        map_stray(va);
    }
}

#[no_mangle]
pub extern "C" fn pkt_hook_x() {
    let ev = log(EV_X);
    let s = st();
    // Cause 0x60: the page of the packet itself is missing.  Cause 0x61: the
    // packet continues into the next page, which is the one missing.
    let va = if ev.cause == CAUSE_TLBMISSX_NEXTPAGE { ev.elr + 16 } else { ev.elr };
    let page = va.wrapping_sub(CODE_BASE) >> 12;
    if page < 2 {
        let page = page as usize;
        s.code_miss[page] += 1;
        if s.code_miss[page] >= s.miss_before_map {
            map_code_page(page);
        }
    } else {
        s.stray += 1;
        map_stray(va);
    }
}

#[no_mangle]
pub extern "C" fn pkt_hook_err() {
    let _ = log(EV_ERR);
    let s = st();
    let elr = read_elr();
    write_elr(if s.resume_elr != 0 { s.resume_elr } else { elr + 4 });
}

extern "C" fn pkt_abort() -> ! {
    let s = st();
    let e = s.events[(MAX_EVENTS - 1).min(s.nev as usize - 1)];
    println!(
        "ABORT: {} exceptions without progress; last event kind {} cause 0x{:x} elr 0x{:08x}",
        s.nev, e.kind, e.cause, e.elr
    );
    record_error();
    println!("FAIL (aborted)");
    semihosting_exit(2);
}

/// Forget everything logged so far and start a fresh vector.
pub fn reset_events() {
    let s = st();
    s.nev = 0;
    s.data_miss = [0; 2];
    s.code_miss = [0; 2];
    s.partial_commits = 0;
    s.partial_first = 0;
    s.stray = 0;
    s.reg_partial = 0;
    s.reg_first = 0;
    s.aborted = false;
}

extern "C" {
    fn pkt_vectors();
}

pub fn vectors_addr() -> u32 {
    pkt_vectors as usize as u32
}
