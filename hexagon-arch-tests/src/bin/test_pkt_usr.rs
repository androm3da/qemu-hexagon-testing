// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! USR sticky overflow and floating-point flags in packets (table driven;
//! see pktgen/usr.rs).

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

use hexagon_arch_tests::*;

#[path = "../pkt/mod.rs"]
mod pkt;

#[no_mangle]
pub extern "C" fn rust_main() -> i32 {
    test_suite_begin("pkt_usr");
    pkt::run_table(
        "pkt_usr",
        include_bytes!(concat!(env!("OUT_DIR"), "/test_pkt_usr.tbl")),
    );
    test_suite_end() as i32
}
