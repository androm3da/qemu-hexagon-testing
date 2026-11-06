// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! HMX in packets (table driven; see pktgen/hmx.rs).

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

use hexagon_arch_tests::*;

#[path = "../pkt/mod.rs"]
mod pkt;

#[no_mangle]
pub extern "C" fn rust_main() -> i32 {
    test_suite_begin("pkt_hmx");
    pkt::run_table(
        "pkt_hmx",
        include_bytes!(concat!(env!("OUT_DIR"), "/test_pkt_hmx.tbl")),
    );
    test_suite_end() as i32
}
