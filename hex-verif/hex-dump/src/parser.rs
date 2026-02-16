use anyhow::{Context, Result};
use regex::Regex;

use crate::types::{InstructionDef, InstructionSetDump, Operand};

/// Parse the HexagonDepInstrInfo.td tablegen file into an InstructionSetDump.
pub fn parse_tablegen(content: &str) -> Result<InstructionSetDump> {
    let blocks = split_into_def_blocks(content);
    let total_parsed = blocks.len();
    let mut instructions = Vec::new();

    for (name, block) in &blocks {
        match parse_def_block(name, block) {
            Ok(insn) => {
                if !insn.should_filter() {
                    instructions.push(insn);
                }
            }
            Err(e) => {
                eprintln!("Warning: failed to parse def {}: {}", name, e);
            }
        }
    }

    Ok(InstructionSetDump {
        version: "1.0".to_string(),
        total_parsed,
        instructions,
    })
}

/// Split the file content into (name, block_text) pairs.
/// Each block starts with `def <NAME> : HInst<` and ends with `}` at column 0.
fn split_into_def_blocks(content: &str) -> Vec<(String, String)> {
    let def_re = Regex::new(r"^def (\w+)\s*:\s*HInst<").unwrap();
    let mut blocks = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_lines: Vec<String> = Vec::new();

    for line in content.lines() {
        if let Some(caps) = def_re.captures(line) {
            // If we were accumulating a block, save it
            if let Some(name) = current_name.take() {
                blocks.push((name, current_lines.join("\n")));
                current_lines.clear();
            }
            current_name = Some(caps[1].to_string());
            current_lines.push(line.to_string());
        } else if current_name.is_some() {
            current_lines.push(line.to_string());
            // Check if we've reached the end of the block
            if line == "}" {
                if let Some(name) = current_name.take() {
                    blocks.push((name, current_lines.join("\n")));
                    current_lines.clear();
                }
            }
        }
    }
    // Handle last block if file doesn't end with }
    if let Some(name) = current_name {
        blocks.push((name, current_lines.join("\n")));
    }

    blocks
}

/// Parse a single def block into an InstructionDef.
fn parse_def_block(name: &str, block: &str) -> Result<InstructionDef> {
    let mut insn = InstructionDef::new(name.to_string());

    // Parse the HInst header: (outs ...), (ins ...), "asm", timing, Type
    parse_header(&mut insn, block).with_context(|| format!("parsing header for {}", name))?;

    // Parse let statements in the body
    parse_let_statements(&mut insn, block);

    // Parse Requires from the header line
    parse_requires(&mut insn, block);

    Ok(insn)
}

/// Parse the HInst<(outs ...), (ins ...), "asm", timing, Type> header.
fn parse_header(insn: &mut InstructionDef, block: &str) -> Result<()> {
    // Extract outs
    let outs_re = Regex::new(r"\(outs\s*(.*?)\)").unwrap();
    if let Some(caps) = outs_re.captures(block) {
        insn.outs = parse_operand_list(&caps[1]);
    }

    // Extract ins
    let ins_re = Regex::new(r"\(ins\s*(.*?)\)").unwrap();
    if let Some(caps) = ins_re.captures(block) {
        insn.ins = parse_operand_list(&caps[1]);
    }

    // Extract assembly syntax (the quoted string)
    let asm_re = Regex::new(r#""([^"]+)""#).unwrap();
    if let Some(caps) = asm_re.captures(block) {
        insn.asm_syntax = caps[1].to_string();
    }

    // Extract Type from header line: ..., TypeXXX>, ... or ..., TypeXXX> {
    let type_re = Regex::new(r",\s*(Type\w+)\s*>").unwrap();
    if let Some(caps) = type_re.captures(block) {
        insn.itype = caps[1].to_string();
    } else {
        // Also check for PSEUDO as the timing class followed by Type
        let type_re2 = Regex::new(r"(?:PSEUDO|tc_\w+),\s*(Type\w+)\s*>").unwrap();
        if let Some(caps) = type_re2.captures(block) {
            insn.itype = caps[1].to_string();
        }
    }

    Ok(())
}

/// Parse an operand list like "IntRegs:$Rd32, IntRegs:$Rs32" or "s32_0Imm:$Ii".
fn parse_operand_list(text: &str) -> Vec<Operand> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }

    let mut operands = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        operands.push(parse_single_operand(part));
    }
    operands
}

