// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Data TLB misses and packet replay (table driven; see pktgen/replay_rw.rs).

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

use hexagon_arch_tests::*;

#[path = "../pkt/mod.rs"]
mod pkt;

#[no_mangle]
pub extern "C" fn rust_main() -> i32 {
    test_suite_begin("pkt_replay_rw");
    pkt::run_table(
        "pkt_replay_rw",
        include_bytes!(concat!(env!("OUT_DIR"), "/test_pkt_replay_rw.tbl")),
    );
    test_suite_end() as i32
}
