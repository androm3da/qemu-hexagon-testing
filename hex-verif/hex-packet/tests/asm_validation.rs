// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Assembly validation integration test.
//!
//! Synthesizes VLIW packets and validates them with `llvm-mc` (the LLVM
//! machine-code assembler) to measure the false positive rate — packets that
//! hex-packet considers valid but the assembler rejects.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use hex_instset::database::InstructionDb;
use hex_instset::filter::{AttributeFilter, Filter};
use hex_packet::synthesizer::{PacketSynthesizer, SynthConfig};
use rand::rngs::StdRng;
use rand::SeedableRng;

/// Default path to the instruction database.
const INSTRUCTIONS_JSON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../instructions.json");

/// Toolchain root.
const TOOLCHAIN_ROOT: &str = "/pkg/qct/software/hexagon/releases/tools/21.0.01";

/// Categorized assembler error.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum AsmErrorCategory {
    OutOfSlots,
    InvalidNvProducer,
    RegisterModifiedTwice,
    DotNewNotModified,
    TooManyStoresOrLoads,
    SoloViolation,
    SoloAxViolation,
    HvxFeatureRequired,
    FeatureRequired,
    InvalidOperand,
    Other(String),
}

impl std::fmt::Display for AsmErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AsmErrorCategory::OutOfSlots => write!(f, "out of slots"),
            AsmErrorCategory::InvalidNvProducer => write!(f, "invalid NV producer"),
            AsmErrorCategory::RegisterModifiedTwice => write!(f, "register modified twice"),
            AsmErrorCategory::DotNewNotModified => write!(f, ".new not validly modified"),
            AsmErrorCategory::TooManyStoresOrLoads => write!(f, "too many stores/loads"),
            AsmErrorCategory::SoloViolation => write!(f, "solo violation"),
            AsmErrorCategory::SoloAxViolation => write!(f, "soloAX violation"),
            AsmErrorCategory::HvxFeatureRequired => write!(f, "requires HVX"),
            AsmErrorCategory::FeatureRequired => write!(f, "requires feature"),
            AsmErrorCategory::InvalidOperand => write!(f, "invalid operand"),
            AsmErrorCategory::Other(s) => write!(f, "other: {}", s),
        }
    }
}

/// Classify an assembler error line into a category.
fn categorize_error(stderr: &str) -> AsmErrorCategory {
    let lower = stderr.to_lowercase();
    if lower.contains("out of slots")
        || lower.contains("slot") && lower.contains("not available")
        || lower.contains("slot error")
    {
        AsmErrorCategory::OutOfSlots
    } else if lower.contains("not have a valid new register producer")
        || lower.contains("not a valid new register producer")
        || lower.contains("new value register consumer has no producer")
    {
        AsmErrorCategory::InvalidNvProducer
    } else if lower.contains("modified more than once") || lower.contains("register modified") {
        AsmErrorCategory::RegisterModifiedTwice
    } else if lower.contains(".new") && lower.contains("not validly modified") {
        AsmErrorCategory::DotNewNotModified
    } else if lower.contains("too many stores") || lower.contains("too many loads") {
        AsmErrorCategory::TooManyStoresOrLoads
    } else if lower.contains("issolo") || lower.contains("is solo") {
        AsmErrorCategory::SoloViolation
    } else if lower.contains("can only be in a packet with alu") || lower.contains("only with alu")
    {
        AsmErrorCategory::SoloAxViolation
    } else if lower.contains("requires -mhvx")
        || lower.contains("requires hvx")
        || lower.contains("not a recognized processor")
    {
        AsmErrorCategory::HvxFeatureRequired
    } else if lower.contains("instruction requires:") {
        AsmErrorCategory::FeatureRequired
    } else if lower.contains("invalid operand") {
        AsmErrorCategory::InvalidOperand
    } else {
        // Extract first meaningful error line
        let first_err = stderr
            .lines()
            .find(|l| l.contains("error:"))
            .unwrap_or(stderr.lines().next().unwrap_or("unknown"));
        AsmErrorCategory::Other(first_err.trim().to_string())
    }
}

/// Result of assembling a set of packets for one profile.
struct ProfileResult {
    name: String,
    total: usize,
    passed: usize,
    failed: usize,
    synth_failures: usize,
    error_breakdown: HashMap<AsmErrorCategory, usize>,
}