/// Parse a single operand like "IntRegs:$Rd32" or "s32_0Imm:$Ii" or "ModRegs:$Mu2".
fn parse_single_operand(text: &str) -> Operand {
    let text = text.trim();
    // Format: Type:$Name
    if let Some((type_part, name_part)) = text.split_once(":$") {
        let is_immediate = is_immediate_type(type_part);
        Operand {
            name: name_part.to_string(),
            reg_class: if is_immediate {
                None
            } else {
                Some(type_part.to_string())
            },
            is_immediate,
            imm_type: if is_immediate {
                Some(type_part.to_string())
            } else {
                None
            },
        }
    } else {
        // Fallback: treat as unknown
        Operand {
            name: text.to_string(),
            reg_class: None,
            is_immediate: false,
            imm_type: None,
        }
    }
}

/// Check if a type name represents an immediate value rather than a register class.
fn is_immediate_type(type_name: &str) -> bool {
    // Immediate types look like: s32_0Imm, u10_0Imm, b30_2Imm, a30_2Imm, etc.
    // Register classes look like: IntRegs, DoubleRegs, PredRegs, HvxVR, ModRegs, etc.
    // Immediates typically start with lowercase and contain "Imm"
    type_name.contains("Imm")
}

/// Parse all `let ATTR = VALUE;` lines in the block body.
fn parse_let_statements(insn: &mut InstructionDef, block: &str) {
    for line in block.lines() {
        let line = line.trim();
        if !line.starts_with("let ") {
            continue;
        }

        // Parse `let NAME = VALUE;`
        if let Some((attr, value)) = parse_let_line(line) {
            apply_attribute(insn, &attr, &value);
        }
    }
}

/// Parse a single `let NAME = VALUE;` line, returning (name, value).
fn parse_let_line(line: &str) -> Option<(String, String)> {
    let line = line.trim().strip_prefix("let ")?.strip_suffix(';')?;
    let (attr, value) = line.split_once('=')?;
    Some((attr.trim().to_string(), value.trim().to_string()))
}

/// Apply a parsed attribute to an InstructionDef.
fn apply_attribute(insn: &mut InstructionDef, attr: &str, value: &str) {
    match attr {
        "isPseudo" => insn.is_pseudo = value == "1",
        "isCodeGenOnly" => insn.is_code_gen_only = value == "1",
        "isSolo" => insn.is_solo = value == "1",
        "isSoloAX" => insn.is_solo_ax = value == "1",
        "isPredicated" => insn.is_predicated = value == "1",
        "isPredicatedFalse" => insn.is_predicated_false = value == "1",
        "isPredicatedNew" => insn.is_predicated_new = value == "1",
        "hasNewValue" => insn.has_new_value = value == "1",
        "isNVStore" => insn.is_nv_store = value == "1",
        "isNVStorable" => insn.is_nv_storable = value == "1",
        "isFloat" => insn.is_fp = value == "1",
        "isCVI" => insn.is_cvi = value == "1",
        "isHVXALU" => insn.is_hvx_alu = value == "1",
        "isHVXALU2SRC" => insn.is_hvx_alu_2src = value == "1",
        "mayLoad" => insn.may_load = value == "1",
        "mayStore" => insn.may_store = value == "1",
        "isCommutable" => insn.is_commutable = value == "1",
        "isPredicable" => insn.is_predicable = value == "1",
        "isExtendable" => insn.is_extendable = value == "1",
        "isExtentSigned" => insn.is_extent_signed = value == "1",
        "hasSideEffects" => insn.has_side_effects = value == "1",
        "isCall" => insn.is_call = value == "1",
        "isReturn" => insn.is_return = value == "1",
        "prefersSlot3" => insn.prefers_slot3 = value == "1",
        "cofMax1" => insn.cof_max1 = value == "1",
        "cofRelax1" => insn.cof_relax1 = value == "1",
        "cofRelax2" => insn.cof_relax2 = value == "1",
        "opNewValue" => insn.op_new_value = value.parse().ok(),
        "opExtentBits" => insn.op_extent_bits = value.parse().ok(),
        "opExtentAlign" => insn.op_extent_align = value.parse().ok(),
        "opExtendable" => insn.op_extendable = value.parse().ok(),
        "Defs" => insn.defs = parse_bracket_list(value),
        "Uses" => insn.uses = parse_bracket_list(value),
        "Constraints" => {
            let v = value.trim_matches('"').to_string();
            if !v.is_empty() {
                insn.constraints = Some(v);
            }
        }
        _ => {} // Ignore other attributes (Inst bits, encoding, etc.)
    }
}

