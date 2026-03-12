use std::collections::HashSet;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use hex_dbg::registers::RegisterState;

use crate::comparison::{compare_states, format_diff};
use crate::runner::{resolve_breakpoint_addrs, run_on_qemu, run_on_sim, RunMode, ToolchainPaths};

/// Minimize a failing test using bisection + ddmin + line-level reduction.
///
/// 1. **Bisect** — binary-search by prefix truncation to find the smallest K
///    such that packets\[0..K\] still reproduces the failure (O(log N) runs).
/// 2. **Ddmin** — on the truncated prefix, partition/complement search for a
///    1-minimal subset where no single packet can be removed.
/// 3. **Line-level ddmin** — blindly try removing every line in the assembly
///    (init registers, scaffolding, etc.) to further reduce.
/// 4. **Analyze** — identify the diverging instruction, its PC, and the
///    input register state.
#[allow(clippy::too_many_arguments)]
pub fn minimize_failure(
    toolchain: &ToolchainPaths,
    work_dir: &Path,
    asm_content: &str,
    _isa_version: &str,
    timeout: Duration,
    run_mode: RunMode,
    hvx: bool,
    initial_diff: &str,
) -> Result<MinimizationResult> {
    let all_packet_indices = extract_step_packet_indices(asm_content);

    if all_packet_indices.is_empty() {
        return Ok(MinimizationResult {
            original_packets: 0,
            minimized_packets: 0,
            minimized_asm: asm_content.to_string(),
            diff_text: initial_diff.to_string(),
        });
    }

    let total = all_packet_indices.len();
    let all_packets_set: HashSet<usize> = all_packet_indices.iter().copied().collect();
    let mut test_count: usize = 0;
    let start = Instant::now();

    // Split budget: phases 0+1 get 2/3, phase 2 gets 1/3 (minimum 20s each)
    let packet_budget = (timeout * 2 / 3).max(Duration::from_secs(20));
    let packet_deadline = start + packet_budget;
    let overall_deadline = start + timeout;

    // ── Phase 0: Bisect to find first divergence ──────────────────────
    let truncated = find_first_divergence(
        toolchain,
        work_dir,
        asm_content,
        &all_packets_set,
        &all_packet_indices,
        run_mode,
        hvx,
        packet_deadline,
        &mut test_count,
    );

    let mut current: Vec<usize> = if let Some(k) = truncated {
        eprintln!(
            "  bisect: divergence at packet {} of {} ({} tests)",
            k, total, test_count
        );
        all_packet_indices[..k].to_vec()
    } else {
        all_packet_indices
    };

    // ── Phase 1: Packet-level ddmin ───────────────────────────────────
    let mut n: usize = 2;

    eprintln!("  ddmin: starting with {} packets, n={}", current.len(), n);

    'outer: while current.len() >= 2 {
        if Instant::now() >= packet_deadline {
            eprintln!("  ddmin: timeout after {} tests", test_count);
            break;
        }

        let chunks = partition(&current, n);

        for chunk in &chunks {
            if Instant::now() >= packet_deadline {
                break 'outer;
            }
            let candidate_asm = rebuild_with_packets(asm_content, &all_packets_set, chunk);
            test_count += 1;

            if try_reproduce(toolchain, work_dir, &candidate_asm, run_mode, hvx).unwrap_or(false) {
                eprintln!(
                    "  ddmin: subset {} -> {} (test #{})",
                    current.len(),
                    chunk.len(),
                    test_count,
                );
                current = chunk.clone();
                n = 2;
                continue 'outer;
            }
        }

        for (i, _chunk) in chunks.iter().enumerate() {
            if Instant::now() >= packet_deadline {
                break 'outer;
            }
            let complement: Vec<usize> = chunks
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .flat_map(|(_, c)| c.iter().copied())
                .collect();
            let candidate_asm = rebuild_with_packets(asm_content, &all_packets_set, &complement);
            test_count += 1;

            if try_reproduce(toolchain, work_dir, &candidate_asm, run_mode, hvx).unwrap_or(false) {
                eprintln!(
                    "  ddmin: complement {} -> {} (test #{})",
                    current.len(),
                    complement.len(),
                    test_count,
                );
                current = complement;
                n = n.saturating_sub(1).max(2);
                continue 'outer;
            }
        }

        if n < current.len() {
            n = (2 * n).min(current.len());
        } else {
            break;
        }
    }

    let packet_minimized = rebuild_with_packets(asm_content, &all_packets_set, &current);
    let remaining_packets = extract_step_packet_indices(&packet_minimized).len();
    eprintln!(
        "  packets: {} -> {} ({} tests)",
        total, remaining_packets, test_count,
    );

    // ── Phase 2: Line-level ddmin ─────────────────────────────────────
    // Get the current diff after packet ddmin to anchor line-level reduction.
    // The diff_regs set prevents drifting to unrelated bugs, and max_mismatches
    // prevents removing init registers that create artificial mismatches.
    let current_diff =
        try_reproduce_with_diff(toolchain, work_dir, &packet_minimized, run_mode, hvx)
            .ok()
            .flatten()
            .unwrap_or_else(|| initial_diff.to_string());
    test_count += 1;
    let diff_regs = extract_diff_regs(&current_diff);
    let max_mismatches = diff_regs.len();
    let line_minimized = line_level_minimize(
        toolchain,
        work_dir,
        &packet_minimized,
        run_mode,
        hvx,
        overall_deadline,
        &mut test_count,
        &diff_regs,
        max_mismatches,
    );
    let final_lines = line_minimized.lines().count();
    let packet_lines = packet_minimized.lines().count();
    if final_lines < packet_lines {
        eprintln!(
            "  lines: {} -> {} ({} tests)",
            packet_lines, final_lines, test_count,
        );
    }

    // ── Phase 3: Analyze divergence ───────────────────────────────────
    let analysis = analyze_divergence(
        toolchain,
        work_dir,
        &line_minimized,
        run_mode,
        hvx,
        &mut test_count,
    );

    // Get final diff from the minimized assembly
    let final_diff = try_reproduce_with_diff(toolchain, work_dir, &line_minimized, run_mode, hvx)
        .ok()
        .flatten()
        .unwrap_or_else(|| initial_diff.to_string());

    // Get disassembly of steps function
    let disasm = get_steps_disassembly(toolchain, work_dir, &line_minimized, hvx);

    // Format header
    let header = format_asm_header(&final_diff, &analysis, disasm.as_deref());
    let minimized_asm = format!("{}\n{}", header, line_minimized);

    let minimized_packets = extract_step_packet_indices(&line_minimized).len();

    eprintln!("  done ({} tests)", test_count);

    Ok(MinimizationResult {
        original_packets: total,
        minimized_packets,
        minimized_asm,
        diff_text: final_diff,
    })
}

