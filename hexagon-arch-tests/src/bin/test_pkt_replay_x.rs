// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Instruction TLB misses with the page boundary at every word of the code
//! (table driven; see pktgen/replay_x.rs).

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

use hexagon_arch_tests::*;

#[path = "../pkt/mod.rs"]
mod pkt;

#[no_mangle]
pub extern "C" fn rust_main() -> i32 {
    test_suite_begin("pkt_replay_x");
    pkt::run_table(
        "pkt_replay_x",
        include_bytes!(concat!(env!("OUT_DIR"), "/test_pkt_replay_x.tbl")),
    );
    test_suite_end() as i32
}
