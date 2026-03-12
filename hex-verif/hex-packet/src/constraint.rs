use hex_dump::types::InstructionDef;
use hex_instset::database::is_early_predicate_producer;

use crate::slot::assign_slots_with_restrictions;
use crate::synthesizer::ConcreteInsn;

/// Result of packet validation.
#[derive(Debug)]
pub struct ValidationResult {
    pub valid: bool,
    pub reason: Option<String>,
}

impl ValidationResult {
    fn ok() -> Self {
        Self {
            valid: true,
            reason: None,
        }
    }

    fn fail(reason: impl Into<String>) -> Self {
        Self {
            valid: false,
            reason: Some(reason.into()),
        }
    }
}

/// Check if an instruction qualifies as an A-type or X-type companion for SoloAX.
/// FP instructions are excluded even if their itype would otherwise qualify.
fn is_a_or_x_type(insn: &InstructionDef) -> bool {
    if insn.is_fp {
        return false;
    }
    matches!(
        insn.itype.as_str(),
        "TypeALU32_2op" | "TypeALU32_3op" | "TypeALU32_ADDI" | "TypeEXTENDER"
    )
}

/// Validate that a set of instructions can legally form a VLIW packet.
pub fn validate_packet(insns: &[&InstructionDef]) -> ValidationResult {
    if insns.is_empty() {
        return ValidationResult::fail("Empty packet");
    }
    if insns.len() > 4 {
        return ValidationResult::fail("Packet too large (max 4)");
    }

    // Rule 1: Solo instructions must be alone
    for insn in insns {
        if insn.is_solo && insns.len() > 1 {
            return ValidationResult::fail(format!(
                "{} is solo but packet has {} insns",
                insn.name,
                insns.len()
            ));
        }
    }

    // Rule 2: SoloAX - other instructions must be A-type or X-type (ALU32 or extender),
    // excluding FP instructions.
    for insn in insns {
        if insn.is_solo_ax {
            for other in insns {
                if std::ptr::eq(*insn, *other) {
                    continue;
                }
                if !is_a_or_x_type(other) {
                    return ValidationResult::fail(format!(
                        "{} is soloAX but {} is type {}",
                        insn.name, other.name, other.itype
                    ));
                }
            }
        }
    }

    // Rule 3: At most 2 memory operations (loads + stores)
    let mem_ops = insns.iter().filter(|i| i.may_load || i.may_store).count();
    if mem_ops > 2 {
        return ValidationResult::fail(format!("Too many memory ops: {}", mem_ops));
    }

    // Rule 3b: At most 1 store (hardware limit)
    let stores = insns.iter().filter(|i| i.may_store).count();
    if stores > 1 {
        return ValidationResult::fail(format!("Too many stores: {}", stores));
    }

    // Rule 4: CVI resource limits
    let cvi_loads = insns
        .iter()
        .filter(|i| i.is_cvi && i.may_load && i.itype != "TypeCVI_ZW")
        .count();
    let cvi_zw = insns.iter().filter(|i| i.itype == "TypeCVI_ZW").count();
    let cvi_stores = insns.iter().filter(|i| i.is_cvi && i.may_store).count();
    if cvi_loads > 1 {
        return ValidationResult::fail(format!("Too many CVI loads: {}", cvi_loads));
    }
    if cvi_zw > 1 {
        return ValidationResult::fail(format!("Too many CVI ZW: {}", cvi_zw));
    }
    if cvi_stores > 1 {
        return ValidationResult::fail(format!("Too many CVI stores: {}", cvi_stores));
    }

    // Rule 5: At most 2 branches (using is_branch field)
    let branches = insns.iter().filter(|i| i.is_branch).count();
    if branches > 2 {
        return ValidationResult::fail(format!("Too many branches: {}", branches));
    }

    // Rule 6: cofMax1 constraint
    // A cofMax1 instruction can coexist with others only if the cofMax1 insn
    // has cofRelax1 (as the first branch) or cofRelax2 (as the second branch),
    // and its companion is a proper branch. For simplicity, reject packets with
    // more than one cofMax1 instruction, and reject a cofMax1 instruction
    // alongside a non-cofRelax branch if neither has cofRelax.
    let cof_max1_insns: Vec<&InstructionDef> =
        insns.iter().filter(|i| i.cof_max1).copied().collect();
    if cof_max1_insns.len() > 1 {
        return ValidationResult::fail(format!(
            "Too many cofMax1 instructions: {}",
            cof_max1_insns.len()
        ));
    }
    if cof_max1_insns.len() == 1 {
        let cof_insn = cof_max1_insns[0];
        // If there are other branches, the cofMax1 instruction needs cofRelax1 or cofRelax2
        let other_branches = insns
            .iter()
            .filter(|&&i| !std::ptr::eq(i, cof_insn) && i.is_branch)
            .count();
        if other_branches > 0 && !cof_insn.cof_relax1 && !cof_insn.cof_relax2 {
            return ValidationResult::fail(format!(
                "{} is cofMax1 without cofRelax but has {} other branch(es)",
                cof_insn.name, other_branches
            ));
        }
    }

    // Rule 7: Slot assignment must succeed (with restriction rules)
    if assign_slots_with_restrictions(insns).is_none() {
        return ValidationResult::fail("Slot assignment failed");
    }

    ValidationResult::ok()
}