/// Result of the minimization process.
#[derive(Debug)]
pub struct MinimizationResult {
    pub original_packets: usize,
    pub minimized_packets: usize,
    pub minimized_asm: String,
    pub diff_text: String,
}

/// Info about the exact divergence point within the minimized test.
struct DivergenceInfo {
    /// 1-indexed line number in the minimized assembly.
    line_number: usize,
    /// The assembly text of the diverging packet.
    instruction_text: String,
    /// Register state just before the diverging packet (reference/sim values).
    /// Only registers that appear in the final diff are included.
    pre_state: Vec<(String, String)>,
}

// ── Minimization helpers ──────────────────────────────────────────────

/// Line-level ddmin: try removing every line in the assembly.
///
/// This goes beyond packet-level reduction — it can strip unnecessary
/// register initializations, callee-save pairs, data directives, etc.
/// Any line whose removal breaks the build or stops the *same* mismatch
/// from reproducing is kept; everything else is removed.
///
/// `diff_regs` contains the register names from the current mismatch;
/// `max_mismatches` is the current number of mismatching registers.
/// A candidate is only accepted if it still mismatches on at least one
/// of these registers AND doesn't increase the total mismatch count
/// (preventing drift to unrelated bugs or init-register artifacts).
#[allow(clippy::too_many_arguments)]
fn line_level_minimize(
    toolchain: &ToolchainPaths,
    work_dir: &Path,
    asm: &str,
    run_mode: RunMode,
    hvx: bool,
    deadline: Instant,
    test_count: &mut usize,
    diff_regs: &HashSet<String>,
    max_mismatches: usize,
) -> String {
    let lines: Vec<&str> = asm.lines().collect();
    let n = lines.len();
    if n <= 2 {
        return asm.to_string();
    }

    // All line indices are candidates for removal
    let mut current: Vec<usize> = (0..n).collect();
    let all_set: HashSet<usize> = current.iter().copied().collect();
    let mut granularity: usize = 2;

    'outer: loop {
        if Instant::now() >= deadline || current.len() <= 2 {
            break;
        }

        let chunks = partition(&current, granularity);

        // Try removing each chunk (complement = keep everything except this chunk)
        for (i, _) in chunks.iter().enumerate() {
            if Instant::now() >= deadline {
                break 'outer;
            }
            let complement: Vec<usize> = chunks
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .flat_map(|(_, c)| c.iter().copied())
                .collect();
            let candidate = rebuild_from_lines(&lines, &all_set, &complement);
            *test_count += 1;

            if try_reproduce_same_bug(
                toolchain,
                work_dir,
                &candidate,
                run_mode,
                hvx,
                diff_regs,
                max_mismatches,
            ) {
                current = complement;
                granularity = 2;
                continue 'outer;
            }
        }

        // No chunk removal succeeded — increase granularity
        {
            if granularity < current.len() {
                granularity = (2 * granularity).min(current.len());
            } else {
                break;
            }
        }
    }

    rebuild_from_lines(&lines, &all_set, &current)
}

