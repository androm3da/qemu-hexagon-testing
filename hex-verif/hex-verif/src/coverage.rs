use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use hex_dump::types::{InstructionDef, Operand};
use hex_instset::database::InstructionDb;
use hex_instset::register::RegisterClass;

/// Metadata for a single generated instruction.
#[derive(Debug, Clone)]
pub struct GeneratedInstruction {
    pub name: String,
    pub itype: String,
    pub asm_text: String,
}

/// Static encoding space analysis of the instruction set.
pub struct EncodingSpace {
    /// Per-instruction encoding count (name -> number of distinct encodings).
    pub per_instruction: HashMap<String, u128>,
    /// Total encoding space (sum across all instructions).
    pub total: u128,
    /// Number of instructions included.
    pub instruction_count: usize,
}

impl EncodingSpace {
    /// Compute encoding space for all real (non-pseudo) instructions in the database.
    pub fn compute(db: &InstructionDb) -> Self {
        let all: Vec<&InstructionDef> = db.all().iter().filter(|i| !i.should_filter()).collect();
        Self::compute_from_slice(&all)
    }

    /// Compute encoding space for a specific set of instruction names.
    pub fn compute_from_names(db: &InstructionDb, names: &[String]) -> Self {
        let insns: Vec<&InstructionDef> = names.iter().filter_map(|n| db.get(n)).collect();
        Self::compute_from_slice(&insns)
    }

    fn compute_from_slice(instructions: &[&InstructionDef]) -> Self {
        let mut per_instruction = HashMap::new();
        let mut total: u128 = 0;

        for insn in instructions {
            let space = encoding_space_for_instruction(insn);
            per_instruction.insert(insn.name.clone(), space);
            total = total.saturating_add(space);
        }

        EncodingSpace {
            instruction_count: instructions.len(),
            per_instruction,
            total,
        }
    }
}

/// Thread-safe coverage tracker for recording which instructions and encodings
/// have been exercised during testing.
pub struct CoverageTracker {
    inner: Mutex<CoverageInner>,
}

struct CoverageInner {
    instructions_seen: HashSet<String>,
    unique_encodings: HashSet<u64>,
    per_instruction_count: HashMap<String, usize>,
    per_itype_seen: HashMap<String, HashSet<String>>,
    total_instruction_words: usize,
    candidate_names: Option<Vec<String>>,
}

impl CoverageTracker {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(CoverageInner {
                instructions_seen: HashSet::new(),
                unique_encodings: HashSet::new(),
                per_instruction_count: HashMap::new(),
                per_itype_seen: HashMap::new(),
                total_instruction_words: 0,
                candidate_names: None,
            }),
        }
    }

    /// Record a batch of generated instructions.
    pub fn record(&self, instructions: &[GeneratedInstruction]) {
        let mut inner = self.inner.lock().unwrap();
        for gi in instructions {
            inner.instructions_seen.insert(gi.name.clone());
            inner
                .per_instruction_count
                .entry(gi.name.clone())
                .and_modify(|c| *c += 1)
                .or_insert(1);
            inner
                .per_itype_seen
                .entry(gi.itype.clone())
                .or_default()
                .insert(gi.name.clone());

            // Hash the asm_text to track unique encodings
            let h = hash_string(&gi.asm_text);
            inner.unique_encodings.insert(h);
            inner.total_instruction_words += 1;
        }
    }

    /// Store candidate names (synthesizable instruction pool) once.
    pub fn set_candidate_names(&self, names: Vec<String>) {
        let mut inner = self.inner.lock().unwrap();
        if inner.candidate_names.is_none() {
            inner.candidate_names = Some(names);
        }
    }

    /// Get the stored candidate names.
    pub fn candidate_names(&self) -> Option<Vec<String>> {
        self.inner.lock().unwrap().candidate_names.clone()
    }

    /// Generate a coverage report.
    pub fn report(&self, full_space: &EncodingSpace, synth_space: &EncodingSpace) -> String {
        let inner = self.inner.lock().unwrap();
        let mut lines = Vec::new();

        lines.push(String::new());
        lines.push("Encoding Space Coverage".to_string());
        lines.push("=======================".to_string());

        lines.push(format!(
            "Instruction database:  {} instructions ({} synthesizable)",
            full_space.instruction_count, synth_space.instruction_count,
        ));
        lines.push(format!(
            "Encoding space (full ISA):      {} encodings",
            format_large_number(full_space.total)
        ));
        lines.push(format!(
            "Encoding space (synthesizable): {} encodings",
            format_large_number(synth_space.total)
        ));

        lines.push(String::new());

        let insn_coverage_pct = if synth_space.instruction_count > 0 {
            inner.instructions_seen.len() as f64 / synth_space.instruction_count as f64 * 100.0
        } else {
            0.0
        };
        lines.push(format!(
            "Instruction coverage:  {} / {} synthesizable ({:.1}%)",
            inner.instructions_seen.len(),
            synth_space.instruction_count,
            insn_coverage_pct,
        ));

        let encoding_pct = if synth_space.total > 0 {
            inner.unique_encodings.len() as f64 / synth_space.total as f64 * 100.0
        } else {
            0.0
        };
        lines.push(format!(
            "Unique encodings:      {} / {} synthesizable ({:.6}%)",
            inner.unique_encodings.len(),
            format_large_number(synth_space.total),
            encoding_pct,
        ));

        lines.push(format!(
            "Total instruction words generated: {}",
            inner.total_instruction_words
        ));

        // Per-itype breakdown
        if !inner.per_itype_seen.is_empty() {
            lines.push(String::new());
            lines.push("By instruction type:".to_string());

            let mut itypes: Vec<_> = inner.per_itype_seen.iter().collect();
            itypes.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

            for (itype, names) in &itypes {
                // Count total synthesizable for this itype
                let synth_for_type = synth_space
                    .per_instruction
                    .iter()
                    .filter(|(name, _)| {
                        inner
                            .candidate_names
                            .as_ref()
                            .and_then(|cn| cn.iter().find(|n| *n == name.as_str()).map(|_| true))
                            .unwrap_or(false)
                            && self.itype_for_name(name, &inner) == Some(itype.as_str())
                    })
                    .count();

                // Count unique encodings for this itype
                let unique_for_type: usize = names
                    .iter()
                    .filter_map(|n| inner.per_instruction_count.get(n))
                    .sum();

                let type_total = if synth_for_type > 0 {
                    synth_for_type
                } else {
                    names.len()
                };

                lines.push(format!(
                    "  {:20} {:>4} / {:<4} instructions, {:>6} unique encodings",
                    itype,
                    names.len(),
                    type_total,
                    unique_for_type,
                ));
            }
        }

        lines.join("\n")
    }

    fn itype_for_name<'a>(&self, _name: &str, inner: &'a CoverageInner) -> Option<&'a str> {
        for (itype, names) in &inner.per_itype_seen {
            if names.contains(_name) {
                return Some(itype.as_str());
            }
        }
        None
    }
}

