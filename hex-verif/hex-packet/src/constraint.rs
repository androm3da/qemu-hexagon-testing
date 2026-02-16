use hex_dump::types::InstructionDef;
use hex_instset::database::slot_mask_for_itype;

use crate::slot::assign_slots;
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

    // Rule 2: SoloAX - other instructions must be A-type or X-type (ALU32 or extender)
    for insn in insns {
        if insn.is_solo_ax {
            for other in insns {
                if std::ptr::eq(*insn, *other) {
                    continue;
                }
                let is_a_type = other.itype.starts_with("TypeALU32");
                let is_x_type = other.itype == "TypeEXTENDER";
                if !is_a_type && !is_x_type {
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

    // Rule 5: At most 2 branches
    let branches = insns
        .iter()
        .filter(|i| matches!(i.itype.as_str(), "TypeJ" | "TypeCJ" | "TypeNCJ"))
        .count();
    if branches > 2 {
        return ValidationResult::fail(format!("Too many branches: {}", branches));
    }

    // Rule 6: cofMax1 constraint - at most one cofMax1 instruction
    let cof_max1 = insns.iter().filter(|i| i.cof_max1).count();
    if cof_max1 > 1 {
        return ValidationResult::fail(format!("Too many cofMax1 instructions: {}", cof_max1));
    }

    // Rule 7: Slot assignment must succeed
    let itypes: Vec<&str> = insns.iter().map(|i| i.itype.as_str()).collect();
    if assign_slots(&itypes).is_none() {
        return ValidationResult::fail("Slot assignment failed");
    }

    ValidationResult::ok()
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

    // Rule: No unconditional double-write to the same register.
    // (Two instructions writing the same register without predication is illegal.)
    let mut dest_regs: Vec<(&str, bool)> = Vec::new(); // (reg_name, is_predicated)
    for ci in insns {
        for dest in &ci.dest_regs {
            // Check for conflict with existing destinations
            for &(existing, existing_pred) in &dest_regs {
                if existing == dest.as_str() {
                    // Two writes to same register
                    if !ci.def.is_predicated && !existing_pred {
                        return ValidationResult::fail(format!(
                            "Unconditional double-write to {}",
                            dest
                        ));
                    }
                }
            }
            dest_regs.push((dest, ci.def.is_predicated));
        }
    }

    ValidationResult::ok()
}

/// Check if an instruction is a "slot 0 only" type (most restricted).
pub fn is_slot0_only(itype: &str) -> bool {
    slot_mask_for_itype(itype) == 0x1
}

/// Check if an instruction is restricted to slots 0-1.
pub fn is_slot01(itype: &str) -> bool {
    slot_mask_for_itype(itype) == 0x3
}

/// Check if an instruction is restricted to slots 2-3.
pub fn is_slot23(itype: &str) -> bool {
    slot_mask_for_itype(itype) == 0xC
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
        let other = make_insn("A2_add", "TypeALU32_3op");
        let result = validate_packet(&[&solo, &other]);
        assert!(!result.valid);
        assert!(result.reason.unwrap().contains("solo"));
    }

    #[test]
    fn test_solo_alone_ok() {
        let mut solo = make_insn("J2_ploop0si", "TypeJ");
        solo.is_solo = true;
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
        assert!(result.reason.unwrap().contains("memory ops"));
    }

    #[test]
    fn test_two_stores_slot_conflict() {
        let mut s1 = make_insn("S2_storeri", "TypeST");
        s1.may_store = true;
        let mut s2 = make_insn("S2_storerh", "TypeST");
        s2.may_store = true;
        let result = validate_packet(&[&s1, &s2]);
        assert!(!result.valid);
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
}