/// Returns the implicit predicate register written by an instruction, if any.
///
/// Hardware loop setup instructions (`J2_ploop*`) implicitly write P3 but
/// don't list it as an explicit output operand. This function detects such
/// instructions: `is_predicate_late` is set, but there's no `PredRegs` in
/// the explicit output list.
pub fn implicit_pred_output(insn: &InstructionDef) -> Option<&'static str> {
    if insn.is_predicate_late {
        let has_explicit_pred_out = insn
            .outs
            .iter()
            .any(|op| op.reg_class.as_deref() == Some("PredRegs"));
        if !has_explicit_pred_out {
            return Some("p3");
        }
    }
    None
}

/// Returns implicit integer register outputs for an instruction.
///
/// Some instructions have implicit defs not captured in the `outs` operand list:
/// - `L2_deallocframe` → R29 (SP restored)
/// - `L4_return*` (dealloc_return) → R29 (SP restored)
/// - `S2_allocframe` → R30 (FP saved/updated)
pub fn implicit_int_reg_outputs(insn: &InstructionDef) -> &'static [&'static str] {
    if insn.name == "L2_deallocframe" || insn.name.starts_with("L4_return") {
        &["r29"]
    } else if insn.name == "S2_allocframe" {
        &["r30"]
    } else {
        &[]
    }
}

/// Information about a register write for conflict checking.
struct RegWrite<'a> {
    /// All physical registers this write touches (including aliases).
    regs: Vec<String>,
    /// The primary register name (for error messages).
    primary_reg: &'a str,
    is_predicated: bool,
    is_pred_false: bool,
    pred_reg: Option<&'a str>,
    /// True if this register is a predicate register.
    is_pred_reg: bool,
    /// True if this is a late predicate write.
    is_late_pred: bool,
}

/// Expand a register name into all physical registers it touches.
/// For example, "r5:4" → ["r5:4", "r5", "r4"].
/// Single registers also map to their double-reg pair.
fn reg_aliases(name: &str) -> Vec<String> {
    let mut aliases = vec![name.to_string()];
    if let Some(colon_pos) = name.find(':') {
        let hi_part = &name[..colon_pos];
        let lo_part = &name[colon_pos + 1..];
        let prefix = &hi_part[..1];
        let lo_name = format!("{}{}", prefix, lo_part);
        aliases.push(hi_part.to_string());
        aliases.push(lo_name);
    } else {
        let prefix = &name[..1];
        if let Ok(idx) = name[1..].parse::<usize>() {
            if prefix == "r" || prefix == "v" {
                let pair_lo = idx & !1;
                let pair_hi = pair_lo + 1;
                let double_name = format!("{}{}:{}", prefix, pair_hi, pair_lo);
                aliases.push(double_name);
            }
        }
    }
    aliases
}

