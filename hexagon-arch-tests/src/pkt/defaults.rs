// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Default machine state for table-driven packet cases.
//!
//! This file is compiled into both the target-side runner (src/pkt/mod.rs)
//! and the host-side case generator (pktgen/), so it must stay pure `core`
//! code with no dependencies.  A case only lists the state it overrides;
//! everything else starts as, and (unless a case says otherwise) must still
//! be after the packet, the value below.  Leftover sentinels therefore catch
//! clobbered registers and stray stores.

/// Number of scalar state slots: r0..r27 plus the control registers listed
/// in asm/pkt/pkt_common.h.
pub const NSTATE: usize = 40;

pub const ST_P: usize = 28;
pub const ST_USR: usize = 29;
pub const ST_M0: usize = 30;
pub const ST_M1: usize = 31;
pub const ST_CS0: usize = 32;
pub const ST_CS1: usize = 33;
pub const ST_GP: usize = 34;
pub const ST_LR: usize = 35;
pub const ST_SA0: usize = 36;
pub const ST_LC0: usize = 37;
pub const ST_SA1: usize = 38;
pub const ST_LC1: usize = 39;

/// State ids from this value up name 32-bit words of the memory window.
pub const MEM_ID_BASE: u32 = 64;
/// Size of the memory window that packets are pointed at.
pub const WIN_BYTES: usize = 512;
pub const WIN_WORDS: usize = WIN_BYTES / 4;

/// Set in the id of an override whose value is an offset from the window
/// base rather than an absolute value.
pub const WIN_REL: u32 = 0x8000_0000;

/// Default value of state slot `id` at the start of a case.
pub const fn def_state(id: usize) -> u32 {
    match id {
        0..=27 => 0xC0DE_0000 | ((id as u32) << 8) | 0x11,
        ST_P => 0x00FF_00FF,
        ST_USR => 0,
        ST_M0 => 0x4D30_4D30,
        ST_M1 => 0x4D31_4D31,
        ST_CS0 => 0xC500_0000,
        ST_CS1 => 0xC510_0000,
        ST_GP => 0xC0DE_6700,
        ST_LR => 0xC0DE_0123,
        _ => 0,
    }
}

/// Default value of byte `k` of the memory window.  All bytes in any
/// 256-byte span are distinct, so a misplaced or partial store is visible.
pub const fn def_mem_byte(k: usize) -> u8 {
    (k as u32).wrapping_mul(29).wrapping_add(0x40) as u8
}

/// Set in the id of an expected-output entry whose slot must not be checked.
pub const IGNORE: u32 = 0x4000_0000;

/// Set in the id of an entry whose value is `off | (add / 0x200) << 16`:
/// the bit-reversed-domain pointer for window offset `off`, plus `add`.
pub const BREV: u32 = 0x2000_0000;

/// HVX vector length in bytes (128-byte mode).
pub const VLEN: usize = 128;

/// Default byte `i` of vector register `n`.  Lanes and registers differ, so
/// a lane that ends up in the wrong place or register is visible.
pub const fn def_vbyte(n: usize, i: usize) -> u8 {
    ((n as u32).wrapping_mul(37).wrapping_add((i as u32).wrapping_mul(5)) ^ 0x5a) as u8
}

/// Default byte `i` of predicate register `n`, in the 0xff/0x00 per-byte
/// form the harness exchanges (one byte per predicate bit).
pub const fn def_qbyte(n: usize, i: usize) -> u8 {
    if (i * 3 + n) % 5 < 2 {
        0xff
    } else {
        0
    }
}

/// State ids from here up name 32-bit words of the VTCM window.
pub const VT_ID_BASE: u32 = 0x8000;
/// Size of the VTCM window compared after a case.
pub const VT_BYTES: usize = 8192;
pub const VT_WORDS: usize = VT_BYTES / 4;
/// Set in the id of an entry whose value is an offset from the VTCM base.
pub const VT_REL: u32 = 0x1000_0000;
/// Virtual (= physical) address of the VTCM.
pub const VTCM_VA: u32 = 0xd900_0000;

/// Default byte `k` of the VTCM window.
pub const fn def_vt_byte(k: usize) -> u8 {
    ((k as u32).wrapping_mul(31).wrapping_add(7) ^ ((k as u32) >> 8)) as u8
}