impl ProfileResult {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            total: 0,
            passed: 0,
            failed: 0,
            synth_failures: 0,
            error_breakdown: HashMap::new(),
        }
    }

    fn failure_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.failed as f64 / self.total as f64 * 100.0
    }

    fn merge(&mut self, other: &ProfileResult) {
        self.total += other.total;
        self.passed += other.passed;
        self.failed += other.failed;
        self.synth_failures += other.synth_failures;
        for (cat, count) in &other.error_breakdown {
            *self.error_breakdown.entry(cat.clone()).or_insert(0) += count;
        }
    }
}

/// Find llvm-mc binary.
fn find_llvm_mc() -> Option<PathBuf> {
    let toolchain_mc = PathBuf::from(TOOLCHAIN_ROOT).join("Tools/bin/llvm-mc");
    if toolchain_mc.exists() {
        return Some(toolchain_mc);
    }
    // Try PATH
    let output = Command::new("which").arg("llvm-mc").output().ok()?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    None
}

/// Assemble a batch of packets via a single llvm-mc invocation.
///
/// Each packet is placed on a separate line range with a label marker. Errors
/// in stderr are parsed by line number and mapped back to the originating
/// packet. Returns one `Result` per packet: `Ok(())` if accepted, `Err(stderr
/// fragment)` if rejected.
fn try_assemble_batch(llvm_mc: &Path, packets_asm: &[String]) -> Vec<Result<(), String>> {
    if packets_asm.is_empty() {
        return Vec::new();
    }

    // Build one big assembly blob, tracking the first line of each packet.
    let mut asm = String::from(".text\n");
    let mut packet_start_lines: Vec<usize> = Vec::new();
    let mut current_line: usize = 2; // ".text\n" is line 1

    for pkt in packets_asm {
        packet_start_lines.push(current_line);
        asm.push_str(pkt);
        asm.push('\n');
        current_line += pkt.lines().count();
    }

    // Spawn one llvm-mc for the whole batch.
    let mut child = match Command::new(llvm_mc)
        .args(["-triple=hexagon", "-mcpu=hexagonv73", "-filetype=null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return packets_asm.iter().map(|_| Err(e.to_string())).collect();
        }
    };

    child.stdin.take().unwrap().write_all(asm.as_bytes()).ok();

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            return packets_asm.iter().map(|_| Err(e.to_string())).collect();
        }
    };

    if output.status.success() {
        return packets_asm.iter().map(|_| Ok(())).collect();
    }

    // Parse stderr: lines like `<stdin>:LINE:COL: error: ...`
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut failed_packets: HashMap<usize, String> = HashMap::new();

    for line in stderr.lines() {
        if !line.contains("error:") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("<stdin>:") {
            if let Some(colon_pos) = rest.find(':') {
                if let Ok(err_line) = rest[..colon_pos].parse::<usize>() {
                    // Find which packet owns this line number.
                    let pkt_idx = packet_start_lines
                        .iter()
                        .rposition(|&start| err_line >= start)
                        .unwrap_or(0);
                    failed_packets
                        .entry(pkt_idx)
                        .or_insert_with(|| line.to_string());
                }
            }
        }
    }

    (0..packets_asm.len())
        .map(|i| {
            if let Some(err) = failed_packets.get(&i) {
                Err(err.clone())
            } else {
                Ok(())
            }
        })
        .collect()
}

/// Run a single profile batch: synthesize N packets, assemble them in one
/// llvm-mc invocation, and collect stats.
fn run_profile(
    db: &InstructionDb,
    llvm_mc: &Path,
    name: &str,
    config: SynthConfig,
    count: usize,
    seed: u64,
) -> ProfileResult {
    let synth = PacketSynthesizer::new(db, config);
    let mut rng = StdRng::seed_from_u64(seed);
    let mut result = ProfileResult::new(name);

    // Synthesize all packets first.
    let mut asm_packets: Vec<String> = Vec::with_capacity(count);
    for _ in 0..count {
        match synth.synthesize_packet(&mut rng) {
            Ok(p) => asm_packets.push(p.to_asm()),
            Err(_) => result.synth_failures += 1,
        }
    }

    // Assemble in one batched call.
    let results = try_assemble_batch(llvm_mc, &asm_packets);
    for res in results {
        result.total += 1;
        match res {
            Ok(()) => result.passed += 1,
            Err(stderr) => {
                result.failed += 1;
                let category = categorize_error(&stderr);
                *result.error_breakdown.entry(category).or_insert(0) += 1;
            }
        }
    }

    result
}