/// Check if two register writes overlap (share any physical register).
fn regs_overlap(w1: &RegWrite, w2: &RegWrite) -> bool {
    w1.regs.iter().any(|r1| w2.regs.iter().any(|r2| r1 == r2))
}

/// Validate a packet of concrete instructions (with register assignments).
/// Checks register conflict rules in addition to the basic packet rules.
pub fn validate_concrete_packet(insns: &[ConcreteInsn]) -> ValidationResult {
    // First validate the instruction-level rules
    let defs: Vec<&InstructionDef> = insns.iter().map(|ci| ci.def).collect();
    let result = validate_packet(&defs);
    if !result.valid {
        return result;
    }

    // Build a list of all register writes with predication info
    let mut writes: Vec<RegWrite> = Vec::new();
    for ci in insns {
        // Find the concrete predicate register for this instruction (if predicated)
        let pred_reg = if ci.def.is_predicated {
            ci.def
                .ins
                .iter()
                .filter(|op| !op.is_immediate)
                .zip(ci.src_regs.iter())
                .find(|(op, _)| op.reg_class.as_deref() == Some("PredRegs"))
                .map(|(_, name)| name.as_str())
        } else {
            None
        };

        for (dest, op) in ci.dest_regs.iter().zip(ci.def.outs.iter()) {
            let is_pred_reg = op.reg_class.as_deref() == Some("PredRegs");
            writes.push(RegWrite {
                regs: reg_aliases(dest),
                primary_reg: dest,
                is_predicated: ci.def.is_predicated,
                is_pred_false: ci.def.is_predicated_false,
                pred_reg,
                is_pred_reg,
                is_late_pred: is_pred_reg && ci.def.is_predicate_late,
            });
        }

        // Add implicit P3 write for J2_ploop* and similar instructions.
        if let Some(implicit_pred) = implicit_pred_output(ci.def) {
            writes.push(RegWrite {
                regs: reg_aliases(implicit_pred),
                primary_reg: implicit_pred,
                is_predicated: false,
                is_pred_false: false,
                pred_reg: None,
                is_pred_reg: true,
                is_late_pred: true,
            });
        }

        // Add implicit integer register writes (e.g. R29 for deallocframe/return).
        for &implicit_reg in implicit_int_reg_outputs(ci.def) {
            writes.push(RegWrite {
                regs: reg_aliases(implicit_reg),
                primary_reg: implicit_reg,
                is_predicated: ci.def.is_predicated,
                is_pred_false: ci.def.is_predicated_false,
                pred_reg,
                is_pred_reg: false,
                is_late_pred: false,
            });
        }
    }

    // Check for register write conflicts
    for i in 0..writes.len() {
        for j in (i + 1)..writes.len() {
            if !regs_overlap(&writes[i], &writes[j]) {
                continue;
            }

            let w1 = &writes[i];
            let w2 = &writes[j];

            // Predicate registers: the MCChecker allows multiple early
            // predicate writes (they're logically AND-ed), but a late
            // predicate write + any other def is illegal.
            if w1.is_pred_reg && w2.is_pred_reg {
                if w1.is_late_pred || w2.is_late_pred {
                    return ValidationResult::fail(format!(
                        "Late predicate write to {} conflicts with another predicate def",
                        w1.primary_reg
                    ));
                }
                continue;
            }

            if !w1.is_predicated || !w2.is_predicated {
                // At least one is unconditional — illegal
                return ValidationResult::fail(format!(
                    "Unconditional double-write to {}",
                    w1.primary_reg
                ));
            }

            // Both are predicated: check the MCChecker rules.
            // The MCChecker allows at most 2 conditional writes to the same
            // register. They must NOT have the same (pred_reg, sense) pair.
            // If they have the same pred_reg with opposite sense, that's OK
            // (at most 2). Different pred_regs are also OK (at most 2 total).
            match (w1.pred_reg, w2.pred_reg) {
                (Some(p1), Some(p2)) => {
                    if p1 == p2 && w1.is_pred_false == w2.is_pred_false {
                        // Same predicate, same sense — illegal
                        return ValidationResult::fail(format!(
                            "Conditional double-write to {} with same predicate sense on {}",
                            w1.primary_reg, p1
                        ));
                    }
                    // Same pred opposite sense, or different preds: allowed (2 writes max)
                }
                _ => {
                    // Can't determine predicate registers — reject conservatively
                    return ValidationResult::fail(format!(
                        "Conditional double-write to {} but predicate register unknown",
                        w1.primary_reg
                    ));
                }
            }
        }
    }

    // Check for more than 2 writes to the same physical register
    // (the MCChecker allows at most 2 conditional writes)
    let mut write_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for w in &writes {
        for reg in &w.regs {
            *write_counts.entry(reg.as_str()).or_insert(0) += 1;
        }
    }
    for (reg, count) in &write_counts {
        if *count > 2 {
            return ValidationResult::fail(format!(
                "Register {} written {} times (max 2)",
                reg, count
            ));
        }
    }

    ValidationResult::ok()
}