/// Rebuild assembly from a subset of line indices.
fn rebuild_from_lines(all_lines: &[&str], _all_set: &HashSet<usize>, keep: &[usize]) -> String {
    let keep_set: HashSet<usize> = keep.iter().copied().collect();
    all_lines
        .iter()
        .enumerate()
        .filter(|(i, _)| keep_set.contains(i))
        .map(|(_, line)| *line)
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Divergence analysis ───────────────────────────────────────────────

/// Identify which packet in the minimized assembly causes the first divergence.
///
/// Bisects the remaining packets in `steps:` to find the smallest prefix
/// that still fails, then captures the pre-execution register state.
fn analyze_divergence(
    toolchain: &ToolchainPaths,
    work_dir: &Path,
    minimized_asm: &str,
    run_mode: RunMode,
    hvx: bool,
    test_count: &mut usize,
) -> Option<DivergenceInfo> {
    let packet_indices = extract_step_packet_indices(minimized_asm);
    if packet_indices.is_empty() {
        return None;
    }

    let all_set: HashSet<usize> = packet_indices.iter().copied().collect();
    let lines: Vec<&str> = minimized_asm.lines().collect();

    // Single packet: it's trivially the divergence point
    if packet_indices.len() == 1 {
        let line_idx = packet_indices[0];
        return Some(DivergenceInfo {
            line_number: line_idx + 1,
            instruction_text: lines.get(line_idx).unwrap_or(&"").trim().to_string(),
            pre_state: Vec::new(),
        });
    }

    // Bisect: find smallest k such that packets[0..k] reproduces failure
    let mut lo: usize = 1;
    let mut hi: usize = packet_indices.len();
    let mut best: usize = packet_indices.len();

    while lo < hi {
        let mid = (lo + hi) / 2;
        let prefix = &packet_indices[..mid];
        let candidate = rebuild_with_packets(minimized_asm, &all_set, prefix);
        *test_count += 1;

        if try_reproduce(toolchain, work_dir, &candidate, run_mode, hvx).unwrap_or(false) {
            best = mid;
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }

    let diverge_pkt_idx = best.saturating_sub(1); // 0-indexed into packet_indices
    let diverge_line_idx = packet_indices[diverge_pkt_idx];
    let instruction_text = lines
        .get(diverge_line_idx)
        .unwrap_or(&"")
        .trim()
        .to_string();

    // Capture pre-state: run with all packets BEFORE the diverging one
    let pre_state = if diverge_pkt_idx > 0 {
        let prefix = &packet_indices[..diverge_pkt_idx];
        let candidate = rebuild_with_packets(minimized_asm, &all_set, prefix);
        *test_count += 1;
        get_pre_state_registers(toolchain, work_dir, &candidate, run_mode, hvx)
    } else {
        // Divergence at the very first packet; get state from running with 0 steps packets.
        // The init function sets up the state, so running with no steps gives us that.
        let candidate = rebuild_with_packets(minimized_asm, &all_set, &[]);
        *test_count += 1;
        get_pre_state_registers(toolchain, work_dir, &candidate, run_mode, hvx)
    };

    Some(DivergenceInfo {
        line_number: diverge_line_idx + 1,
        instruction_text,
        pre_state,
    })
}

/// Run the assembly and return the reference (sim) register state.
/// Used to capture the pre-execution state before the diverging packet.
fn get_pre_state_registers(
    toolchain: &ToolchainPaths,
    work_dir: &Path,
    asm_content: &str,
    run_mode: RunMode,
    hvx: bool,
) -> Vec<(String, String)> {
    let states = match build_and_run(toolchain, work_dir, asm_content, run_mode, hvx) {
        Ok((ref_states, _)) => ref_states,
        Err(_) => return Vec::new(),
    };

    // Take the last state (the register dump at test_end)
    match states.last() {
        Some(state) => state
            .registers
            .iter()
            .map(|(name, val)| (name.clone(), val.clone()))
            .collect(),
        None => Vec::new(),
    }
}

/// Get the `steps` disassembly by building the minimized ELF and running objdump.
fn get_steps_disassembly(
    toolchain: &ToolchainPaths,
    work_dir: &Path,
    asm_content: &str,
    hvx: bool,
) -> Option<String> {
    let test_asm = work_dir.join("disasm_test.S");
    let test_helper = work_dir.join("dump_helper.c");
    let test_elf = work_dir.join("disasm_test.elf");

    std::fs::write(&test_asm, asm_content).ok()?;
    std::fs::write(&test_helper, hex_prog::template::DUMP_HELPER_C).ok()?;

    let clang = toolchain.hexagon_clang();
    let mut cmd = Command::new(&clang);
    cmd.args(["-O0", "-G0", &format!("-m{}", toolchain.isa_version)]);
    if hvx {
        cmd.arg("-mhvx");
    }
    cmd.arg(&test_asm).arg(&test_helper);
    if needs_pageable_helper(asm_content) {
        let pageable_helper = work_dir.join("pageable_helper.c");
        std::fs::write(&pageable_helper, hex_prog::template::PAGEABLE_HELPER_C).ok()?;
        cmd.arg(&pageable_helper);
    }
    let status = cmd.arg("-o").arg(&test_elf).status().ok()?;
    if !status.success() {
        return None;
    }

    let objdump = toolchain
        .toolchain_root
        .join("Tools/bin/hexagon-llvm-objdump");
    let output = Command::new(&objdump)
        .args(["-d", "--no-show-raw-insn"])
        .arg(&test_elf)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let full = String::from_utf8_lossy(&output.stdout);

    // Extract from <steps>: to <test_end>:, falling back to <body>: if no <steps>:
    let mut in_section = false;
    let mut disasm_lines = Vec::new();
    for line in full.lines() {
        if line.contains("<steps>:") || (!in_section && line.contains("<body>:")) {
            in_section = true;
        }
        if in_section {
            if line.contains("<test_end>:") {
                break;
            }
            disasm_lines.push(line);
        }
    }

    if disasm_lines.is_empty() {
        None
    } else {
        Some(disasm_lines.join("\n"))
    }
}

// ── Build/run helpers ─────────────────────────────────────────────────

/// Check if assembly uses pageable memory (needs pageable_helper.c linked).
fn needs_pageable_helper(asm_content: &str) -> bool {
    asm_content.contains("pageable_backing")
}

/// Build the assembly and run on both backends, returning raw register states.
fn build_and_run(
    toolchain: &ToolchainPaths,
    work_dir: &Path,
    asm_content: &str,
    run_mode: RunMode,
    hvx: bool,
) -> Result<(Vec<RegisterState>, Vec<RegisterState>)> {
    let test_asm = work_dir.join("minimize_test.S");
    let test_helper = work_dir.join("dump_helper.c");
    let test_elf = work_dir.join("minimize_test.elf");

    std::fs::write(&test_asm, asm_content)?;
    std::fs::write(&test_helper, hex_prog::template::DUMP_HELPER_C)?;

    let clang = toolchain.hexagon_clang();
    let mut cmd = Command::new(&clang);
    cmd.args(["-O0", "-G0", &format!("-m{}", toolchain.isa_version)]);
    if hvx {
        cmd.arg("-mhvx");
    }
    cmd.arg(&test_asm).arg(&test_helper);
    if needs_pageable_helper(asm_content) {
        let pageable_helper = work_dir.join("pageable_helper.c");
        std::fs::write(&pageable_helper, hex_prog::template::PAGEABLE_HELPER_C)?;
        cmd.arg(&pageable_helper);
    }
    let status = cmd
        .arg("-o")
        .arg(&test_elf)
        .status()
        .with_context(|| format!("Failed to run {}", clang.display()))?;

    if !status.success() {
        anyhow::bail!("Build failed");
    }

    match run_mode {
        RunMode::Direct => {
            let ref_states =
                crate::runner::run_direct_sim_pub(&toolchain.hexagon_sim(), &test_elf, hvx)?;
            let test_states =
                crate::runner::run_direct_qemu_pub(&toolchain.qemu_path, &test_elf, hvx)?;
            Ok((ref_states, test_states))
        }
        RunMode::Gdb => {
            let breakpoint_addrs = resolve_breakpoint_addrs(&test_elf, run_mode)?;
            let ref_states = run_on_sim(&toolchain.hexagon_sim(), &test_elf, &breakpoint_addrs)?;
            let test_states = run_on_qemu(&toolchain.qemu_path, &test_elf, &breakpoint_addrs)?;
            Ok((ref_states, test_states))
        }
    }
}

/// Build and run, returning `Some(diff_text)` on mismatch, `None` if states match.
fn try_reproduce_with_diff(
    toolchain: &ToolchainPaths,
    work_dir: &Path,
    asm_content: &str,
    run_mode: RunMode,
    hvx: bool,
) -> Result<Option<String>> {
    let (ref_states, test_states) = build_and_run(toolchain, work_dir, asm_content, run_mode, hvx)?;

    let min_len = ref_states.len().min(test_states.len());
    let mut diff_parts = Vec::new();
    for i in 0..min_len {
        let result = compare_states(&ref_states[i], &test_states[i]);
        if !result.matches {
            diff_parts.push(format_diff(&result));
        }
    }

    if diff_parts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(diff_parts.join("\n")))
    }
}