fn print_profile_result(r: &ProfileResult) {
    println!("\n--- Profile: {} ---", r.name);
    println!(
        "  Total: {}  Passed: {}  Failed: {}  Failure rate: {:.1}%  (synth failures: {})",
        r.total,
        r.passed,
        r.failed,
        r.failure_rate(),
        r.synth_failures,
    );
    if !r.error_breakdown.is_empty() {
        println!("  Error breakdown:");
        let mut errors: Vec<_> = r.error_breakdown.iter().collect();
        errors.sort_by(|a, b| b.1.cmp(a.1));
        for (cat, count) in errors {
            println!("    {}: {}", cat, count);
        }
    }
}

/// Common blocked features for all scalar profiles.
///
/// Block features not enabled by `-mcpu=hexagonv73`:
/// - HVX: all vector extensions
/// - V81: architecture features beyond v73
/// - ZReg: Z-axis register extensions
/// - Audio: audio DSP extensions
/// - Cabac: CABAC accelerator
/// - Compound: compound instruction set
fn scalar_blocked_features() -> Vec<String> {
    vec![
        "HVX".to_string(),
        "V81".to_string(),
        "ZReg".to_string(),
        "Audio".to_string(),
        "Cabac".to_string(),
        "Compound".to_string(),
    ]
}

fn make_scalar_basic_config() -> SynthConfig {
    SynthConfig {
        allow_predicated: true,
        allow_predicated_new: false,
        allow_new_value: false,
        max_cvi_per_packet: 0,
        blocked_features: scalar_blocked_features(),
        exclude_filters: vec![
            Filter::ByAttribute(AttributeFilter::MayLoad(true)),
            Filter::ByAttribute(AttributeFilter::MayStore(true)),
        ],
        ..SynthConfig::default()
    }
}

fn make_with_memory_config() -> SynthConfig {
    SynthConfig {
        allow_predicated: true,
        allow_predicated_new: false,
        allow_new_value: false,
        max_cvi_per_packet: 0,
        blocked_features: scalar_blocked_features(),
        ..SynthConfig::default()
    }
}

fn make_with_nv_store_config() -> SynthConfig {
    SynthConfig {
        allow_predicated: true,
        allow_predicated_new: false,
        allow_new_value: true,
        max_cvi_per_packet: 0,
        blocked_features: scalar_blocked_features(),
        ..SynthConfig::default()
    }
}

fn make_with_dot_new_config() -> SynthConfig {
    SynthConfig {
        allow_predicated: true,
        allow_predicated_new: true,
        allow_new_value: false,
        max_cvi_per_packet: 0,
        blocked_features: scalar_blocked_features(),
        ..SynthConfig::default()
    }
}

// ---- Small tests (200 packets each, run in normal cargo test) ----

#[test]
fn asm_validation_scalar_basic() {
    let llvm_mc = match find_llvm_mc() {
        Some(c) => c,
        None => {
            eprintln!("SKIP: llvm-mc not found");
            return;
        }
    };

    let db_path = Path::new(INSTRUCTIONS_JSON);
    if !db_path.exists() {
        eprintln!("SKIP: instructions.json not found at {}", db_path.display());
        return;
    }

    let db = InstructionDb::load_from_json(db_path).expect("Failed to load instruction database");
    let result = run_profile(
        &db,
        &llvm_mc,
        "scalar_basic",
        make_scalar_basic_config(),
        200,
        42,
    );
    print_profile_result(&result);

    assert!(
        result.failure_rate() < 5.0,
        "scalar_basic failure rate too high: {:.1}%",
        result.failure_rate()
    );
}