/// Compute the encoding space for a single instruction.
///
/// The space = product of operand cardinalities, where tied operands
/// (same name appearing in both outs and ins) are counted only once.
fn encoding_space_for_instruction(insn: &InstructionDef) -> u128 {
    // Collect unique operand names and their sizes
    let mut seen_names: HashSet<&str> = HashSet::new();
    let mut space: u128 = 1;

    for op in insn.outs.iter().chain(insn.ins.iter()) {
        if seen_names.contains(op.name.as_str()) {
            continue; // Tied operand, already counted
        }
        seen_names.insert(&op.name);

        let size = operand_encoding_size(op, insn);
        if size > 0 {
            space = space.saturating_mul(size as u128);
        }
    }

    space
}

/// Compute the number of possible values for a single operand.
fn operand_encoding_size(op: &Operand, insn: &InstructionDef) -> u64 {
    if op.is_immediate {
        let bits = parse_imm_encoding_bits(op.imm_type.as_deref())
            .or(insn.op_extent_bits)
            .unwrap_or(8);
        1u64.checked_shl(bits).unwrap_or(u64::MAX)
    } else if let Some(ref rc_name) = op.reg_class {
        RegisterClass::parse(rc_name)
            .map(|rc| rc.count() as u64)
            .unwrap_or(1)
    } else {
        1
    }
}

/// Parse the bit width from an immediate type string.
///
/// Format: `[a|b|s|u|n]<bits>_<align>Imm`
/// We strip any alphabetic prefix, then parse the digits before `_`.
fn parse_imm_encoding_bits(imm_type: Option<&str>) -> Option<u32> {
    let it = imm_type?;
    // Strip leading alphabetic characters
    let numeric_start = it.find(|c: char| c.is_ascii_digit())?;
    let rest = &it[numeric_start..];
    let num_part = rest.split('_').next()?;
    num_part.parse().ok()
}

