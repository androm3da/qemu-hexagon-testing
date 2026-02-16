use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use crate::comparison::compare_states;
use crate::lldb_script::{generate_qemu_script, generate_sim_script, parse_lldb_output};
use crate::runner::{find_free_port, run_on_qemu, run_on_sim, ToolchainPaths};

/// Minimize a failing test using the ddmin (delta debugging) algorithm.
///
/// Starts with coarse partitions and refines, converging on a 1-minimal set
/// of packets where no single packet can be removed while preserving the failure.
/// This is much faster than one-at-a-time elimination for large inputs.
pub fn minimize_failure(
    toolchain: &ToolchainPaths,
    work_dir: &Path,
    asm_content: &str,
    _isa_version: &str,
) -> Result<MinimizationResult> {
    let all_packet_indices = extract_step_packet_indices(asm_content);

    if all_packet_indices.is_empty() {
        return Ok(MinimizationResult {
            original_packets: 0,
            minimized_packets: 0,
            minimized_asm: asm_content.to_string(),
        });
    }

    let total = all_packet_indices.len();
    let all_packets_set: HashSet<usize> = all_packet_indices.iter().copied().collect();

    // current holds the packet indices still included
    let mut current: Vec<usize> = all_packet_indices;
    let mut n: usize = 2;
    let mut test_count: usize = 0;

    eprintln!("  ddmin: starting with {} packets, n={}", current.len(), n);

    'outer: while current.len() >= 2 {
        let chunks = partition(&current, n);

        // Phase 1: Try reducing to a single chunk (subset)
        for chunk in &chunks {
            let candidate_asm = rebuild_with_packets(asm_content, &all_packets_set, chunk);
            test_count += 1;

            if try_reproduce(toolchain, work_dir, &candidate_asm).unwrap_or(false) {
                eprintln!(
                    "  ddmin: subset hit, {} -> {} packets (test #{})",
                    current.len(),
                    chunk.len(),
                    test_count,
                );
                current = chunk.clone();
                n = 2;
                continue 'outer;
            }
        }

        // Phase 2: Try removing a single chunk (complement)
        for (i, _chunk) in chunks.iter().enumerate() {
            let complement: Vec<usize> = chunks
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .flat_map(|(_, c)| c.iter().copied())
                .collect();
            let candidate_asm = rebuild_with_packets(asm_content, &all_packets_set, &complement);
            test_count += 1;

            if try_reproduce(toolchain, work_dir, &candidate_asm).unwrap_or(false) {
                eprintln!(
                    "  ddmin: complement hit, {} -> {} packets (test #{})",
                    current.len(),
                    complement.len(),
                    test_count,
                );
                current = complement;
                n = n.saturating_sub(1).max(2);
                continue 'outer;
            }
        }

        // Phase 3: Increase granularity
        if n < current.len() {
            n = (2 * n).min(current.len());
            eprintln!("  ddmin: refining granularity to n={}", n);
        } else {
            break; // 1-minimal
        }
    }

    eprintln!(
        "  ddmin: done, {} -> {} packets in {} tests",
        total,
        current.len(),
        test_count,
    );

    let minimized_asm = rebuild_with_packets(asm_content, &all_packets_set, &current);

    Ok(MinimizationResult {
        original_packets: total,
        minimized_packets: current.len(),
        minimized_asm,
    })
}

/// Result of the minimization process.
#[derive(Debug)]
pub struct MinimizationResult {
    pub original_packets: usize,
    pub minimized_packets: usize,
    pub minimized_asm: String,
}

/// Extract line indices of removable packets within the `steps` function.
///
/// A removable packet is any `{ ... }` line between the `steps:` label and
/// the `.globl test_end` / `test_end:` marker.
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

    indices
}

/// Partition `items` into `n` roughly-equal chunks.
fn partition(items: &[usize], n: usize) -> Vec<Vec<usize>> {
    let n = n.min(items.len()).max(1);
    let chunk_size = items.len().div_ceil(n);
    items.chunks(chunk_size).map(|c| c.to_vec()).collect()
}

