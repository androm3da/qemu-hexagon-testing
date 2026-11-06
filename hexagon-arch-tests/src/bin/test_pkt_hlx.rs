// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! HLX in packets (table driven; see pktgen/hlx.rs).

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

use hexagon_arch_tests::*;

#[path = "../pkt/mod.rs"]
mod pkt;

#[no_mangle]
pub extern "C" fn rust_main() -> i32 {
    test_suite_begin("pkt_hlx");
    pkt::run_table(
        "pkt_hlx",
        include_bytes!(concat!(env!("OUT_DIR"), "/test_pkt_hlx.tbl")),
    );
    test_suite_end() as i32
}
