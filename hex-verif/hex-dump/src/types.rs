use serde::{Deserialize, Serialize};

/// A single operand of an instruction (input or output).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Operand {
    /// The operand name as it appears in the tablegen, e.g. "Rd32", "Rs32", "Ii".
    pub name: String,
    /// The register class, e.g. "IntRegs", "DoubleRegs", "PredRegs", "HvxVR", or None for immediates.
    pub reg_class: Option<String>,
    /// True if this operand is an immediate value rather than a register.
    pub is_immediate: bool,
    /// The immediate type string if this is an immediate, e.g. "s32_0Imm", "u10_0Imm".
    pub imm_type: Option<String>,
}

/// A parsed instruction definition from the Hexagon tablegen.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstructionDef {
    /// The instruction name, e.g. "A2_add".
    pub name: String,
    /// Output operands (dests).
    pub outs: Vec<Operand>,
    /// Input operands (sources).
    pub ins: Vec<Operand>,
    /// Assembly syntax string, e.g. "$Rd32 = add($Rs32,$Rt32)".
    pub asm_syntax: String,
    /// Instruction type, e.g. "TypeALU32_3op", "TypeLD", "TypeST".
    pub itype: String,

    // Boolean flags
    pub is_pseudo: bool,
    pub is_code_gen_only: bool,
    pub is_solo: bool,
    pub is_solo_ax: bool,
    pub is_predicated: bool,
    pub is_predicated_false: bool,
    pub is_predicated_new: bool,
    pub has_new_value: bool,
    pub is_nv_store: bool,
    pub is_nv_storable: bool,
    pub is_fp: bool,
    pub is_cvi: bool,
    pub is_hvx_alu: bool,
    pub is_hvx_alu_2src: bool,
    pub may_load: bool,
    pub may_store: bool,
    pub is_commutable: bool,
    pub is_predicable: bool,
    pub is_extendable: bool,
    pub is_extent_signed: bool,
    pub has_side_effects: bool,
    pub is_call: bool,
    pub is_return: bool,
    pub prefers_slot3: bool,
    pub cof_max1: bool,
    pub cof_relax1: bool,
    pub cof_relax2: bool,
    pub is_restrict_no_slot1_store: bool,
    pub is_restrict_slot1_aok: bool,
    pub is_new_value: bool,
    pub is_predicate_late: bool,
    pub is_branch: bool,
    pub is_accumulator: bool,
    pub cvi_new: bool,
    pub has_hvx_tmp: bool,
    pub addr_mode: u32,

    /// VLIW slot mask from the scheduling class itinerary.
    /// Bit 0 = SLOT0, bit 1 = SLOT1, bit 2 = SLOT2, bit 3 = SLOT3.
    /// Default 0xf (all slots) when scheduling class is unknown.
    pub slot_mask: u8,

    // Numeric attributes
    pub op_new_value: Option<u32>,
    pub op_extent_bits: Option<u32>,
    pub op_extent_align: Option<u32>,
    pub op_extendable: Option<u32>,

    // Lists
    pub defs: Vec<String>,
    pub uses: Vec<String>,
    pub requires: Vec<String>,

    // Constraints
    pub constraints: Option<String>,
}

impl InstructionDef {
    pub fn new(name: String) -> Self {
        Self {
            name,
            outs: Vec::new(),
            ins: Vec::new(),
            asm_syntax: String::new(),
            itype: String::new(),
            is_pseudo: false,
            is_code_gen_only: false,
            is_solo: false,
            is_solo_ax: false,
            is_predicated: false,
            is_predicated_false: false,
            is_predicated_new: false,
            has_new_value: false,
            is_nv_store: false,
            is_nv_storable: false,
            is_fp: false,
            is_cvi: false,
            is_hvx_alu: false,
            is_hvx_alu_2src: false,
            may_load: false,
            may_store: false,
            is_commutable: false,
            is_predicable: false,
            is_extendable: false,
            is_extent_signed: false,
            has_side_effects: false,
            is_call: false,
            is_return: false,
            prefers_slot3: false,
            cof_max1: false,
            cof_relax1: false,
            cof_relax2: false,
            is_restrict_no_slot1_store: false,
            is_restrict_slot1_aok: false,
            is_new_value: false,
            is_predicate_late: false,
            is_branch: false,
            is_accumulator: false,
            cvi_new: false,
            has_hvx_tmp: false,
            addr_mode: 0,
            slot_mask: 0xf,
            op_new_value: None,
            op_extent_bits: None,
            op_extent_align: None,
            op_extendable: None,
            defs: Vec::new(),
            uses: Vec::new(),
            requires: Vec::new(),
            constraints: None,
        }
    }

    /// Returns true if this instruction always requires a constant extender.
    ///
    /// Instructions with `u32_0Imm` or `s32_0Imm` operands (absolute-address,
    /// absolute-set, or unscaled-register addressing modes) always encode the
    /// immediate via a constant extender that occupies one VLIW packet slot.
    pub fn needs_constant_extender(&self) -> bool {
        self.ins.iter().chain(self.outs.iter()).any(|op| {
            op.imm_type
                .as_deref()
                .is_some_and(|t| t == "u32_0Imm" || t == "s32_0Imm")
        })
    }

    /// Returns true if this instruction should be filtered out (not a real encodable instruction).
    pub fn should_filter(&self) -> bool {
        if self.is_pseudo || self.is_code_gen_only {
            return true;
        }
        matches!(
            self.itype.as_str(),
            "TypeMAPPING"
                | "TypePSEUDO"
                | "TypeSUBINSN"
                | "TypeDUPLEX"
                | "TypeENDLOOP"
                | "TypeEXTENDER"
        )
    }
}

/// The top-level dump of the entire instruction set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstructionSetDump {
    /// Version string for the dump format.
    pub version: String,
    /// Total number of defs parsed (before filtering).
    pub total_parsed: usize,
    /// The instruction definitions (after filtering).
    pub instructions: Vec<InstructionDef>,
}