/// Parse a bracket list like `[USR_OVF]` or `[PC, R31]` into a Vec<String>.
fn parse_bracket_list(value: &str) -> Vec<String> {
    let inner = value
        .trim()
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(value);
    inner
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parse `Requires<[...]>` from the header portion of the block.
fn parse_requires(insn: &mut InstructionDef, block: &str) {
    let req_re = Regex::new(r"Requires<\[([^\]]+)\]>").unwrap();
    if let Some(caps) = req_re.captures(block) {
        insn.requires = caps[1]
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TABLEGEN: &str = r#"
def A2_add : HInst<
(outs IntRegs:$Rd32),
(ins IntRegs:$Rs32, IntRegs:$Rt32),
"$Rd32 = add($Rs32,$Rt32)",
tc_713b66bf, TypeALU32_3op>, Enc_5ab2be, PredNewRel, ImmRegRel {
let Inst{7-5} = 0b000;
let Inst{13-13} = 0b0;
let Inst{31-21} = 0b11110011000;
let hasNewValue = 1;
let opNewValue = 0;
let BaseOpcode = "A2_add";
let CextOpcode = "A2_add";
let InputType = "reg";
let isCommutable = 1;
let isPredicable = 1;
}
def A2_addsp : HInst<
(outs DoubleRegs:$Rdd32),
(ins IntRegs:$Rs32, DoubleRegs:$Rtt32),
"$Rdd32 = add($Rs32,$Rtt32)",
tc_01d44cb2, TypeALU64> {
let isPseudo = 1;
}
def A2_abssat : HInst<
(outs IntRegs:$Rd32),
(ins IntRegs:$Rs32),
"$Rd32 = abs($Rs32):sat",
tc_d61dfdc3, TypeS_2op>, Enc_5e2823 {
let Inst{13-5} = 0b000000101;
let Inst{31-21} = 0b10001100100;
let hasNewValue = 1;
let opNewValue = 0;
let prefersSlot3 = 1;
let Defs = [USR_OVF];
}
def V6_vabsb : HInst<
(outs HvxVR:$Vd32),
(ins HvxVR:$Vu32),
"$Vd32.b = vabs($Vu32.b)",
tc_0ec46cf9, TypeCVI_VA>, Enc_e7581c, Requires<[UseHVXV65]> {
let Inst{7-5} = 0b100;
let Inst{13-13} = 0b0;
let Inst{31-16} = 0b0001111000000001;
let hasNewValue = 1;
let opNewValue = 0;
let isCVI = 1;
let isHVXALU = 1;
let isHVXALU2SRC = 1;
let DecoderNamespace = "EXT_mmvec";
}
def A2_combineii : HInst<
(outs DoubleRegs:$Rdd32),
(ins s32_0Imm:$Ii, s8_0Imm:$II),
"$Rdd32 = combine(#$Ii,#$II)",
tc_713b66bf, TypeALU32_2op>, Enc_18c338 {
let Inst{31-23} = 0b011111000;
let isExtendable = 1;
let opExtendable = 1;
let isExtentSigned = 1;
let opExtentBits = 8;
let opExtentAlign = 0;
}
"#;

    #[test]
    fn test_parse_tablegen_basic() {
        let dump = parse_tablegen(SAMPLE_TABLEGEN).unwrap();
        // A2_addsp is pseudo, so filtered out. 4 real instructions remain.
        assert_eq!(dump.total_parsed, 5);
        assert_eq!(dump.instructions.len(), 4);
    }

    #[test]
    fn test_parse_a2_add() {
        let dump = parse_tablegen(SAMPLE_TABLEGEN).unwrap();
        let a2_add = dump
            .instructions
            .iter()
            .find(|i| i.name == "A2_add")
            .unwrap();
        assert_eq!(a2_add.itype, "TypeALU32_3op");
        assert_eq!(a2_add.outs.len(), 1);
        assert_eq!(a2_add.ins.len(), 2);
        assert_eq!(a2_add.outs[0].name, "Rd32");
        assert_eq!(a2_add.outs[0].reg_class.as_deref(), Some("IntRegs"));
        assert!(!a2_add.outs[0].is_immediate);
        assert_eq!(a2_add.ins[0].name, "Rs32");
        assert_eq!(a2_add.ins[1].name, "Rt32");
        assert!(a2_add.has_new_value);
        assert_eq!(a2_add.op_new_value, Some(0));
        assert!(a2_add.is_commutable);
        assert!(a2_add.is_predicable);
        assert_eq!(a2_add.asm_syntax, "$Rd32 = add($Rs32,$Rt32)");
    }

    #[test]
    fn test_parse_cvi_instruction() {
        let dump = parse_tablegen(SAMPLE_TABLEGEN).unwrap();
        let vabs = dump
            .instructions
            .iter()
            .find(|i| i.name == "V6_vabsb")
            .unwrap();
        assert_eq!(vabs.itype, "TypeCVI_VA");
        assert!(vabs.is_cvi);
        assert!(vabs.is_hvx_alu);
        assert_eq!(vabs.requires, vec!["UseHVXV65"]);
        assert_eq!(vabs.outs[0].reg_class.as_deref(), Some("HvxVR"));
    }

    #[test]
    fn test_parse_defs_uses() {
        let dump = parse_tablegen(SAMPLE_TABLEGEN).unwrap();
        let abssat = dump
            .instructions
            .iter()
            .find(|i| i.name == "A2_abssat")
            .unwrap();
        assert_eq!(abssat.defs, vec!["USR_OVF"]);
        assert!(abssat.prefers_slot3);
    }

    #[test]
    fn test_parse_immediate_operands() {
        let dump = parse_tablegen(SAMPLE_TABLEGEN).unwrap();
        let combine = dump
            .instructions
            .iter()
            .find(|i| i.name == "A2_combineii")
            .unwrap();
        assert_eq!(combine.ins.len(), 2);
        assert!(combine.ins[0].is_immediate);
        assert_eq!(combine.ins[0].imm_type.as_deref(), Some("s32_0Imm"));
        assert!(combine.ins[1].is_immediate);
        assert_eq!(combine.ins[1].imm_type.as_deref(), Some("s8_0Imm"));
        assert!(combine.is_extendable);
        assert!(combine.is_extent_signed);
        assert_eq!(combine.op_extent_bits, Some(8));
    }

    #[test]
    fn test_pseudo_filtered() {
        let dump = parse_tablegen(SAMPLE_TABLEGEN).unwrap();
        assert!(dump.instructions.iter().all(|i| i.name != "A2_addsp"));
    }
}
