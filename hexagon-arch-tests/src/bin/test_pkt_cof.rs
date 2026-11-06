// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Change of flow in packets: dual jumps, compound compare-jumps, calls,
//! frames and hardware loops (table driven; see pktgen/cof.rs).

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

use hexagon_arch_tests::*;

#[path = "../pkt/mod.rs"]
mod pkt;

#[no_mangle]
pub extern "C" fn rust_main() -> i32 {
    test_suite_begin("pkt_cof");
    pkt::run_table(
        "pkt_cof",
        include_bytes!(concat!(env!("OUT_DIR"), "/test_pkt_cof.tbl")),
    );
    test_suite_end() as i32
}
