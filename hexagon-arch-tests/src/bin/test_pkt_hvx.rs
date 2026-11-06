// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! HVX in packets (table driven; see pktgen/hvx.rs).

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

use hexagon_arch_tests::*;

#[path = "../pkt/mod.rs"]
mod pkt;

#[no_mangle]
pub extern "C" fn rust_main() -> i32 {
    test_suite_begin("pkt_hvx");
    pkt::run_table(
        "pkt_hvx",
        include_bytes!(concat!(env!("OUT_DIR"), "/test_pkt_hvx.tbl")),
    );
    test_suite_end() as i32
}