/// Rebuild assembly keeping only the specified packet lines (and all non-packet lines).
///
/// `all_packet_set` is the full set of packet line indices (computed once).
/// `included` is the subset of packet indices to keep.
fn rebuild_with_packets(asm: &str, all_packet_set: &HashSet<usize>, included: &[usize]) -> String {
    let included_set: HashSet<usize> = included.iter().copied().collect();
    asm.lines()
        .enumerate()
        .filter(|(i, _)| {
            if all_packet_set.contains(i) {
                included_set.contains(i)
            } else {
                true // non-packet lines always kept
            }
        })
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Try to reproduce the failure with the given assembly.
/// Returns Ok(true) if the test still fails (mismatches), Ok(false) if it passes.
fn try_reproduce(toolchain: &ToolchainPaths, work_dir: &Path, asm_content: &str) -> Result<bool> {
    let test_asm = work_dir.join("minimize_test.S");
    let test_elf = work_dir.join("minimize_test.elf");

    std::fs::write(&test_asm, asm_content)?;

    // Build
    let clang = toolchain.hexagon_clang();
    let status = Command::new(&clang)
        .args(["-O2", "-g", "-G0", &format!("-m{}", toolchain.isa_version)])
        .arg(&test_asm)
        .arg("-o")
        .arg(&test_elf)
        .status()
        .with_context(|| format!("Failed to run {}", clang.display()))?;

    if !status.success() {
        anyhow::bail!("Build failed");
    }

    // Run on hexagon-sim (reference)
    let sim_script = generate_sim_script(&["steps", "test_end"]);
    let sim_script_path = work_dir.join("minimize_sim.lldb");
    std::fs::write(&sim_script_path, &sim_script)?;

    let ref_output = run_on_sim(&toolchain.hexagon_lldb(), &sim_script_path, &test_elf)?;

    // Run on QEMU (test)
    let gdb_port = find_free_port()?;
    let qemu_script = generate_qemu_script(&["steps", "test_end"], gdb_port);
    let qemu_script_path = work_dir.join("minimize_qemu.lldb");
    std::fs::write(&qemu_script_path, &qemu_script)?;

    let test_output = run_on_qemu(
        &toolchain.hexagon_lldb(),
        &toolchain.qemu_path,
        &qemu_script_path,
        &test_elf,
        gdb_port,
    )?;

    // Parse and compare register states
    let ref_states = parse_lldb_output(&ref_output);
    let test_states = parse_lldb_output(&test_output);

    let min_len = ref_states.len().min(test_states.len());
    for i in 0..min_len {
        let result = compare_states(&ref_states[i], &test_states[i]);
        if !result.matches {
            return Ok(true); // Mismatch found — failure reproduces
        }
    }

    Ok(false) // No mismatch — failure does not reproduce with this subset
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
        // Keep only first and third packets (exclude r3 = sub)
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
        // 5 items into 2 chunks: [0,1,2], [3,4]
        let items: Vec<usize> = (0..5).collect();
        let chunks = partition(&items, 2);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], vec![0, 1, 2]);
        assert_eq!(chunks[1], vec![3, 4]);

        // 4 items into 4 chunks: [0], [1], [2], [3]
        let items: Vec<usize> = (0..4).collect();
        let chunks = partition(&items, 4);
        assert_eq!(chunks.len(), 4);
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk, &vec![i]);
        }

        // n > len: clamped to len
        let items: Vec<usize> = (0..3).collect();
        let chunks = partition(&items, 10);
        assert_eq!(chunks.len(), 3);

        // 6 items into 3 chunks: [0,1], [2,3], [4,5]
        let items: Vec<usize> = (0..6).collect();
        let chunks = partition(&items, 3);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], vec![0, 1]);
        assert_eq!(chunks[1], vec![2, 3]);
        assert_eq!(chunks[2], vec![4, 5]);
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
        // Only the one packet inside steps, not main's packets
        assert_eq!(indices.len(), 1);
    }
}
