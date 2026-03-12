use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use hex_dump::types::{InstructionDef, InstructionSetDump};

use crate::filter::Filter;

/// Returns `true` if the instruction produces a predicate register **early**
/// (available for `.new` consumers in the same packet).
///
/// An early producer has a `PredRegs` output operand and does not have
/// `is_predicate_late` set.
pub fn is_early_predicate_producer(insn: &InstructionDef) -> bool {
    pred_output_operand(insn).is_some() && !insn.is_predicate_late
}

/// Returns the name of the first `PredRegs` output operand, if any.
pub fn pred_output_operand(insn: &InstructionDef) -> Option<&str> {
    insn.outs.iter().find_map(|op| {
        if op.reg_class.as_deref() == Some("PredRegs") {
            Some(op.name.as_str())
        } else {
            None
        }
    })
}

/// Returns the name of the first `PredRegs` input operand, if any.
pub fn pred_input_operand(insn: &InstructionDef) -> Option<&str> {
    insn.ins.iter().find_map(|op| {
        if op.reg_class.as_deref() == Some("PredRegs") {
            Some(op.name.as_str())
        } else {
            None
        }
    })
}

/// Slot mask for an instruction type, representing which VLIW slots this instruction can execute in.
/// Bit 0 = slot 0, bit 1 = slot 1, etc.
pub type SlotMask = u8;

/// An indexed instruction database loaded from a JSON dump.
#[derive(Clone)]
pub struct InstructionDb {
    /// All instructions, indexed by name.
    by_name: HashMap<String, usize>,
    /// All instructions, indexed by itype.
    by_type: HashMap<String, Vec<usize>>,
    /// The instructions themselves.
    instructions: Vec<InstructionDef>,
}

impl InstructionDb {
    /// Load the instruction database from a JSON file.
    pub fn load_from_json(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let dump: InstructionSetDump =
            serde_json::from_str(&content).context("Failed to parse instruction JSON")?;
        Ok(Self::from_dump(dump))
    }

    /// Build the database from a parsed dump.
    pub fn from_dump(dump: InstructionSetDump) -> Self {
        let instructions = dump.instructions;
        let mut by_name = HashMap::new();
        let mut by_type: HashMap<String, Vec<usize>> = HashMap::new();

        for (idx, insn) in instructions.iter().enumerate() {
            by_name.insert(insn.name.clone(), idx);
            by_type.entry(insn.itype.clone()).or_default().push(idx);
        }

        Self {
            by_name,
            by_type,
            instructions,
        }
    }

    /// Look up an instruction by name.
    pub fn get(&self, name: &str) -> Option<&InstructionDef> {
        self.by_name.get(name).map(|&idx| &self.instructions[idx])
    }

    /// Get all instructions of a given IType.
    pub fn by_type(&self, itype: &str) -> Vec<&InstructionDef> {
        self.by_type
            .get(itype)
            .map(|indices| indices.iter().map(|&idx| &self.instructions[idx]).collect())
            .unwrap_or_default()
    }

    /// Filter instructions using a Filter predicate.
    pub fn filter(&self, predicate: &Filter) -> Vec<&InstructionDef> {
        self.instructions
            .iter()
            .filter(|insn| predicate.matches(insn))
            .collect()
    }

    /// Return all instructions.
    pub fn all(&self) -> &[InstructionDef] {
        &self.instructions
    }

    /// Return the number of instructions.
    pub fn len(&self) -> usize {
        self.instructions.len()
    }

    /// Returns true if the database is empty.
    pub fn is_empty(&self) -> bool {
        self.instructions.is_empty()
    }

    /// Return the precise slot mask for an instruction.
    ///
    /// Uses the itinerary-derived `slot_mask` field when available (non-zero),
    /// falling back to the coarse itype-based mask.
    pub fn slot_mask(insn: &InstructionDef) -> SlotMask {
        if insn.slot_mask != 0 {
            insn.slot_mask
        } else {
            slot_mask_for_itype(&insn.itype)
        }
    }
}