/// Validate NV-store producer-consumer pairing rules.
///
/// Checks that:
/// - Every NV-store consumer has a producer with `has_new_value` in the packet
/// - If the consumer is unconditional, the producer must also be unconditional
/// - If both are predicated, they must use the same predicate register and sense
/// - DoubleRegs producers cannot feed NV stores
/// - Producers with AbsoluteSet or PostInc addressing cannot feed NV stores
pub fn validate_nv_store_pairing(insns: &[ConcreteInsn]) -> ValidationResult {
    for ci in insns {
        if !ci.def.is_nv_store {
            continue;
        }

        // Find the Nt8 operand — the new-value source register
        let nv_reg = ci
            .def
            .ins
            .iter()
            .filter(|op| !op.is_immediate)
            .zip(ci.src_regs.iter())
            .find(|(op, _)| op.name.starts_with("Nt"))
            .map(|(_, name)| name.as_str());

        let nv_reg = match nv_reg {
            Some(r) => r,
            None => continue,
        };

        // Find the producer: an instruction that writes nv_reg with has_new_value
        let producer = insns.iter().find(|other| {
            if std::ptr::eq(ci.def, other.def) && ci.asm_text == other.asm_text {
                return false; // skip self
            }
            other.def.has_new_value && other.dest_regs.iter().any(|d| d == nv_reg)
        });

        let producer = match producer {
            Some(p) => p,
            None => {
                return ValidationResult::fail(format!(
                    "{} is NV-store but no producer for {} in packet",
                    ci.def.name, nv_reg
                ));
            }
        };

        // DoubleRegs cannot be NV producers
        if producer
            .def
            .outs
            .iter()
            .any(|op| op.reg_class.as_deref() == Some("DoubleRegs"))
        {
            return ValidationResult::fail(format!(
                "{} is NV-store but producer {} writes DoubleRegs",
                ci.def.name, producer.def.name
            ));
        }

        // AbsoluteSet (2) or PostInc (6) address mode producers cannot feed NV stores
        if producer.def.addr_mode == 2 || producer.def.addr_mode == 6 {
            return ValidationResult::fail(format!(
                "{} is NV-store but producer {} has addr_mode {}",
                ci.def.name, producer.def.name, producer.def.addr_mode
            ));
        }

        // Predication rules:
        // If consumer is unconditional, producer must be unconditional
        if !ci.def.is_predicated && producer.def.is_predicated {
            return ValidationResult::fail(format!(
                "{} is unconditional NV-store but producer {} is predicated",
                ci.def.name, producer.def.name
            ));
        }

        // If both are predicated, must use same predicate and sense
        if ci.def.is_predicated && producer.def.is_predicated {
            let consumer_pred = ci
                .def
                .ins
                .iter()
                .filter(|op| !op.is_immediate)
                .zip(ci.src_regs.iter())
                .find(|(op, _)| op.reg_class.as_deref() == Some("PredRegs"))
                .map(|(_, name)| name.as_str());

            let producer_pred = producer
                .def
                .ins
                .iter()
                .filter(|op| !op.is_immediate)
                .zip(producer.src_regs.iter())
                .find(|(op, _)| op.reg_class.as_deref() == Some("PredRegs"))
                .map(|(_, name)| name.as_str());

            match (consumer_pred, producer_pred) {
                (Some(cp), Some(pp)) => {
                    if cp != pp {
                        return ValidationResult::fail(format!(
                            "NV-store {} and producer {} use different predicates ({} vs {})",
                            ci.def.name, producer.def.name, cp, pp
                        ));
                    }
                    if ci.def.is_predicated_false != producer.def.is_predicated_false {
                        return ValidationResult::fail(format!(
                            "NV-store {} and producer {} have different predicate sense",
                            ci.def.name, producer.def.name
                        ));
                    }
                }
                _ => {
                    return ValidationResult::fail(format!(
                        "NV-store {} and producer {} are both predicated but predicate register unknown",
                        ci.def.name, producer.def.name
                    ));
                }
            }
        }
    }

    ValidationResult::ok()
}

