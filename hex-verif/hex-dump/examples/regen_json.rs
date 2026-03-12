// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Regenerate instructions.json from the tablegen source.
//!
//! Usage:
//!   cargo run --example regen_json -- <path-to-HexagonDepInstrInfo.td> [output.json]

fn main() {
    let td_path = std::env::args()
        .nth(1)
        .expect("Usage: regen_json <path-to-tablegen> [output.json]");
    let out_path = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "instructions.json".to_string());
    let content = std::fs::read_to_string(&td_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", td_path, e));
    let dump =
        hex_dump::parser::parse_tablegen(&content).unwrap_or_else(|e| panic!("Parse error: {}", e));
    let json = serde_json::to_string_pretty(&dump).expect("Failed to serialize");
    std::fs::write(&out_path, json.as_bytes()).expect("Failed to write");
    eprintln!(
        "Wrote {} instructions to {}",
        dump.instructions.len(),
        out_path
    );
}