/// Returns `Ok(true)` if the test still fails (mismatches), `Ok(false)` if it passes.
fn try_reproduce(
    toolchain: &ToolchainPaths,
    work_dir: &Path,
    asm_content: &str,
    run_mode: RunMode,
    hvx: bool,
) -> Result<bool> {
    Ok(try_reproduce_with_diff(toolchain, work_dir, asm_content, run_mode, hvx)?.is_some())
}

/// Like `try_reproduce`, but only considers the mismatch reproduced if:
/// 1. At least one register from `required_regs` is still mismatching
/// 2. The total mismatch count doesn't exceed `max_mismatches`
///
/// This prevents the minimizer from drifting to an unrelated bug or
/// creating artificial mismatches (e.g. from removing init registers that
/// mask sim-vs-QEMU register initialization differences).
fn try_reproduce_same_bug(
    toolchain: &ToolchainPaths,
    work_dir: &Path,
    asm_content: &str,
    run_mode: RunMode,
    hvx: bool,
    required_regs: &HashSet<String>,
    max_mismatches: usize,
) -> bool {
    match try_reproduce_with_diff(toolchain, work_dir, asm_content, run_mode, hvx) {
        Ok(Some(new_diff)) => {
            let new_regs = extract_diff_regs(&new_diff);
            !new_regs.is_disjoint(required_regs) && new_regs.len() <= max_mismatches
        }
        _ => false,
    }
}

