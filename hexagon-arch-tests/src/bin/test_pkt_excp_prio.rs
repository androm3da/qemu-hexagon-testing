// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Several faults in one packet: priority, BADVA/SSR and no partial commit
//! (table driven; see pktgen/excp_prio.rs).

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

use hexagon_arch_tests::*;

#[path = "../pkt/mod.rs"]
mod pkt;

#[no_mangle]
pub extern "C" fn rust_main() -> i32 {
    test_suite_begin("pkt_excp_prio");
    pkt::run_table(
        "pkt_excp_prio",
        include_bytes!(concat!(env!("OUT_DIR"), "/test_pkt_excp_prio.tbl")),
    );
    test_suite_end() as i32
}
