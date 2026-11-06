// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! `.new` operands in packets: new-value stores, new-value compare-jumps and
//! `.new` predicates (table driven; see pktgen/newval.rs).

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

use hexagon_arch_tests::*;

#[path = "../pkt/mod.rs"]
mod pkt;

#[no_mangle]
pub extern "C" fn rust_main() -> i32 {
    test_suite_begin("pkt_newval");
    pkt::run_table(
        "pkt_newval",
        include_bytes!(concat!(env!("OUT_DIR"), "/test_pkt_newval.tbl")),
    );
    test_suite_end() as i32
}
