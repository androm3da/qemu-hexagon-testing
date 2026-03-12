// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Pre-packet register setup for load/store instructions.
//!
//! When a synthesized packet contains memory operations, the base registers
//! must point into `mem_region` to avoid faults. This module scans a packet
//! for memory dependencies and emits setup packets that initialize base
//! registers to valid addresses.

use hex_packet::synthesizer::Packet;
use rand::prelude::*;
use rand::rngs::StdRng;

/// A register that must hold a specific kind of value before the packet executes.
#[derive(Debug, Clone)]
pub struct PreDependency {
    /// The register name, e.g. "r5".
    pub register: String,
    /// What kind of value the register needs.
    pub kind: SetupKind,
}

/// The kind of setup value a register needs.
#[derive(Debug, Clone)]
pub enum SetupKind {
    /// The register must point into `mem_region`. `max_offset` is the largest
    /// immediate offset used with this base register, so we ensure
    /// `base + max_offset` stays within bounds.
    MemoryAddress { max_offset: i64 },
}

/// Scan a packet for load/store instructions and extract the base registers
/// that need to be initialized.
pub fn find_pre_dependencies(packet: &Packet) -> Vec<PreDependency> {
    let mut deps = Vec::new();
    let mut seen_regs = Vec::new();

    for ci in &packet.insns {
        if !ci.def.may_load && !ci.def.may_store {
            continue;
        }

        // The base register is the first IntRegs source operand
        let base_reg = ci.def.ins.iter().find_map(|op| {
            if !op.is_immediate && op.reg_class.as_deref() == Some("IntRegs") {
                // Find the corresponding concrete register name
                let pattern = format!("${}", op.name);
                // The asm_text has already been resolved, so find which src_reg
                // corresponds to this operand by position
                None::<&str>
                    .or_else(|| {
                        // Match by operand order in ins
                        let reg_idx = ci
                            .def
                            .ins
                            .iter()
                            .filter(|o| !o.is_immediate)
                            .position(|o| o.name == op.name)?;
                        ci.src_regs.get(reg_idx).map(|s| s.as_str())
                    })
                    .or_else(|| {
                        // Fallback: check if pattern was replaced in asm
                        let _ = pattern;
                        ci.src_regs.first().map(|s| s.as_str())
                    })
            } else {
                None
            }
        });

        if let Some(reg) = base_reg {
            if seen_regs.contains(&reg.to_string()) {
                continue;
            }
            seen_regs.push(reg.to_string());

            // Find the largest immediate offset used with this instruction
            let max_offset = ci
                .immediates
                .iter()
                .map(|v| v.unsigned_abs())
                .max()
                .unwrap_or(0) as i64;

            deps.push(PreDependency {
                register: reg.to_string(),
                kind: SetupKind::MemoryAddress { max_offset },
            });
        }
    }

    deps
}

/// Generate setup packets that initialize base registers to point into
/// a memory region. Each setup packet is a single-instruction packet:
/// `{ rN = add(<base_reg>, #<offset>) }` where `base_reg` holds the
/// region base address (e.g. `"r27"` for `mem_region`, `"r26"` for
/// the pageable region).
///
/// `mem_size` is the total size of the target region (e.g. 65536).
pub fn gen_setup_packets(
    deps: &[PreDependency],
    rng: &mut StdRng,
    mem_size: u64,
    base_reg: &str,
) -> Vec<String> {
    let mut packets = Vec::new();

    for dep in deps {
        match dep.kind {
            SetupKind::MemoryAddress { max_offset } => {
                // Ensure base + max_offset < mem_size, with 8-byte alignment
                let headroom = max_offset.unsigned_abs().max(256);
                let usable = mem_size.saturating_sub(headroom);
                let aligned = (usable / 8) * 8;
                let offset = if aligned > 0 {
                    (rng.gen::<u64>() % aligned) & !7
                } else {
                    0
                };
                // Emit: { rN = add(r27, #offset) }
                // For offsets that fit in a small immediate, use add.
                // For larger offsets, use constant extender.
                if offset == 0 {
                    packets.push(format!("    {{ {} = {} }}", dep.register, base_reg));
                } else if offset <= 0x7FFF {
                    packets.push(format!(
                        "    {{ {} = add({},#{}) }}",
                        dep.register, base_reg, offset
                    ));
                } else {
                    packets.push(format!(
                        "    {{ {} = add({},##{}) }}",
                        dep.register, base_reg, offset
                    ));
                }
            }
        }
    }

    packets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gen_setup_packets_empty() {
        let mut rng = StdRng::seed_from_u64(42);
        let packets = gen_setup_packets(&[], &mut rng, 65536, "r27");
        assert!(packets.is_empty());
    }

    #[test]
    fn test_gen_setup_packets_single() {
        let mut rng = StdRng::seed_from_u64(42);
        let deps = vec![PreDependency {
            register: "r5".to_string(),
            kind: SetupKind::MemoryAddress { max_offset: 64 },
        }];
        let packets = gen_setup_packets(&deps, &mut rng, 65536, "r27");
        assert_eq!(packets.len(), 1);
        assert!(packets[0].contains("r5"));
        assert!(packets[0].contains("r27"));
    }

    #[test]
    fn test_gen_setup_packets_zero_offset() {
        let mut rng = StdRng::seed_from_u64(0);
        let deps = vec![PreDependency {
            register: "r3".to_string(),
            kind: SetupKind::MemoryAddress { max_offset: 100000 },
        }];
        // With max_offset close to mem_size, offset should be small or zero
        let packets = gen_setup_packets(&deps, &mut rng, 65536, "r27");
        assert_eq!(packets.len(), 1);
        assert!(packets[0].contains("r3"));
    }

    #[test]
    fn test_gen_setup_packets_custom_base_reg() {
        let mut rng = StdRng::seed_from_u64(42);
        let deps = vec![PreDependency {
            register: "r5".to_string(),
            kind: SetupKind::MemoryAddress { max_offset: 64 },
        }];
        let packets = gen_setup_packets(&deps, &mut rng, 65536, "r26");
        assert_eq!(packets.len(), 1);
        assert!(packets[0].contains("r5"));
        assert!(packets[0].contains("r26"));
    }
}