/// Hash a string to a u64 using the default hasher.
fn hash_string(s: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Format a large u128 number with underscores for readability.
fn format_large_number(n: u128) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push('_');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_dump::types::{InstructionDef, InstructionSetDump, Operand};

    fn make_reg_operand(name: &str, rc: &str) -> Operand {
        Operand {
            name: name.to_string(),
            reg_class: Some(rc.to_string()),
            is_immediate: false,
            imm_type: None,
        }
    }

    fn make_imm_operand(name: &str, imm_type: &str) -> Operand {
        Operand {
            name: name.to_string(),
            reg_class: None,
            is_immediate: true,
            imm_type: Some(imm_type.to_string()),
        }
    }

    #[test]
    fn test_encoding_space_simple() {
        // 3 IntRegs operands: 32^3 = 32768
        let mut insn = InstructionDef::new("A2_add".to_string());
        insn.outs = vec![make_reg_operand("Rd32", "IntRegs")];
        insn.ins = vec![
            make_reg_operand("Rs32", "IntRegs"),
            make_reg_operand("Rt32", "IntRegs"),
        ];

        let space = encoding_space_for_instruction(&insn);
        assert_eq!(space, 32 * 32 * 32); // 32768
    }

    #[test]
    fn test_encoding_space_with_immediate() {
        // 1 IntRegs dest + 1 IntRegs src + 1 s8_0Imm: 32 * 32 * 256
        let mut insn = InstructionDef::new("A2_addi".to_string());
        insn.outs = vec![make_reg_operand("Rd32", "IntRegs")];
        insn.ins = vec![
            make_reg_operand("Rs32", "IntRegs"),
            make_imm_operand("Ii", "s8_0Imm"),
        ];

        let space = encoding_space_for_instruction(&insn);
        assert_eq!(space, 32 * 32 * 256); // 262144
    }

    #[test]
    fn test_encoding_space_tied_operands() {
        // Tied: Rx32 appears in both outs and ins, counted once
        // outs: Rx32 (IntRegs), ins: Rx32 (IntRegs, tied), Rs32 (IntRegs)
        let mut insn = InstructionDef::new("A2_addsat".to_string());
        insn.outs = vec![make_reg_operand("Rx32", "IntRegs")];
        insn.ins = vec![
            make_reg_operand("Rx32", "IntRegs"), // tied
            make_reg_operand("Rs32", "IntRegs"),
        ];

        let space = encoding_space_for_instruction(&insn);
        assert_eq!(space, 32 * 32); // 1024, not 32768
    }

    #[test]
    fn test_coverage_tracker_basic() {
        let tracker = CoverageTracker::new();

        let instructions = vec![
            GeneratedInstruction {
                name: "A2_add".to_string(),
                itype: "TypeALU32_3op".to_string(),
                asm_text: "r0 = add(r1,r2)".to_string(),
            },
            GeneratedInstruction {
                name: "A2_add".to_string(),
                itype: "TypeALU32_3op".to_string(),
                asm_text: "r3 = add(r4,r5)".to_string(),
            },
            GeneratedInstruction {
                name: "A2_sub".to_string(),
                itype: "TypeALU32_3op".to_string(),
                asm_text: "r0 = sub(r2,r1)".to_string(),
            },
        ];

        tracker.record(&instructions);

        let inner = tracker.inner.lock().unwrap();
        assert_eq!(inner.instructions_seen.len(), 2); // A2_add, A2_sub
        assert_eq!(inner.unique_encodings.len(), 3); // all different asm_text
        assert_eq!(inner.total_instruction_words, 3);
        assert_eq!(inner.per_instruction_count["A2_add"], 2);
        assert_eq!(inner.per_instruction_count["A2_sub"], 1);
    }

    #[test]
    fn test_parse_imm_encoding_bits() {
        assert_eq!(parse_imm_encoding_bits(Some("s8_0Imm")), Some(8));
        assert_eq!(parse_imm_encoding_bits(Some("u10_0Imm")), Some(10));
        assert_eq!(parse_imm_encoding_bits(Some("s32_0Imm")), Some(32));
        assert_eq!(parse_imm_encoding_bits(Some("b30_2Imm")), Some(30));
        assert_eq!(parse_imm_encoding_bits(Some("a1_0Imm")), Some(1));
        assert_eq!(parse_imm_encoding_bits(Some("n1_0Imm")), Some(1));
        assert_eq!(parse_imm_encoding_bits(None), None);
    }

    #[test]
    fn test_encoding_space_compute() {
        let mut insns = Vec::new();

        let mut add = InstructionDef::new("A2_add".to_string());
        add.itype = "TypeALU32_3op".to_string();
        add.outs = vec![make_reg_operand("Rd32", "IntRegs")];
        add.ins = vec![
            make_reg_operand("Rs32", "IntRegs"),
            make_reg_operand("Rt32", "IntRegs"),
        ];
        insns.push(add);

        let mut sub = InstructionDef::new("A2_sub".to_string());
        sub.itype = "TypeALU32_3op".to_string();
        sub.outs = vec![make_reg_operand("Rd32", "IntRegs")];
        sub.ins = vec![
            make_reg_operand("Rt32", "IntRegs"),
            make_reg_operand("Rs32", "IntRegs"),
        ];
        insns.push(sub);

        let dump = InstructionSetDump {
            version: "1.0".to_string(),
            total_parsed: 2,
            instructions: insns,
        };
        let db = InstructionDb::from_dump(dump);

        let space = EncodingSpace::compute(&db);
        assert_eq!(space.instruction_count, 2);
        assert_eq!(space.total, 32768 + 32768); // two 3-register instructions
        assert_eq!(space.per_instruction["A2_add"], 32768);
    }
}