#[test]
fn asm_validation_with_memory() {
    let llvm_mc = match find_llvm_mc() {
        Some(c) => c,
        None => {
            eprintln!("SKIP: llvm-mc not found");
            return;
        }
    };

    let db_path = Path::new(INSTRUCTIONS_JSON);
    if !db_path.exists() {
        eprintln!("SKIP: instructions.json not found at {}", db_path.display());
        return;
    }

    let db = InstructionDb::load_from_json(db_path).expect("Failed to load instruction database");
    let result = run_profile(
        &db,
        &llvm_mc,
        "with_memory",
        make_with_memory_config(),
        200,
        123,
    );
    print_profile_result(&result);

    assert!(
        result.failure_rate() < 10.0,
        "with_memory failure rate too high: {:.1}%",
        result.failure_rate()
    );
}

#[test]
fn asm_validation_with_nv_store() {
    let llvm_mc = match find_llvm_mc() {
        Some(c) => c,
        None => {
            eprintln!("SKIP: llvm-mc not found");
            return;
        }
    };

    let db_path = Path::new(INSTRUCTIONS_JSON);
    if !db_path.exists() {
        eprintln!("SKIP: instructions.json not found at {}", db_path.display());
        return;
    }

    let db = InstructionDb::load_from_json(db_path).expect("Failed to load instruction database");
    let result = run_profile(
        &db,
        &llvm_mc,
        "with_nv_store",
        make_with_nv_store_config(),
        200,
        456,
    );
    print_profile_result(&result);

    assert!(
        result.failure_rate() < 10.0,
        "with_nv_store failure rate too high: {:.1}%",
        result.failure_rate()
    );
}

#[test]
fn asm_validation_with_dot_new() {
    let llvm_mc = match find_llvm_mc() {
        Some(c) => c,
        None => {
            eprintln!("SKIP: llvm-mc not found");
            return;
        }
    };

    let db_path = Path::new(INSTRUCTIONS_JSON);
    if !db_path.exists() {
        eprintln!("SKIP: instructions.json not found at {}", db_path.display());
        return;
    }

    let db = InstructionDb::load_from_json(db_path).expect("Failed to load instruction database");
    let result = run_profile(
        &db,
        &llvm_mc,
        "with_dot_new",
        make_with_dot_new_config(),
        200,
        789,
    );
    print_profile_result(&result);

    assert!(
        result.failure_rate() < 20.0,
        "with_dot_new failure rate too high: {:.1}%",
        result.failure_rate()
    );
}

// ---- Large-scale scoring test (10M packets, run with --ignored) ----

#[test]
#[ignore]
fn asm_validation_large_scale() {
    let llvm_mc = match find_llvm_mc() {
        Some(c) => c,
        None => {
            eprintln!("SKIP: llvm-mc not found");
            return;
        }
    };

    let db_path = Path::new(INSTRUCTIONS_JSON);
    if !db_path.exists() {
        eprintln!("SKIP: instructions.json not found at {}", db_path.display());
        return;
    }

    let db = InstructionDb::load_from_json(db_path).expect("Failed to load instruction database");

    let packets_per_profile = 100_000;
    let batch_size = 2_000;
    let batches_per_profile = packets_per_profile / batch_size;

    let profiles: &[(&str, fn() -> SynthConfig)] = &[
        ("scalar_basic", make_scalar_basic_config),
        ("with_memory", make_with_memory_config),
        ("with_nv_store", make_with_nv_store_config),
        ("with_dot_new", make_with_dot_new_config),
    ];

    let mut grand_total = ProfileResult::new("GRAND TOTAL");

    for &(name, make_config) in profiles {
        let mut combined = ProfileResult::new(name);
        let start = std::time::Instant::now();

        for batch_idx in 0..batches_per_profile {
            let seed = batch_idx as u64 * 1000 + 7;
            let batch = run_profile(&db, &llvm_mc, name, make_config(), batch_size, seed);
            combined.merge(&batch);

            // Print progress every 10 batches (20k packets)
            if (batch_idx + 1) % 10 == 0 {
                let elapsed = start.elapsed();
                let pps = combined.total as f64 / elapsed.as_secs_f64();
                eprintln!(
                    "  [{name}] {}/{} batches, {:.1}% failure rate, {:.0} pkts/s",
                    batch_idx + 1,
                    batches_per_profile,
                    combined.failure_rate(),
                    pps,
                );
            }
        }

        print_profile_result(&combined);
        grand_total.merge(&combined);
    }

    println!("\n=========================================");
    print_profile_result(&grand_total);
    println!("=========================================");
}