/// Compute the slot mask for a given IType string.
/// Slots: bit 0 = slot 0, bit 1 = slot 1, bit 2 = slot 2, bit 3 = slot 3.
pub fn slot_mask_for_itype(itype: &str) -> SlotMask {
    match itype {
        // ALU32 types can go in any slot (0-3)
        "TypeALU32_2op" | "TypeALU32_3op" | "TypeALU32_ADDI" => 0xF,

        // ALU64, M, S types go in slots 2-3
        "TypeALU64" | "TypeM" | "TypeS_2op" | "TypeS_3op" => 0xC,

        // Load types go in slots 0-1
        "TypeLD" | "TypeV2LDST" => 0x3,

        // V4LDST (memops: atomic read-modify-write) are slot 0 only
        "TypeV4LDST" => 0x1,

        // Store type goes in slot 0 only
        "TypeST" => 0x1,

        // Branch/control types go in slots 2-3
        "TypeJ" | "TypeCJ" | "TypeNCJ" | "TypeCR" => 0xC,

        // CVI load types go in slots 0-1
        "TypeCVI_VM_LD" | "TypeCVI_VM_TMP_LD" | "TypeCVI_VM_VP_LDU" | "TypeCVI_GATHER"
        | "TypeCVI_GATHER_DV" | "TypeCVI_GATHER_RST" => 0x3,

        // CVI store types go in slot 0
        "TypeCVI_VM_ST"
        | "TypeCVI_VM_NEW_ST"
        | "TypeCVI_VM_STU"
        | "TypeCVI_SCATTER"
        | "TypeCVI_SCATTER_DV"
        | "TypeCVI_SCATTER_RST"
        | "TypeCVI_SCATTER_NEW_RST"
        | "TypeCVI_SCATTER_NEW_ST" => 0x1,

        // CVI ZW goes in slot 0-1
        "TypeCVI_ZW" => 0x3,

        // CVI ALU types can go in any slot (0-3)
        "TypeCVI_VA" | "TypeCVI_VA_DV" | "TypeCVI_VP" | "TypeCVI_VP_VS" | "TypeCVI_VS"
        | "TypeCVI_VS_VX" | "TypeCVI_VX" | "TypeCVI_VX_DV" | "TypeCVI_VX_LATE"
        | "TypeCVI_4SLOT_MPY" | "TypeCVI_HIST" => 0xF,

        // Fallback: any slot
        _ => 0xF,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::Filter;
    use hex_dump::types::{InstructionDef, InstructionSetDump};

    fn make_test_db() -> InstructionDb {
        let mut insns = Vec::new();

        let mut add = InstructionDef::new("A2_add".to_string());
        add.itype = "TypeALU32_3op".to_string();
        add.has_new_value = true;
        insns.push(add);

        let mut load = InstructionDef::new("L2_loadri_io".to_string());
        load.itype = "TypeLD".to_string();
        load.may_load = true;
        insns.push(load);

        let mut store = InstructionDef::new("S2_storeri_io".to_string());
        store.itype = "TypeST".to_string();
        store.may_store = true;
        insns.push(store);

        let mut cvi = InstructionDef::new("V6_vadd".to_string());
        cvi.itype = "TypeCVI_VA".to_string();
        cvi.is_cvi = true;
        insns.push(cvi);

        let dump = InstructionSetDump {
            version: "1.0".to_string(),
            total_parsed: 4,
            instructions: insns,
        };
        InstructionDb::from_dump(dump)
    }

    #[test]
    fn test_get_by_name() {
        let db = make_test_db();
        assert!(db.get("A2_add").is_some());
        assert_eq!(db.get("A2_add").unwrap().itype, "TypeALU32_3op");
        assert!(db.get("nonexistent").is_none());
    }

    #[test]
    fn test_by_type() {
        let db = make_test_db();
        let alu = db.by_type("TypeALU32_3op");
        assert_eq!(alu.len(), 1);
        assert_eq!(alu[0].name, "A2_add");
    }

    #[test]
    fn test_filter() {
        let db = make_test_db();
        let loads = db.filter(&Filter::ByType("TypeLD".to_string()));
        assert_eq!(loads.len(), 1);
        assert_eq!(loads[0].name, "L2_loadri_io");
    }

    #[test]
    fn test_slot_masks() {
        assert_eq!(slot_mask_for_itype("TypeALU32_3op"), 0xF);
        assert_eq!(slot_mask_for_itype("TypeALU64"), 0xC);
        assert_eq!(slot_mask_for_itype("TypeLD"), 0x3);
        assert_eq!(slot_mask_for_itype("TypeST"), 0x1);
        assert_eq!(slot_mask_for_itype("TypeJ"), 0xC);
        assert_eq!(slot_mask_for_itype("TypeCVI_VA"), 0xF);
        assert_eq!(slot_mask_for_itype("TypeCVI_VM_ST"), 0x1);
    }

    #[test]
    fn test_len() {
        let db = make_test_db();
        assert_eq!(db.len(), 4);
        assert!(!db.is_empty());
    }

    #[test]
    fn test_early_predicate_producer() {
        use hex_dump::types::Operand;
        // An instruction with a PredRegs output that is NOT in the late list
        let mut cmp = InstructionDef::new("C2_cmpeq".to_string());
        cmp.itype = "TypeALU32_3op".to_string();
        cmp.outs = vec![Operand {
            name: "Pd4".to_string(),
            reg_class: Some("PredRegs".to_string()),
            is_immediate: false,
            imm_type: None,
        }];
        assert!(is_early_predicate_producer(&cmp));
        assert_eq!(pred_output_operand(&cmp), Some("Pd4"));
    }

    #[test]
    fn test_late_predicate_producer() {
        use hex_dump::types::Operand;
        let mut locked = InstructionDef::new("S2_storew_locked".to_string());
        locked.itype = "TypeST".to_string();
        locked.is_predicate_late = true;
        locked.outs = vec![Operand {
            name: "Pd4".to_string(),
            reg_class: Some("PredRegs".to_string()),
            is_immediate: false,
            imm_type: None,
        }];
        // Has pred output but is_predicate_late
        assert!(!is_early_predicate_producer(&locked));
        assert_eq!(pred_output_operand(&locked), Some("Pd4"));
    }

    #[test]
    fn test_no_pred_output() {
        // A2_add has no predicate output
        let add = InstructionDef::new("A2_add".to_string());
        assert!(!is_early_predicate_producer(&add));
        assert_eq!(pred_output_operand(&add), None);
    }

    #[test]
    fn test_pred_input_operand() {
        use hex_dump::types::Operand;
        let mut cond = InstructionDef::new("A2_paddt".to_string());
        cond.ins = vec![Operand {
            name: "Pu4".to_string(),
            reg_class: Some("PredRegs".to_string()),
            is_immediate: false,
            imm_type: None,
        }];
        assert_eq!(pred_input_operand(&cond), Some("Pu4"));

        let plain = InstructionDef::new("A2_add".to_string());
        assert_eq!(pred_input_operand(&plain), None);
    }
}
