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

    // Extract scheduling class and Type from header line:
    //   "asm", tc_XXXXX, TypeYYY>, ...
    let sched_type_re = Regex::new(r"(tc_\w+),\s*(Type\w+)\s*>").unwrap();
    if let Some(caps) = sched_type_re.captures(block) {
        insn.slot_mask = sched_class_to_slot_mask(&caps[1]);
        insn.itype = caps[2].to_string();
    } else {
        // Fallback: extract just the Type (PSEUDO timing class, etc.)
        let type_re = Regex::new(r",\s*(Type\w+)\s*>").unwrap();
        if let Some(caps) = type_re.captures(block) {
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
        "isFP" => insn.is_fp = value == "1",
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
        "isRestrictNoSlot1Store" => insn.is_restrict_no_slot1_store = value == "1",
        "isRestrictSlot1AOK" => insn.is_restrict_slot1_aok = value == "1",
        "isNewValue" => insn.is_new_value = value == "1",
        "isPredicateLate" => insn.is_predicate_late = value == "1",
        "isBranch" => insn.is_branch = value == "1",
        "isAccumulator" => insn.is_accumulator = value == "1",
        "CVINew" => insn.cvi_new = value == "1",
        "hasHvxTmp" => insn.has_hvx_tmp = value == "1",
        "addrMode" => insn.addr_mode = parse_addr_mode(value),
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

/// Map an addrMode symbolic name to a numeric ID.
fn parse_addr_mode(value: &str) -> u32 {
    match value {
        "Absolute" => 1,
        "AbsoluteSet" => 2,
        "BaseImmOffset" => 3,
        "BaseLongOffset" => 4,
        "BaseRegOffset" => 5,
        "PostInc" => 6,
        _ => 0,
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

/// Map scheduling class name to VLIW slot mask.
/// Generated from HexagonDepIICScalar.td and HexagonDepIICHVX.td.
fn sched_class_to_slot_mask(sched_class: &str) -> u8 {
    match sched_class {
        "tc_011e0e9d" => 0x1,
        "tc_01d44cb2" => 0xc,
        "tc_01e1be3b" => 0xc,
        "tc_02fe1c65" => 0xc,
        "tc_0390c1ca" => 0x3,
        "tc_04da405a" => 0xf,
        "tc_05ca8cfd" => 0xf,
        "tc_0655b949" => 0x3,
        "tc_075c8dd8" => 0x3,
        "tc_08a4f1b6" => 0xc,
        "tc_0a195f2c" => 0xc,
        "tc_0a43be35" => 0x8,
        "tc_0a6c20ae" => 0x1,
        "tc_0afc8be9" => 0xc,
        "tc_0b04c6c7" => 0xc,
        "tc_0ba0d5da" => 0x4,
        "tc_0dfac0a7" => 0xc,
        "tc_0ec46cf9" => 0xf,
        "tc_0fac1eb8" => 0x1,
        "tc_112d30d6" => 0xf,
        "tc_1242dc2a" => 0x1,
        "tc_1248597c" => 0x8,
        "tc_131f1c81" => 0x1,
        "tc_1381a97c" => 0xf,
        "tc_139ef484" => 0x4,
        "tc_14ab4f41" => 0x1,
        "tc_151bf368" => 0xc,
        "tc_158aa3f7" => 0x1,
        "tc_15fdf750" => 0xc,
        "tc_16ff9ef8" => 0xf,
        "tc_191381c1" => 0x1,
        "tc_197dce51" => 0x8,
        "tc_1981450d" => 0x1,
        "tc_1ad8a370" => 0xc,
        "tc_1ba8a0cd" => 0x3,
        "tc_1c2c7a4a" => 0xf,
        "tc_1c7522a8" => 0x3,
        "tc_1d41f8b7" => 0xc,
        "tc_1fcb8495" => 0xc,
        "tc_1fe4ab69" => 0x3,
        "tc_20131976" => 0xc,
        "tc_20a4bbec" => 0x1,
        "tc_2237d952" => 0x1,
        "tc_227864f7" => 0x1,
        "tc_23708a21" => 0xf,
        "tc_2471c1c8" => 0x1,
        "tc_24e109c7" => 0x1,
        "tc_24f426ab" => 0xf, // V73: all slots (was 0xc on V5)
        "tc_257f6f7c" => 0xf,
        "tc_26a377fe" => 0xc,
        "tc_27106296" => 0x8,
        "tc_280f7fe1" => 0x3,
        "tc_28e55c6f" => 0x8,
        "tc_2a698a03" => 0xf,
        "tc_2b4c548e" => 0xc,
        "tc_2c13e7f5" => 0xc,
        "tc_2c3e17fc" => 0x8,
        "tc_2c745bb8" => 0xf,
        "tc_2d4051cd" => 0xc,
        "tc_2e8f5f6e" => 0xc,
        "tc_2f573607" => 0x4,
        "tc_309dbb4f" => 0xf,
        "tc_33e7e673" => 0x4,
        "tc_362b0be2" => 0x4,
        "tc_37820f4c" => 0xc,
        "tc_38382228" => 0xc,
        "tc_388f9897" => 0xf,
        "tc_38e0bae9" => 0xc,
        "tc_3904b926" => 0x3,
        "tc_3aacf4a8" => 0xf,
        "tc_3ad719fb" => 0x3,
        "tc_3c56e5ce" => 0x1,
        "tc_3c8c15d0" => 0xc,
        "tc_3ce09744" => 0x1,
        "tc_3d14a17b" => 0x3,
        "tc_3e2aaafc" => 0x1,
        "tc_3edca78f" => 0x8,
        "tc_3fbf1042" => 0x3,
        "tc_407e96f9" => 0xc,
        "tc_40d64c94" => 0x1,
        "tc_4222e6bf" => 0x3,
        "tc_42ff66ba" => 0x4,
        "tc_442395f3" => 0xf,
        "tc_447d9895" => 0x1,
        "tc_449acf79" => 0x3,
        "tc_44d5a428" => 0x3,
        "tc_44fffc58" => 0xc,
        "tc_453fe68d" => 0x3,
        "tc_45791fb8" => 0x3,
        "tc_45f9d1be" => 0x4,
        "tc_46c18ecf" => 0x8,
        "tc_46d6c3e0" => 0xf,
        "tc_4942646a" => 0xc,
        "tc_49fdfd4b" => 0x8,
        "tc_4a55d03c" => 0xc,
        "tc_4abdbdc6" => 0x8,
        "tc_4ac61d92" => 0xf,
        "tc_4bf903b0" => 0x1,
        "tc_503ce0f3" => 0xc,
        "tc_512b1653" => 0x1,
        "tc_51d0ecc3" => 0xf,
        "tc_52447ecc" => 0x3,
        "tc_531b383c" => 0xf,
        "tc_53c851ab" => 0x4,
        "tc_540c3da3" => 0x1,
        "tc_54a0dc47" => 0x1,
        "tc_54f0cee2" => 0x8,
        "tc_5502c366" => 0xc,
        "tc_55255f2b" => 0x8,
        "tc_556f6577" => 0xc,
        "tc_55a9a350" => 0x1,
        "tc_55b33fda" => 0xc,
        "tc_561aaa58" => 0xf,
        "tc_56a124a7" => 0xf, // V73: all slots (was 0xc on V5)
        "tc_56c4f9fe" => 0xf,
        "tc_56e64202" => 0xf,
        "tc_57a4709c" => 0xf,
        "tc_57a55b54" => 0x8,
        "tc_58d21193" => 0x1,
        "tc_5944960d" => 0x3,
        "tc_59a7822c" => 0x3,
        "tc_5a222e89" => 0x4,
        "tc_5a4b5e58" => 0x8,
        "tc_5b347363" => 0x3,
        "tc_5bf8afbb" => 0xf,
        "tc_5cdf8c84" => 0xc,
        "tc_5ceb2f9e" => 0x3,
        "tc_5da50c4b" => 0xc,
        "tc_5deb5e47" => 0x1,
        "tc_5e4cf0e8" => 0xc,
        "tc_5f2afaf7" => 0x3,
        "tc_60e324ff" => 0x4,
        "tc_61bf7c03" => 0xc,
        "tc_63567288" => 0x3,
        "tc_649072c2" => 0xc,
        "tc_64b00d8a" => 0x1,
        "tc_651cbe02" => 0xc,
        "tc_65279839" => 0xc,
        "tc_65cbd974" => 0x3,
        "tc_660769f1" => 0xc,
        "tc_663c80a7" => 0x3,
        "tc_6942b6e0" => 0x1,
        "tc_69bfb303" => 0xc,
        "tc_6aa823ab" => 0x8,
        "tc_6ae3426b" => 0x8,
        "tc_6d861a95" => 0x8,
        "tc_6e20402a" => 0x1,
        "tc_6e7fa133" => 0xf,
        "tc_6f42bc60" => 0x1,
        "tc_6fb52018" => 0x1,
        "tc_6fc5dbea" => 0xc,
        "tc_7095ecba" => 0x2,
        "tc_711c805f" => 0xf, // V73: all slots (was 0xc on V5)
        "tc_713b66bf" => 0xf,
        "tc_71646d06" => 0xf,
        "tc_7177e272" => 0x1,
        "tc_718b5c53" => 0xf,
        "tc_7273323b" => 0x1,
        "tc_72e2b393" => 0xc,
        "tc_73efe966" => 0xc,
        "tc_7401744f" => 0xc,
        "tc_7417e785" => 0xf,
        "tc_7476d766" => 0x8,
        "tc_74a42bda" => 0x3,
        "tc_759e57be" => 0x4,
        "tc_767c4e9d" => 0xf,
        "tc_76bb5435" => 0x3,
        "tc_77f94a5e" => 0x1,
        "tc_788b1d09" => 0xc,
        "tc_78f87ed3" => 0x1,
        "tc_7af3a37e" => 0x1,
        "tc_7b9187d3" => 0x1,
        "tc_7c28bd7e" => 0x1,
        "tc_7c31e19a" => 0x3,
        "tc_7c6d32e4" => 0x3,
        "tc_7d68d5c2" => 0x2,
        "tc_7d6a2568" => 0x4,
        "tc_7dc63b5c" => 0x8,
        "tc_7e6a3e89" => 0xf,
        "tc_7f58404a" => 0x8,
        "tc_7f7f45f5" => 0xc,
        "tc_7f8ae742" => 0xc,
        "tc_8035e91f" => 0x3,
        "tc_822c3c68" => 0x3,
        "tc_829d8a86" => 0x1,
        "tc_838c4d7a" => 0x3,
        "tc_84a7500d" => 0xf,
        "tc_86173609" => 0xf,
        "tc_8772086c" => 0xf,
        "tc_87adc037" => 0xf,
        "tc_887d1bb7" => 0x3,
        "tc_8a6d0d94" => 0x3,
        "tc_8a825db2" => 0xc,
        "tc_8b5bd4f5" => 0xf,
        "tc_8e420e4d" => 0x1,
        "tc_8e82e8ca" => 0x3,
        "tc_8f36a2fd" => 0x3,
        "tc_90bcc1db" => 0x4,
        "tc_9124c04f" => 0xf,
        "tc_92240447" => 0x1,
        "tc_933f2b39" => 0xc,
        "tc_934753bb" => 0x1,
        "tc_937dd41c" => 0x3,
        "tc_9406230a" => 0x8,
        "tc_946013d8" => 0xf,
        "tc_95a33176" => 0xf,
        "tc_95f43c5e" => 0x4,
        "tc_96ef76ef" => 0x1,
        "tc_975a4e54" => 0x1,
        "tc_9783714b" => 0xc,
        "tc_9a1cab75" => 0x3,
        "tc_9aff7a2a" => 0x1,
        "tc_9b20a062" => 0x4,
        "tc_9b34f5e0" => 0x4,
        "tc_9b3c0462" => 0xc,
        "tc_9bcfb2ee" => 0x1,
        "tc_9c52f549" => 0xf,
        "tc_9d1dc972" => 0xf,
        "tc_9e27f2f9" => 0xf, // V73: all slots (was 0xc on V5)
        "tc_9e72dc89" => 0xc,
        "tc_9edb7c77" => 0xc,
        "tc_9edefe01" => 0x3,
        "tc_9f363d21" => 0x1,
        "tc_9f6cd987" => 0xc,
        "tc_a02a10a8" => 0x1,
        "tc_a08b630b" => 0xc,
        "tc_a0dbea28" => 0x3,
        "tc_a1297125" => 0xc,
        "tc_a154b476" => 0xc,
        "tc_a19b9305" => 0xc,
        "tc_a28f32b5" => 0x2,
        "tc_a2b365d2" => 0x3,
        "tc_a3070909" => 0x1,
        "tc_a32e03e7" => 0x3,
        "tc_a38c45dc" => 0xc,
        "tc_a4e22bbd" => 0xc,
        "tc_a4ee89db" => 0x1,
        "tc_a69eeee1" => 0x2,
        "tc_a724463d" => 0x1,
        "tc_a7a13fac" => 0xc,
        "tc_a7bdb22c" => 0xc,
        "tc_a7e6707d" => 0x1,
        "tc_a9edeffa" => 0x3,
        "tc_ab23f776" => 0x1,
        "tc_abe8c3b2" => 0x3,
        "tc_abfd9a6d" => 0x3,
        "tc_ac4046bc" => 0xc,
        "tc_ac65613f" => 0x3,
        "tc_addc37a8" => 0x1,
        "tc_ae5babd7" => 0x3,
        "tc_aee6250c" => 0x3,
        "tc_af25efd9" => 0xf,
        "tc_af6af259" => 0x3,
        "tc_b091f1c6" => 0xc,
        "tc_b1ae5f67" => 0x1,
        "tc_b2196a3f" => 0x8,
        "tc_b28e51aa" => 0xf,
        "tc_b3d46584" => 0x1,
        "tc_b4416217" => 0xf,
        "tc_b4dc7630" => 0x3,
        "tc_b7c4062a" => 0x3,
        "tc_b837298f" => 0xf,
        "tc_b9bec29e" => 0x4,
        "tc_b9db8205" => 0x3,
        "tc_ba9255a6" => 0x3,
        "tc_bb07f2c5" => 0x3,
        "tc_bb599486" => 0xc,
        "tc_bb78483e" => 0x8,
        "tc_bb831a7c" => 0xc,
        "tc_bf2ffc0f" => 0x3,
        "tc_c0749f3c" => 0x3,
        "tc_c127de3a" => 0xc,
        "tc_c20701f0" => 0xc,
        "tc_c21d7447" => 0xc,
        "tc_c4edf264" => 0xc,
        "tc_c57d9f39" => 0xf,
        "tc_c5dba46e" => 0x1,
        "tc_c7039829" => 0x1,
        "tc_c818ff7f" => 0x1,
        "tc_cd94bfe0" => 0xc,
        "tc_cda936da" => 0xc,
        "tc_ce59038e" => 0x1,
        "tc_cfa0e29b" => 0x1,
        "tc_d03278fd" => 0x3,
        "tc_d234b61a" => 0x1,
        "tc_d33e5eee" => 0xf,
        "tc_d3632d88" => 0xc,
        "tc_d45ba9cd" => 0x1,
        "tc_d57d649c" => 0x4,
        "tc_d61dfdc3" => 0xc,
        "tc_d68dca5c" => 0xc,
        "tc_d71ea8fa" => 0x8,
        "tc_d7718fbe" => 0x8,
        "tc_d8287c14" => 0xc,
        "tc_db5555f3" => 0xf,
        "tc_db596beb" => 0xc,
        "tc_db96aa6b" => 0x1,
        "tc_dc51281d" => 0x4,
        "tc_dcca380f" => 0xc,
        "tc_dd5b0695" => 0x3,
        "tc_decdde8a" => 0xf, // V73: all slots (was 0xc on V5)
        "tc_df5d53f9" => 0x1,
        "tc_df80eeb0" => 0xf,
        "tc_e2d2e9e5" => 0x1,
        "tc_e2fdd6e6" => 0xf,
        "tc_e35c1e93" => 0xf,
        "tc_e3d699e3" => 0xc,
        "tc_e3f68a46" => 0xf,
        "tc_e60def48" => 0xc, // V73: SLOT2,3 (was 0x4 on V5)
        "tc_e675c45a" => 0xc,
        "tc_e699ae41" => 0x3,
        "tc_e9170fb7" => 0x3,
        "tc_e99d4c2e" => 0x1,
        "tc_ed03645c" => 0x4,
        "tc_ed3f8d2a" => 0x1,
        "tc_eed07714" => 0x3,
        "tc_eeda4109" => 0xf, // V73: all slots (was 0xc on V5)
        "tc_ef921005" => 0xc,
        "tc_f098b237" => 0xc,
        "tc_f0cdeccf" => 0xc,
        "tc_f0e8e832" => 0xc,
        "tc_f175e046" => 0xc,
        "tc_f1de44ef" => 0x4,
        "tc_f21e8abb" => 0x1,
        "tc_f34c1c21" => 0xc,
        "tc_f38f92e1" => 0x1,
        "tc_f529831b" => 0x1,
        "tc_f6e2aff9" => 0x1,
        "tc_f7569068" => 0xc,
        "tc_f97707c1" => 0x4,
        "tc_f999c66e" => 0xf, // V73: all slots (was 0xc on V5)
        "tc_fae9dfa5" => 0x8,
        "tc_fedb7e19" => 0x3, // V73: SLOT0,1 (tc_ld)
        _ => 0xf,             // default: all slots
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
        // tc_713b66bf → 0xf (all slots)
        assert_eq!(a2_add.slot_mask, 0xf);
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
        // tc_0ec46cf9 → 0xf (all slots)
        assert_eq!(vabs.slot_mask, 0xf);
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
        // tc_d61dfdc3 → 0xc (slots 2-3)
        assert_eq!(abssat.slot_mask, 0xc);
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