/// Extract register names from a diff string (lines matching "  <reg> : ref=... test=...").
fn extract_diff_regs(diff_text: &str) -> HashSet<String> {
    diff_text
        .lines()
        .filter_map(|l| {
            let trimmed = l.trim();
            if trimmed.contains(" : ") {
                trimmed.split_whitespace().next().map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect()
}

// ── Packet/line manipulation ──────────────────────────────────────────

/// Extract line indices of removable packets within the `steps` function.
///
/// Primary: look for packets between `steps:` and `test_end:`.
/// Fallback: if `steps:` is missing (e.g. after line-level ddmin), look
/// for packets between `body:` and `test_end:`.
fn extract_step_packet_indices(asm: &str) -> Vec<usize> {
    let mut indices = Vec::new();
    let mut in_steps = false;

    for (i, line) in asm.lines().enumerate() {
        let trimmed = line.trim();

        if trimmed == "steps:" {
            in_steps = true;
            continue;
        }

        if in_steps && (trimmed.starts_with(".globl test_end") || trimmed == "test_end:") {
            break;
        }

        if in_steps && trimmed.starts_with('{') && trimmed.ends_with('}') {
            indices.push(i);
        }
    }

    if !indices.is_empty() {
        return indices;
    }

    // Fallback: look between `body:` and `test_end:`
    let mut in_body = false;
    for (i, line) in asm.lines().enumerate() {
        let trimmed = line.trim();

        if trimmed == "body:" {
            in_body = true;
            continue;
        }

        if in_body && (trimmed.starts_with(".globl test_end") || trimmed == "test_end:") {
            break;
        }

        if in_body && trimmed.starts_with('{') && trimmed.ends_with('}') {
            indices.push(i);
        }
    }

    indices
}

/// Partition `items` into `n` roughly-equal chunks.
fn partition(items: &[usize], n: usize) -> Vec<Vec<usize>> {
    let n = n.min(items.len()).max(1);
    let chunk_size = items.len().div_ceil(n);
    items.chunks(chunk_size).map(|c| c.to_vec()).collect()
}

/// Rebuild assembly keeping only the specified packet lines (and all non-packet lines).
fn rebuild_with_packets(asm: &str, all_packet_set: &HashSet<usize>, included: &[usize]) -> String {
    let included_set: HashSet<usize> = included.iter().copied().collect();
    asm.lines()
        .enumerate()
        .filter(|(i, _)| {
            if all_packet_set.contains(i) {
                included_set.contains(i)
            } else {
                true
            }
        })
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Binary-search by prefix truncation to find the first divergence point.
#[allow(clippy::too_many_arguments)]
fn find_first_divergence(
    toolchain: &ToolchainPaths,
    work_dir: &Path,
    asm_content: &str,
    all_packets_set: &HashSet<usize>,
    packet_indices: &[usize],
    run_mode: RunMode,
    hvx: bool,
    deadline: Instant,
    test_count: &mut usize,
) -> Option<usize> {
    let n = packet_indices.len();
    if n <= 2 {
        return None;
    }

    let mut lo: usize = 1;
    let mut hi: usize = n;
    let mut best: usize = n;

    while lo < hi {
        if Instant::now() >= deadline {
            break;
        }
        let mid = (lo + hi) / 2;
        let prefix = &packet_indices[..mid];
        let candidate = rebuild_with_packets(asm_content, all_packets_set, prefix);
        *test_count += 1;

        if try_reproduce(toolchain, work_dir, &candidate, run_mode, hvx).unwrap_or(false) {
            best = mid;
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }

    if best < n {
        Some(best)
    } else {
        None
    }
}

// ── Output formatting ─────────────────────────────────────────────────

/// Format the header for the minimized assembly file.
fn format_asm_header(
    diff_text: &str,
    analysis: &Option<DivergenceInfo>,
    disasm: Option<&str>,
) -> String {
    let mut h = String::new();
    h.push_str("//\n");

    // Mismatch summary
    for line in diff_text.lines() {
        h.push_str(&format!("// {}\n", line));
    }

    // Divergence point
    if let Some(info) = analysis {
        h.push_str("//\n");
        h.push_str(&format!("// Divergence at line {}:\n", info.line_number));
        h.push_str(&format!("//   {}\n", info.instruction_text));

        // Pre-state (registers involved in the diff)
        if !info.pre_state.is_empty() {
            // Extract register names from the diff to filter pre-state
            let diff_regs: HashSet<&str> = diff_text
                .lines()
                .filter_map(|l| {
                    let trimmed = l.trim();
                    if trimmed.contains(" : ") {
                        trimmed.split_whitespace().next()
                    } else {
                        None
                    }
                })
                .collect();

            let relevant: Vec<_> = info
                .pre_state
                .iter()
                .filter(|(name, _)| diff_regs.contains(name.as_str()))
                .collect();

            if !relevant.is_empty() {
                h.push_str("//\n");
                h.push_str("// Input state (sim, before divergence):\n");
                for (name, val) in &relevant {
                    h.push_str(&format!("//   {} = {}\n", name, val));
                }
            }
        }
    }

    // Disassembly
    if let Some(disasm) = disasm {
        h.push_str("//\n");
        h.push_str("// Disassembly (steps):\n");
        for line in disasm.lines() {
            h.push_str(&format!("// {}\n", line));
        }
    }

    h.push_str("//");
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_step_packet_indices() {
        let asm = "\
.text
steps:
    { r0 = add(r1,r2) }
    { r3 = sub(r4,r5) }
    { r6 = and(r7,r8) }
.globl test_end
test_end:
    { jumpr r31 }
.data";

        let indices = extract_step_packet_indices(asm);
        assert_eq!(indices.len(), 3);
    }

    #[test]
    fn test_rebuild_with_packets_subset() {
        let asm = "\
.text
steps:
    { r0 = add(r1,r2) }
    { r3 = sub(r4,r5) }
    { r6 = and(r7,r8) }
.globl test_end
test_end:
    { jumpr r31 }
.data";

        let indices = extract_step_packet_indices(asm);
        let all_set: HashSet<usize> = indices.iter().copied().collect();
        let included = vec![indices[0], indices[2]];
        let rebuilt = rebuild_with_packets(asm, &all_set, &included);
        assert!(rebuilt.contains("r0 = add"));
        assert!(!rebuilt.contains("r3 = sub"));
        assert!(rebuilt.contains("r6 = and"));
        assert!(rebuilt.contains("test_end:"));
    }

    #[test]
    fn test_rebuild_with_packets_none() {
        let asm = "\
steps:
    { r0 = add(r1,r2) }
.globl test_end
test_end:
    { jumpr r31 }";

        let indices = extract_step_packet_indices(asm);
        let all_set: HashSet<usize> = indices.iter().copied().collect();
        let rebuilt = rebuild_with_packets(asm, &all_set, &[]);
        assert!(!rebuilt.contains("r0 = add"));
        assert!(rebuilt.contains("test_end:"));
    }

    #[test]
    fn test_partition() {
        let items: Vec<usize> = (0..5).collect();
        let chunks = partition(&items, 2);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], vec![0, 1, 2]);
        assert_eq!(chunks[1], vec![3, 4]);

        let items: Vec<usize> = (0..4).collect();
        let chunks = partition(&items, 4);
        assert_eq!(chunks.len(), 4);
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk, &vec![i]);
        }

        let items: Vec<usize> = (0..3).collect();
        let chunks = partition(&items, 10);
        assert_eq!(chunks.len(), 3);

        let items: Vec<usize> = (0..6).collect();
        let chunks = partition(&items, 3);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], vec![0, 1]);
        assert_eq!(chunks[1], vec![2, 3]);
        assert_eq!(chunks[2], vec![4, 5]);
    }

    #[test]
    fn test_format_asm_header() {
        let diff = "MISMATCH: 1 register(s) differ:\n  r5 : ref=0x42 test=0x0";
        let header = format_asm_header(diff, &None, None);
        assert!(header.contains("// MISMATCH: 1 register(s) differ:"));
        assert!(header.contains("//   r5 : ref=0x42 test=0x0"));
    }

    #[test]
    fn test_format_asm_header_with_divergence() {
        let diff = "MISMATCH: 1 register(s) differ:\n  r4 : ref=0xff test=0x0";
        let info = DivergenceInfo {
            line_number: 42,
            instruction_text: "{ r4 = sxtb(r11) }".to_string(),
            pre_state: vec![
                ("r4".to_string(), "0x00000000".to_string()),
                ("r11".to_string(), "0xffffffff".to_string()),
            ],
        };
        let header = format_asm_header(diff, &Some(info), None);
        assert!(header.contains("// Divergence at line 42:"));
        assert!(header.contains("//   { r4 = sxtb(r11) }"));
        assert!(header.contains("// Input state"));
        assert!(header.contains("//   r4 = 0x00000000"));
        // r11 not in diff, so should not appear in pre-state section
        assert!(!header.contains("r11 = 0x"));
    }

    #[test]
    fn test_rebuild_from_lines() {
        let lines = vec!["aaa", "bbb", "ccc", "ddd"];
        let all_set: HashSet<usize> = (0..4).collect();
        let keep = vec![0, 2, 3];
        let result = rebuild_from_lines(&lines, &all_set, &keep);
        assert_eq!(result, "aaa\nccc\nddd");
    }

    #[test]
    fn test_extract_ignores_non_steps() {
        let asm = "\
main:
    { allocframe(#64) }
    { call init }
steps:
    { r0 = add(r1,r2) }
.globl test_end
test_end:
    { jumpr r31 }";

        let indices = extract_step_packet_indices(asm);
        assert_eq!(indices.len(), 1);
    }

    #[test]
    fn test_extract_body_fallback() {
        // When steps: is missing, fall back to body: -> test_end:
        let asm = "\
main:
    { call body }
body:
    { r0 = add(r1,r2) }
    { r3 = sub(r4,r5) }
.globl test_end
test_end:
    { jumpr r31 }";

        let indices = extract_step_packet_indices(asm);
        assert_eq!(indices.len(), 2);
    }
}