/// Diagnostic test: captures instruction details for slot failures.
#[test]
#[ignore]
fn asm_validation_slot_diagnostic() {
    let llvm_mc = match find_llvm_mc() {
        Some(c) => c,
        None => {
            eprintln!("SKIP: llvm-mc not found");
            return;
        }
    };

    let db_path = Path::new(INSTRUCTIONS_JSON);
    if !db_path.exists() {
        eprintln!("SKIP: instructions.json not found at {}", db_path.display());
        return;
    }

    let db = InstructionDb::load_from_json(db_path).expect("Failed to load instruction database");
    // Use with_dot_new to investigate remaining slot errors
    let synth = PacketSynthesizer::new(&db, make_with_dot_new_config());
    let mut rng = StdRng::seed_from_u64(42);
    let mut slot_failures = 0;
    let max_examples = 30;

    for i in 0..5000 {
        let packet = match synth.synthesize_packet(&mut rng) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let asm = packet.to_asm();

        // Assemble individually to get per-packet error
        let asm_content = format!(".text\n{}\n", asm);
        let mut child = Command::new(&llvm_mc)
            .args(["-triple=hexagon", "-mcpu=hexagonv73", "-filetype=null"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(asm_content.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let cat = categorize_error(&stderr);
            if matches!(cat, AsmErrorCategory::OutOfSlots) && slot_failures < max_examples {
                slot_failures += 1;
                println!("\n=== Slot failure #{} (packet {}) ===", slot_failures, i);
                println!("  ASM: {}", asm);
                for ci in &packet.insns {
                    println!(
                        "    {:40} itype={:20} may_load={} may_store={} restrict_no_slot1={} slot1_aok={} is_branch={} prefers_slot3={}",
                        ci.def.name,
                        ci.def.itype,
                        ci.def.may_load,
                        ci.def.may_store,
                        ci.def.is_restrict_no_slot1_store,
                        ci.def.is_restrict_slot1_aok,
                        ci.def.is_branch,
                        ci.def.prefers_slot3,
                    );
                }
                println!(
                    "  ERROR: {}",
                    stderr.lines().find(|l| l.contains("error:")).unwrap_or("?")
                );
            }
        }
    }
    println!("\nTotal slot failures in 5000 packets: {}", slot_failures);
}

/// Diagnostic test: captures instruction details for "invalid operand" failures.
#[test]
#[ignore]
fn asm_validation_operand_diagnostic() {
    let llvm_mc = match find_llvm_mc() {
        Some(c) => c,
        None => {
            eprintln!("SKIP: llvm-mc not found");
            return;
        }
    };

    let db_path = Path::new(INSTRUCTIONS_JSON);
    if !db_path.exists() {
        eprintln!("SKIP: instructions.json not found at {}", db_path.display());
        return;
    }

    let db = InstructionDb::load_from_json(db_path).expect("Failed to load instruction database");

    // Test with_dot_new profile since it has most "invalid operand" errors
    let synth = PacketSynthesizer::new(&db, make_with_dot_new_config());
    let mut rng = StdRng::seed_from_u64(42);
    let mut operand_failures = 0;
    let max_examples = 30;

    for i in 0..5000 {
        let packet = match synth.synthesize_packet(&mut rng) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let asm = packet.to_asm();
        let asm_content = format!(".text\n{}\n", asm);
        let mut child = Command::new(&llvm_mc)
            .args(["-triple=hexagon", "-mcpu=hexagonv73", "-filetype=null"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(asm_content.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let cat = categorize_error(&stderr);
            if matches!(cat, AsmErrorCategory::InvalidOperand) && operand_failures < max_examples {
                operand_failures += 1;
                println!(
                    "\n=== Invalid operand #{} (packet {}) ===",
                    operand_failures, i
                );
                println!("  ASM: {}", asm);
                for ci in &packet.insns {
                    let operand_info: Vec<String> = ci
                        .def
                        .ins
                        .iter()
                        .chain(ci.def.outs.iter())
                        .map(|op| {
                            format!(
                                "{}({}/{})",
                                op.name,
                                op.reg_class.as_deref().unwrap_or("imm"),
                                op.imm_type.as_deref().unwrap_or("?")
                            )
                        })
                        .collect();
                    println!(
                        "    {:40} itype={:20} ops=[{}]",
                        ci.def.name,
                        ci.def.itype,
                        operand_info.join(", "),
                    );
                }
                println!(
                    "  ERROR: {}",
                    stderr.lines().find(|l| l.contains("error:")).unwrap_or("?")
                );
            }
        }
    }
    println!(
        "\nTotal invalid-operand failures in 5000 packets: {}",
        operand_failures
    );
}

/// Diagnostic test: captures instruction details for NV producer failures.
#[test]
#[ignore]
fn asm_validation_nv_diagnostic() {
    let llvm_mc = match find_llvm_mc() {
        Some(c) => c,
        None => {
            eprintln!("SKIP: llvm-mc not found");
            return;
        }
    };

    let db_path = Path::new(INSTRUCTIONS_JSON);
    if !db_path.exists() {
        eprintln!("SKIP: instructions.json not found at {}", db_path.display());
        return;
    }

    let db = InstructionDb::load_from_json(db_path).expect("Failed to load instruction database");
    let synth = PacketSynthesizer::new(&db, make_with_nv_store_config());
    let mut rng = StdRng::seed_from_u64(42);
    let mut nv_failures = 0;
    let max_examples = 20;

    for i in 0..5000 {
        let packet = match synth.synthesize_packet(&mut rng) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let asm = packet.to_asm();
        let asm_content = format!(".text\n{}\n", asm);
        let mut child = Command::new(&llvm_mc)
            .args(["-triple=hexagon", "-mcpu=hexagonv73", "-filetype=null"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(asm_content.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let cat = categorize_error(&stderr);
            if matches!(cat, AsmErrorCategory::InvalidNvProducer) && nv_failures < max_examples {
                nv_failures += 1;
                println!(
                    "\n=== NV producer failure #{} (packet {}) ===",
                    nv_failures, i
                );
                println!("  ASM: {}", asm);
                for ci in &packet.insns {
                    println!(
                        "    {:40} nv_store={} has_new_value={} is_pred={} pred_new={} pred_false={} may_load={} addr_mode={}",
                        ci.def.name,
                        ci.def.is_nv_store,
                        ci.def.has_new_value,
                        ci.def.is_predicated,
                        ci.def.is_predicated_new,
                        ci.def.is_predicated_false,
                        ci.def.may_load,
                        ci.def.addr_mode,
                    );
                    println!("      outs: {:?}", ci.dest_regs);
                    println!("      ins:  {:?}", ci.src_regs);
                }
                println!(
                    "  ERROR: {}",
                    stderr.lines().find(|l| l.contains("error:")).unwrap_or("?")
                );
            }
        }
    }
    println!(
        "\nTotal NV-producer failures in 5000 packets: {}",
        nv_failures
    );
}

/// High-volume diagnostic: uses the same batch assembly approach as the large-scale
/// test but captures detailed info for every "register modified twice" failure.
/// This is needed because the remaining failures are extremely rare (~1 in 67k).
#[test]
#[ignore]
fn asm_validation_regmod_hunt() {
    let llvm_mc = match find_llvm_mc() {
        Some(c) => c,
        None => {
            eprintln!("SKIP: llvm-mc not found");
            return;
        }
    };

    let db_path = Path::new(INSTRUCTIONS_JSON);
    if !db_path.exists() {
        eprintln!("SKIP: instructions.json not found at {}", db_path.display());
        return;
    }

    let db = InstructionDb::load_from_json(db_path).expect("Failed to load instruction database");

    let packets_per_profile = 100_000;
    let batch_size = 2_000;
    let batches_per_profile = packets_per_profile / batch_size;

    let profiles: &[(&str, fn() -> SynthConfig)] = &[
        ("scalar_basic", make_scalar_basic_config),
        ("with_memory", make_with_memory_config),
        ("with_nv_store", make_with_nv_store_config),
        ("with_dot_new", make_with_dot_new_config),
    ];

    let mut total_found = 0;

    for &(name, make_config) in profiles {
        let mut profile_found = 0;

        for batch_idx in 0..batches_per_profile {
            let seed = batch_idx as u64 * 1000 + 7;
            let config = make_config();
            let synth = PacketSynthesizer::new(&db, config);
            let mut rng = StdRng::seed_from_u64(seed);

            // Synthesize batch
            let mut packets = Vec::with_capacity(batch_size);
            for _ in 0..batch_size {
                match synth.synthesize_packet(&mut rng) {
                    Ok(p) => packets.push(p),
                    Err(_) => {}
                }
            }

            // Batch-assemble
            let asm_texts: Vec<String> = packets.iter().map(|p| p.to_asm()).collect();
            let results = try_assemble_batch(&llvm_mc, &asm_texts);

            // Check for failures
            for (i, res) in results.iter().enumerate() {
                if let Err(stderr) = res {
                    let cat = categorize_error(stderr);
                    if matches!(cat, AsmErrorCategory::RegisterModifiedTwice) {
                        profile_found += 1;
                        total_found += 1;
                        let packet = &packets[i];
                        println!(
                            "\n=== [{name}] regmod #{profile_found} (batch {batch_idx}, pkt {i}) ==="
                        );
                        println!("  ASM: {}", packet.to_asm());
                        for ci in &packet.insns {
                            println!(
                                "    {:40} itype={:20} is_pred={} pred_new={} pred_false={} pred_late={} is_fp={} may_load={} may_store={}",
                                ci.def.name,
                                ci.def.itype,
                                ci.def.is_predicated,
                                ci.def.is_predicated_new,
                                ci.def.is_predicated_false,
                                ci.def.is_predicate_late,
                                ci.def.is_fp,
                                ci.def.may_load,
                                ci.def.may_store,
                            );
                            println!("      outs: {:?}", ci.dest_regs);
                            println!(
                                "      out_classes: {:?}",
                                ci.def
                                    .outs
                                    .iter()
                                    .map(|o| o.reg_class.as_deref().unwrap_or("?"))
                                    .collect::<Vec<_>>()
                            );
                            println!("      ins:  {:?}", ci.src_regs);
                        }

                        // Re-assemble individually for the full error
                        let asm_content = format!(".text\n{}\n", packet.to_asm());
                        let mut child = Command::new(&llvm_mc)
                            .args(["-triple=hexagon", "-mcpu=hexagonv73", "-filetype=null"])
                            .stdin(Stdio::piped())
                            .stdout(Stdio::null())
                            .stderr(Stdio::piped())
                            .spawn()
                            .unwrap();
                        child
                            .stdin
                            .take()
                            .unwrap()
                            .write_all(asm_content.as_bytes())
                            .unwrap();
                        let output = child.wait_with_output().unwrap();
                        let full_stderr = String::from_utf8_lossy(&output.stderr);
                        for line in full_stderr.lines() {
                            if line.contains("error:") || line.contains("warning:") {
                                println!("  ERR: {}", line);
                            }
                        }
                    }
                }
            }
        }
        println!("\n[{name}] Total register-modified-twice: {profile_found}");
    }
    println!("\n=== Grand total register-modified-twice: {total_found} ===");
}

/// Diagnostic test: captures instruction details for "register modified twice" failures.
#[test]
#[ignore]
fn asm_validation_regmod_diagnostic() {
    let llvm_mc = match find_llvm_mc() {
        Some(c) => c,
        None => {
            eprintln!("SKIP: llvm-mc not found");
            return;
        }
    };

    let db_path = Path::new(INSTRUCTIONS_JSON);
    if !db_path.exists() {
        eprintln!("SKIP: instructions.json not found at {}", db_path.display());
        return;
    }

    let db = InstructionDb::load_from_json(db_path).expect("Failed to load instruction database");

    let profiles: &[(&str, fn() -> SynthConfig)] = &[
        ("scalar_basic", make_scalar_basic_config),
        ("with_memory", make_with_memory_config),
        ("with_nv_store", make_with_nv_store_config),
        ("with_dot_new", make_with_dot_new_config),
    ];

    for &(name, make_config) in profiles {
        let synth = PacketSynthesizer::new(&db, make_config());
        let mut rng = StdRng::seed_from_u64(42);
        let mut failures = 0;
        let max_examples = 10;

        for i in 0..10_000 {
            let packet = match synth.synthesize_packet(&mut rng) {
                Ok(p) => p,
                Err(_) => continue,
            };

            let asm = packet.to_asm();
            let asm_content = format!(".text\n{}\n", asm);
            let mut child = Command::new(&llvm_mc)
                .args(["-triple=hexagon", "-mcpu=hexagonv73", "-filetype=null"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(asm_content.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let cat = categorize_error(&stderr);
                if matches!(cat, AsmErrorCategory::RegisterModifiedTwice) && failures < max_examples
                {
                    failures += 1;
                    println!(
                        "\n=== [{name}] Register-modified-twice #{} (packet {}) ===",
                        failures, i
                    );
                    println!("  ASM: {}", asm);
                    for ci in &packet.insns {
                        println!(
                            "    {:40} itype={:20} is_pred={} pred_new={} pred_false={} pred_late={} is_fp={}",
                            ci.def.name,
                            ci.def.itype,
                            ci.def.is_predicated,
                            ci.def.is_predicated_new,
                            ci.def.is_predicated_false,
                            ci.def.is_predicate_late,
                            ci.def.is_fp,
                        );
                        println!("      outs: {:?}", ci.dest_regs);
                        println!("      ins:  {:?}", ci.src_regs);
                    }
                    println!(
                        "  ERROR: {}",
                        stderr.lines().find(|l| l.contains("error:")).unwrap_or("?")
                    );
                }
            }
        }
        println!("\n[{name}] Total register-modified-twice in 10000 packets: {failures}");
    }
}

/// Diagnostic test: captures instruction details for ".new not validly modified" failures.
#[test]
#[ignore]
fn asm_validation_dotnew_diagnostic() {
    let llvm_mc = match find_llvm_mc() {
        Some(c) => c,
        None => {
            eprintln!("SKIP: llvm-mc not found");
            return;
        }
    };

    let db_path = Path::new(INSTRUCTIONS_JSON);
    if !db_path.exists() {
        eprintln!("SKIP: instructions.json not found at {}", db_path.display());
        return;
    }

    let db = InstructionDb::load_from_json(db_path).expect("Failed to load instruction database");

    let profiles: &[(&str, fn() -> SynthConfig)] = &[
        ("scalar_basic", make_scalar_basic_config),
        ("with_memory", make_with_memory_config),
        ("with_nv_store", make_with_nv_store_config),
        ("with_dot_new", make_with_dot_new_config),
    ];

    for &(name, make_config) in profiles {
        let synth = PacketSynthesizer::new(&db, make_config());
        let mut rng = StdRng::seed_from_u64(42);
        let mut failures = 0;
        let max_examples = 10;

        for i in 0..10_000 {
            let packet = match synth.synthesize_packet(&mut rng) {
                Ok(p) => p,
                Err(_) => continue,
            };

            let asm = packet.to_asm();
            let asm_content = format!(".text\n{}\n", asm);
            let mut child = Command::new(&llvm_mc)
                .args(["-triple=hexagon", "-mcpu=hexagonv73", "-filetype=null"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(asm_content.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let cat = categorize_error(&stderr);
                if matches!(cat, AsmErrorCategory::DotNewNotModified) && failures < max_examples {
                    failures += 1;
                    println!(
                        "\n=== [{name}] .new-not-modified #{} (packet {}) ===",
                        failures, i
                    );
                    println!("  ASM: {}", asm);
                    for ci in &packet.insns {
                        println!(
                            "    {:40} itype={:20} is_pred={} pred_new={} pred_false={} pred_late={} has_new_val={}",
                            ci.def.name,
                            ci.def.itype,
                            ci.def.is_predicated,
                            ci.def.is_predicated_new,
                            ci.def.is_predicated_false,
                            ci.def.is_predicate_late,
                            ci.def.has_new_value,
                        );
                        println!("      outs: {:?}", ci.dest_regs);
                        println!("      ins:  {:?}", ci.src_regs);
                    }
                    println!(
                        "  ERROR: {}",
                        stderr.lines().find(|l| l.contains("error:")).unwrap_or("?")
                    );
                }
            }
        }
        println!("\n[{name}] Total .new-not-modified in 10000 packets: {failures}");
    }
}