/// Validate that every `.new` predicate consumer has a matching early producer
/// in the same packet writing to the same predicate register.
pub fn validate_dot_new_pairing(insns: &[ConcreteInsn]) -> ValidationResult {
    for ci in insns {
        if !ci.def.is_predicated_new {
            continue;
        }

        // Find the predicate register this consumer reads
        let pred_reg = ci
            .def
            .ins
            .iter()
            .find(|op| op.reg_class.as_deref() == Some("PredRegs"))
            .and_then(|op| {
                // Find it in the concrete src_regs by position
                let reg_idx = ci
                    .def
                    .ins
                    .iter()
                    .filter(|o| !o.is_immediate)
                    .position(|o| o.name == op.name)?;
                ci.src_regs.get(reg_idx).map(|s| s.as_str())
            });

        let pred_reg = match pred_reg {
            Some(r) => r,
            None => continue, // No pred operand found, skip
        };

        // Check that some other instruction in the packet writes this pred register
        // and is an early producer
        let has_producer = insns.iter().any(|other| {
            if std::ptr::eq(ci.def, other.def) && ci.asm_text == other.asm_text {
                return false; // skip self
            }
            if !is_early_predicate_producer(other.def) {
                return false;
            }
            other.dest_regs.iter().any(|d| d == pred_reg)
        });

        if !has_producer {
            return ValidationResult::fail(format!(
                "{} uses .new predicate {} but no early producer found in packet",
                ci.def.name, pred_reg
            ));
        }

        // MCChecker rule: if ANY instruction defines this predicate register
        // as a late producer, the .new use is forbidden — even if an early
        // producer also exists.
        let has_late_conflict = insns.iter().any(|other| {
            if std::ptr::eq(ci.def, other.def) && ci.asm_text == other.asm_text {
                return false;
            }
            if !other.def.is_predicate_late {
                return false;
            }
            // Check explicit PredRegs outputs
            let explicit_conflict = other.dest_regs.iter().any(|d| d == pred_reg);
            // Check implicit P3 write (J2_ploop* etc.)
            let implicit_conflict = implicit_pred_output(other.def).is_some_and(|p| p == pred_reg);
            explicit_conflict || implicit_conflict
        });

        if has_late_conflict {
            return ValidationResult::fail(format!(
                "{} uses .new predicate {} but a late producer also defines it",
                ci.def.name, pred_reg
            ));
        }
    }

    ValidationResult::ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_dump::types::InstructionDef;

    fn make_insn(name: &str, itype: &str) -> InstructionDef {
        let mut insn = InstructionDef::new(name.to_string());
        insn.itype = itype.to_string();
        insn
    }

    #[test]
    fn test_valid_single_insn() {
        let insn = make_insn("A2_add", "TypeALU32_3op");
        let result = validate_packet(&[&insn]);
        assert!(result.valid);
    }

    #[test]
    fn test_solo_with_others() {
        let mut solo = make_insn("J2_ploop0si", "TypeJ");
        solo.is_solo = true;
        solo.is_branch = true;
        let other = make_insn("A2_add", "TypeALU32_3op");
        let result = validate_packet(&[&solo, &other]);
        assert!(!result.valid);
        assert!(result.reason.unwrap().contains("solo"));
    }

    #[test]
    fn test_solo_alone_ok() {
        let mut solo = make_insn("J2_ploop0si", "TypeJ");
        solo.is_solo = true;
        solo.is_branch = true;
        let result = validate_packet(&[&solo]);
        assert!(result.valid);
    }

    #[test]
    fn test_too_many_mem_ops() {
        let mut l1 = make_insn("L2_loadri", "TypeLD");
        l1.may_load = true;
        let mut l2 = make_insn("L2_loadrh", "TypeLD");
        l2.may_load = true;
        let mut s1 = make_insn("S2_storeri", "TypeST");
        s1.may_store = true;
        let result = validate_packet(&[&l1, &l2, &s1]);
        assert!(!result.valid);
        // Now rejects for "too many stores" since loads + store > 2 mem ops
        // but also store count is checked
    }

    #[test]
    fn test_two_stores_rejected() {
        let mut s1 = make_insn("S2_storeri", "TypeST");
        s1.may_store = true;
        let mut s2 = make_insn("S2_storerh", "TypeST");
        s2.may_store = true;
        let result = validate_packet(&[&s1, &s2]);
        assert!(!result.valid);
        assert!(result.reason.unwrap().contains("stores"));
    }

    #[test]
    fn test_valid_full_packet() {
        let mut s = make_insn("S2_storeri", "TypeST");
        s.may_store = true;
        let mut l = make_insn("L2_loadri", "TypeLD");
        l.may_load = true;
        let alu1 = make_insn("A2_add", "TypeALU64");
        let alu2 = make_insn("A2_sub", "TypeALU32_3op");
        let result = validate_packet(&[&s, &l, &alu1, &alu2]);
        assert!(result.valid);
    }

    #[test]
    fn test_solo_ax_rejects_fp() {
        let mut solo_ax = make_insn("F2_sfadd", "TypeM");
        solo_ax.is_solo_ax = true;
        let mut fp_companion = make_insn("F2_sfmpy", "TypeALU32_3op");
        fp_companion.is_fp = true;
        let result = validate_packet(&[&solo_ax, &fp_companion]);
        assert!(!result.valid);
        assert!(result.reason.unwrap().contains("soloAX"));
    }

    #[test]
    fn test_solo_ax_allows_alu32() {
        let mut solo_ax = make_insn("F2_sfadd", "TypeM");
        solo_ax.is_solo_ax = true;
        let companion = make_insn("A2_add", "TypeALU32_3op");
        let result = validate_packet(&[&solo_ax, &companion]);
        assert!(result.valid);
    }

    #[test]
    fn test_branch_count_uses_is_branch() {
        let mut b1 = make_insn("J2_jump", "TypeJ");
        b1.is_branch = true;
        let mut b2 = make_insn("J4_cmpeq_t_p0", "TypeCJ");
        b2.is_branch = true;
        let mut b3 = make_insn("J4_cmpeqi_t_p0", "TypeNCJ");
        b3.is_branch = true;
        let result = validate_packet(&[&b1, &b2, &b3]);
        assert!(!result.valid);
        assert!(result.reason.unwrap().contains("branches"));
    }
}
