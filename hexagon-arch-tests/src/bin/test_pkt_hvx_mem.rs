// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! HVX memory accesses that miss and replay (table driven; see pktgen/hvx_mem.rs).

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

use hexagon_arch_tests::*;

#[path = "../pkt/mod.rs"]
mod pkt;

#[no_mangle]
pub extern "C" fn rust_main() -> i32 {
    test_suite_begin("pkt_hvx_mem");
    pkt::run_table(
        "pkt_hvx_mem",
        include_bytes!(concat!(env!("OUT_DIR"), "/test_pkt_hvx_mem.tbl")),
    );
    test_suite_end() as i32
}
