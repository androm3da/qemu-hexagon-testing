use hex_dump::types::InstructionDef;
use hex_instset::database::InstructionDb;
use hex_instset::filter::Filter;
use hex_instset::register::RegisterClass;
use rand::prelude::*;
use rand::rngs::StdRng;

use crate::constraint::{validate_concrete_packet, validate_packet};
use crate::slot::assign_slots;

/// A concrete instruction with register and immediate assignments.
#[derive(Debug, Clone)]
pub struct ConcreteInsn<'a> {
    /// The instruction definition.
    pub def: &'a InstructionDef,
    /// Concrete destination register names.
    pub dest_regs: Vec<String>,
    /// Concrete source register names.
    pub src_regs: Vec<String>,
    /// Concrete immediate values.
    pub immediates: Vec<i64>,
    /// The fully resolved assembly text.
    pub asm_text: String,
}

/// A synthesized VLIW packet.
#[derive(Debug, Clone)]
pub struct Packet<'a> {
    pub insns: Vec<ConcreteInsn<'a>>,
}

impl<'a> Packet<'a> {
    /// Format this packet as assembly text.
    pub fn to_asm(&self) -> String {
        if self.insns.len() == 1 {
            format!("    {{ {} }}", self.insns[0].asm_text)
        } else {
            let inner: Vec<&str> = self.insns.iter().map(|i| i.asm_text.as_str()).collect();
            format!("    {{ {} }}", inner.join(" ; "))
        }
    }
}

/// Configuration for the packet synthesizer.
pub struct SynthConfig {
    /// Maximum number of instructions per packet.
    pub max_packet_size: usize,
    /// Allow predicated instructions.
    pub allow_predicated: bool,
    /// Allow .new predicate forms.
    pub allow_predicated_new: bool,
    /// Allow new-value consumers.
    pub allow_new_value: bool,
    /// Maximum number of CVI instructions per packet.
    pub max_cvi_per_packet: usize,
    /// Terms to skip in instruction names/syntax.
    pub skip_terms: Vec<String>,
    /// Exclude filters (instructions matching any of these are excluded).
    pub exclude_filters: Vec<Filter>,
    /// Include filter (only instructions matching this are considered).
    pub include_filter: Option<Filter>,
    /// Feature blocklist (instructions requiring these are excluded).
    pub blocked_features: Vec<String>,
    /// Allow load/store instructions with memory-safe operand assignment.
    ///
    /// When true, the synthesizer forces the base register to r27 and
    /// constrains offsets to `[0, mem_region_size - access_size]`.
    pub allow_mem_ops: bool,
    /// Size of the memory region in bytes (used to constrain offsets).
    pub mem_region_size: usize,
}

impl Default for SynthConfig {
    fn default() -> Self {
        Self {
            max_packet_size: 4,
            allow_predicated: true,
            allow_predicated_new: false, // MVP: avoid .new predicates
            allow_new_value: false,      // MVP: avoid new-value consumers
            max_cvi_per_packet: 1,       // MVP: at most 1 CVI per packet
            skip_terms: Vec::new(),
            exclude_filters: Vec::new(),
            include_filter: None,
            blocked_features: Vec::new(),
            allow_mem_ops: false,
            mem_region_size: 65536,
        }
    }
}

/// Synthesizes legal VLIW packets from an instruction database.
pub struct PacketSynthesizer<'a> {
    _db: &'a InstructionDb,
    config: SynthConfig,
    /// Prefiltered candidate instructions.
    candidates: Vec<&'a InstructionDef>,
}

impl<'a> PacketSynthesizer<'a> {
    pub fn new(db: &'a InstructionDb, config: SynthConfig) -> Self {
        let candidates = Self::build_candidate_list(db, &config);
        Self {
            _db: db,
            config,
            candidates,
        }
    }

    /// Build the filtered candidate list based on config.
    fn build_candidate_list(
        db: &'a InstructionDb,
        config: &SynthConfig,
    ) -> Vec<&'a InstructionDef> {
        let mut result: Vec<&InstructionDef> = db
            .all()
            .iter()
            .filter(|insn| {
                // Apply exclude filters from recipe
                for ef in &config.exclude_filters {
                    if ef.matches(insn) {
                        return false;
                    }
                }

                // Apply built-in predicated_new / nv_store filters
                if !config.allow_predicated_new && insn.is_predicated_new {
                    return false;
                }
                if !config.allow_new_value && insn.is_nv_store {
                    return false;
                }

                // Apply include filter if present
                if let Some(ref inc) = config.include_filter {
                    if !inc.matches(insn) {
                        return false;
                    }
                }

                // Apply blocked features
                if insn.requires.iter().any(|r| {
                    config
                        .blocked_features
                        .iter()
                        .any(|b| r.contains(b.as_str()))
                }) {
                    return false;
                }

                // Apply skip terms
                !config.skip_terms.iter().any(|term| {
                    insn.asm_syntax
                        .to_lowercase()
                        .contains(&term.to_lowercase())
                        || insn.name.to_lowercase().contains(&term.to_lowercase())
                })
            })
            .collect();

        // Sort for deterministic iteration
        result.sort_by(|a, b| a.name.cmp(&b.name));
        result
    }

    /// Access the candidate list (for external filtering).
    pub fn candidates(&self) -> &[&'a InstructionDef] {
        &self.candidates
    }

    /// Synthesize a single legal packet.
    ///
    /// Retries until a valid packet is produced. With non-empty candidates this
    /// always succeeds, though exotic filter configs may require many attempts.
    pub fn synthesize_packet(&self, rng: &mut StdRng) -> Packet<'a> {
        assert!(
            !self.candidates.is_empty(),
            "synthesize_packet called with no candidates"
        );

        for attempt in 1.. {
            if let Some(packet) = self.try_synthesize(rng) {
                return packet;
            }
            if attempt == 1000 {
                eprintln!(
                    "Warning: packet synthesis struggling after {} attempts ({} candidates)",
                    attempt,
                    self.candidates.len()
                );
            }
        }
        unreachable!()
    }

    /// Single attempt at synthesizing a packet.
    fn try_synthesize(&self, rng: &mut StdRng) -> Option<Packet<'a>> {
        if self.candidates.is_empty() {
            return None;
        }

        // Pick target packet size (weighted toward 3-4)
        let size = self.pick_packet_size(rng);

        // Pick instructions one at a time, checking compatibility
        let mut selected: Vec<&InstructionDef> = Vec::new();
        let mut cvi_count = 0;

        for _ in 0..size {
            let insn = self.pick_compatible_insn(rng, &selected, cvi_count)?;
            if insn.is_cvi {
                cvi_count += 1;
            }
            selected.push(insn);
        }

        // Validate the packet
        if !validate_packet(&selected).valid {
            return None;
        }

        // Assign concrete operands
        let concrete = self.assign_operands(rng, &selected)?;

        // Validate concrete packet (register conflicts)
        if !validate_concrete_packet(&concrete).valid {
            return None;
        }

        Some(Packet { insns: concrete })
    }

    fn pick_packet_size(&self, rng: &mut StdRng) -> usize {
        let max = self.config.max_packet_size.min(4);
        // Weighted distribution favoring larger packets
        let weights = match max {
            1 => vec![1.0],
            2 => vec![0.3, 0.7],
            3 => vec![0.1, 0.3, 0.6],
            4 => vec![0.05, 0.15, 0.35, 0.45],
            _ => vec![0.25; max],
        };
        let total: f64 = weights.iter().sum();
        let mut r = rng.gen::<f64>() * total;
        for (i, w) in weights.iter().enumerate() {
            r -= w;
            if r <= 0.0 {
                return i + 1;
            }
        }
        max
    }

    /// Pick an instruction compatible with the already-selected instructions.
    fn pick_compatible_insn(
        &self,
        rng: &mut StdRng,
        selected: &[&'a InstructionDef],
        cvi_count: usize,
    ) -> Option<&'a InstructionDef> {
        // Build list of compatible candidates
        let compatible: Vec<&&InstructionDef> = self
            .candidates
            .iter()
            .filter(|&&insn| {
                // Check CVI limit
                if insn.is_cvi && cvi_count >= self.config.max_cvi_per_packet {
                    return false;
                }

                // Check slot compatibility
                let mut test_insns: Vec<&InstructionDef> = selected.to_vec();
                test_insns.push(insn);
                let itypes: Vec<&str> = test_insns.iter().map(|i| i.itype.as_str()).collect();
                if assign_slots(&itypes).is_none() {
                    return false;
                }

                // Check memory op limit
                let mem_count = test_insns
                    .iter()
                    .filter(|i| i.may_load || i.may_store)
                    .count();
                if mem_count > 2 {
                    return false;
                }

                // Check branch limit
                let branch_count = test_insns
                    .iter()
                    .filter(|i| matches!(i.itype.as_str(), "TypeJ" | "TypeCJ" | "TypeNCJ"))
                    .count();
                if branch_count > 2 {
                    return false;
                }

                // Check cofMax1
                let cof_count = test_insns.iter().filter(|i| i.cof_max1).count();
                if cof_count > 1 {
                    return false;
                }

                true
            })
            .collect();

        if compatible.is_empty() {
            return None;
        }

        Some(compatible[rng.gen_range(0..compatible.len())])
    }

    /// Assign concrete register and immediate values to the selected instructions.
    fn assign_operands(
        &self,
        rng: &mut StdRng,
        insns: &[&'a InstructionDef],
    ) -> Option<Vec<ConcreteInsn<'a>>> {
        let mut used_dest_regs: Vec<String> = Vec::new();
        let mut concrete = Vec::new();

        for insn in insns {
            let ci = self.assign_single_operands(rng, insn, &used_dest_regs)?;
            // Track destination registers, expanding double regs into component singles
            for dest in &ci.dest_regs {
                used_dest_regs.extend(expand_reg_aliases(dest));
            }
            concrete.push(ci);
        }

        Some(concrete)
    }

    /// Assign operands for a single instruction.
    fn assign_single_operands(
        &self,
        rng: &mut StdRng,
        insn: &'a InstructionDef,
        used_dests: &[String],
    ) -> Option<ConcreteInsn<'a>> {
        let mut dest_regs = Vec::new();
        let mut src_regs = Vec::new();
        let mut immediates = Vec::new();
        let mut asm = insn.asm_syntax.clone();

        let is_mem_op = self.config.allow_mem_ops && (insn.may_load || insn.may_store);
        let access_size = if is_mem_op {
            mem_access_size(&insn.asm_syntax)
        } else {
            0
        };
        // Track whether the first IntRegs source has been assigned (base register for mem ops)
        let mut base_reg_assigned = false;
        // Track whether the first immediate has been assigned (offset for mem ops)
        let mut mem_offset_assigned = false;

        // Assign output operands
        for op in &insn.outs {
            if op.is_immediate {
                continue;
            }
            if let Some(ref rc_name) = op.reg_class {
                let rc = RegisterClass::parse(rc_name)?;
                let mut safe = rc.safe_indices();
                if safe.is_empty() {
                    return None;
                }

                // When mem ops are enabled, protect r27 from being overwritten.
                // For IntRegs, exclude index 27 (r27).
                // For DoubleRegs, exclude index 13 (r27:26).
                if self.config.allow_mem_ops {
                    if rc == RegisterClass::IntRegs {
                        safe.retain(|&idx| idx != 27);
                    } else if rc == RegisterClass::DoubleRegs {
                        safe.retain(|&idx| idx != 13);
                    }
                    if safe.is_empty() {
                        return None;
                    }
                }

                // Pick a register not already used as a destination
                let mut attempts = 0;
                loop {
                    let idx = safe[rng.gen_range(0..safe.len())];
                    let name = rc.register_name(idx);
                    let aliases = expand_reg_aliases(&name);
                    let conflicts = aliases.iter().any(|a| used_dests.contains(a));
                    if !conflicts || attempts > 20 {
                        asm = asm.replace(&format!("${}", op.name), &name);
                        dest_regs.push(name);
                        break;
                    }
                    attempts += 1;
                }
            }
        }

        // Assign input operands
        for op in &insn.ins {
            if op.is_immediate {
                let imm = if is_mem_op && !mem_offset_assigned {
                    // First immediate of a memory op: constrain offset
                    mem_offset_assigned = true;
                    self.pick_mem_offset(rng, insn, op.imm_type.as_deref(), access_size)
                } else {
                    self.pick_immediate(rng, insn, op.imm_type.as_deref())
                };
                // The asm syntax typically already has `#` before `$Ii`, so we
                // replace `$Ii` with just the numeric value to avoid `##` (extender).
                // If the syntax has `#$Ii`, we get `#<value>`.
                // If the syntax has just `$Ii` (rare), we still need a `#`.
                let pattern = format!("${}", op.name);
                if asm.contains(&format!("#{}", pattern)) {
                    // Syntax already has #, just replace $name with number
                    asm = asm.replace(&pattern, &format!("{}", imm));
                } else {
                    // No # prefix, add one
                    asm = asm.replace(&pattern, &format!("#{}", imm));
                }
                immediates.push(imm);
            } else if let Some(ref rc_name) = op.reg_class {
                let rc = RegisterClass::parse(rc_name)?;

                // For memory ops, force the first IntRegs source to r27 (base register)
                if is_mem_op && rc == RegisterClass::IntRegs && !base_reg_assigned {
                    base_reg_assigned = true;
                    let name = "r27".to_string();
                    asm = asm.replace(&format!("${}", op.name), &name);
                    src_regs.push(name);
                    continue;
                }

                let safe = rc.safe_indices();
                if safe.is_empty() {
                    return None;
                }
                let idx = safe[rng.gen_range(0..safe.len())];
                let name = rc.register_name(idx);
                asm = asm.replace(&format!("${}", op.name), &name);
                src_regs.push(name);
            }
        }

        Some(ConcreteInsn {
            def: insn,
            dest_regs,
            src_regs,
            immediates,
            asm_text: asm,
        })
    }

    /// Pick a memory offset that stays within the memory region AND fits
    /// in the operand's base encoding (no constant extender needed).
    ///
    /// The tricky part: instruction-level `op_extent_bits` / `op_extent_align`
    /// describe the *extendable* operand, which for store-immediate instructions
    /// (`S4_storeiri_io` etc.) is the value to store, NOT the memory offset.
    /// So we derive encoding limits from the per-operand `imm_type` string.
    ///
    /// The `imm_type` naming convention is `[su]<stored_bits>_<align_shift>Imm`:
    /// - Small stored_bits (≤16): this IS the base encoding (e.g., `u6_2Imm`).
    ///   Max value = (2^bits - 1) * 2^align   [unsigned]
    /// - Large stored_bits (>16): this is the *extended* range with constant
    ///   extender (e.g., `s30_2Imm`). Fall back to `op_extent_bits` for the
    ///   base encoding width.
    ///
    /// Returns a non-negative, aligned offset in
    /// `[0, min(encoding_max, region_size - access_size)]`.
    fn pick_mem_offset(
        &self,
        rng: &mut StdRng,
        insn: &InstructionDef,
        imm_type: Option<&str>,
        access_size: usize,
    ) -> i64 {
        // Parse per-operand encoding info from imm_type
        let parsed = imm_type.and_then(parse_imm_bits_and_align);

        // Always use per-operand alignment (from imm_type), falling back to
        // access-size-derived alignment if unavailable.
        let align = parsed.map(|(_, a)| a).unwrap_or_else(|| match access_size {
            8 => 3,
            4 => 2,
            2 => 1,
            _ => 0,
        });
        let align_step = 1i64 << align;

        // Determine encoding max based on whether imm_type is a base or extended type
        let encoding_max = match parsed {
            Some((imm_bits, _)) if imm_bits <= 16 => {
                // Small imm_type: this IS the base encoding range.
                // `bits` is the stored field width; value = stored * align_step.
                let unsigned = imm_type.is_some_and(is_imm_unsigned);
                let raw_max = if unsigned {
                    (1i64 << imm_bits) - 1
                } else {
                    (1i64 << (imm_bits - 1)) - 1
                };
                raw_max * align_step
            }
            _ => {
                // Large or missing imm_type: use op_extent_bits as total value
                // bit-width (includes implicit alignment zeros).
                let bits = insn.op_extent_bits.unwrap_or(8);
                let raw_max = (1i64 << (bits - 1)) - 1;
                // Round down to alignment since raw_max is a value, not field
                (raw_max / align_step) * align_step
            }
        };

        // Maximum safe offset: stay within the memory region
        let region_max = (self.config.mem_region_size.saturating_sub(access_size)) as i64;

        // Take the tighter of the two constraints
        let max_offset = encoding_max.min(region_max);
        let aligned_max = (max_offset / align_step) * align_step;

        if aligned_max <= 0 {
            return 0;
        }

        let steps = aligned_max / align_step;
        let step = rng.gen_range(0..=steps);
        step * align_step
    }

    /// Pick an immediate value appropriate for the instruction.
    fn pick_immediate(
        &self,
        rng: &mut StdRng,
        insn: &InstructionDef,
        imm_type: Option<&str>,
    ) -> i64 {
        // Try to determine range from instruction attributes
        let bits = insn.op_extent_bits.unwrap_or_else(|| {
            // Try to infer from imm_type name like "s8_0Imm" -> 8 bits
            if let Some(it) = imm_type {
                parse_imm_bits(it).unwrap_or(8)
            } else {
                8
            }
        });

        let signed = insn.is_extent_signed || imm_type.is_some_and(|it| it.starts_with('s'));
        let align = insn.op_extent_align.unwrap_or(0);

        let max_val: i64;
        let min_val: i64;

        if signed {
            max_val = (1i64 << (bits - 1)) - 1;
            min_val = -(1i64 << (bits - 1));
        } else {
            max_val = (1i64 << bits) - 1;
            min_val = 0;
        }

        // Generate a value in range, respecting alignment
        let align_step = 1i64 << align;
        let aligned_min = (min_val + align_step - 1) / align_step * align_step;
        let aligned_max = max_val / align_step * align_step;

        if aligned_min > aligned_max {
            return 0;
        }

        let steps = (aligned_max - aligned_min) / align_step;
        if steps == 0 {
            return aligned_min;
        }

        let step = rng.gen_range(0..=steps);
        aligned_min + step * align_step
    }
}

/// Determine the memory access size in bytes from an instruction's assembly syntax.
///
/// Matches the `mem*` mnemonic in the syntax string:
/// - `memd` → 8 bytes (doubleword)
/// - `memw` → 4 bytes (word)
/// - `memh` / `memuh` → 2 bytes (halfword)
/// - `memb` / `memub` → 1 byte
///
/// Returns 4 (word) as a safe default if the pattern is unrecognized.
fn mem_access_size(asm_syntax: &str) -> usize {
    let lower = asm_syntax.to_lowercase();
    if lower.contains("memd") {
        8
    } else if lower.contains("memw") {
        4
    } else if lower.contains("memh(") || lower.contains("memuh(") {
        2
    } else if lower.contains("memb(") || lower.contains("memub(") {
        1
    } else {
        4 // safe default
    }
}

/// Expand a register name into all names that it aliases.
/// For example, "r5:4" aliases with "r4", "r5", and "r5:4" itself.
/// For single registers, "r5" aliases with "r5" and "r5:4" (or "r5:4"/"r6:5" depending on convention).
fn expand_reg_aliases(name: &str) -> Vec<String> {
    let mut aliases = vec![name.to_string()];
    // DoubleRegs format: "rN+1:N" -> also aliases rN and rN+1
    if let Some(colon_pos) = name.find(':') {
        let hi_part = &name[..colon_pos]; // e.g. "r5"
        let lo_part = &name[colon_pos + 1..]; // e.g. "4" but with prefix from hi
        let prefix = &hi_part[..1]; // "r", "v", "c", "g"
        let lo_name = format!("{}{}", prefix, lo_part);
        aliases.push(hi_part.to_string());
        aliases.push(lo_name);
    } else {
        // Single register, compute its double-reg pair
        // e.g. "r5" is part of "r5:4" (if odd index) or "r1:0" pattern
        let prefix = &name[..1];
        if let Ok(idx) = name[1..].parse::<usize>() {
            if prefix == "r" || prefix == "v" {
                let pair_lo = idx & !1; // even number
                let pair_hi = pair_lo + 1;
                let double_name = format!("{}{}:{}", prefix, pair_hi, pair_lo);
                aliases.push(double_name);
            }
        }
    }
    aliases
}

/// Parse the bit width from an immediate type name like "s8_0Imm" -> 8, "u10_0Imm" -> 10.
fn parse_imm_bits(imm_type: &str) -> Option<u32> {
    let stripped = imm_type
        .strip_prefix('s')
        .or_else(|| imm_type.strip_prefix('u'))?;
    let num_part = stripped.split('_').next()?;
    num_part.parse().ok()
}

/// Parse both bit width and alignment shift from an immediate type name.
///
/// Format: `[su]<bits>_<align>Imm`
/// Examples: `u6_2Imm` → (6, 2), `s11_1Imm` → (11, 1), `s32_0Imm` → (32, 0)
fn parse_imm_bits_and_align(imm_type: &str) -> Option<(u32, u32)> {
    let stripped = imm_type
        .strip_prefix('s')
        .or_else(|| imm_type.strip_prefix('u'))?;
    let mut parts = stripped.split('_');
    let bits: u32 = parts.next()?.parse().ok()?;
    let align_part = parts.next()?;
    // Strip the "Imm" suffix to get just the alignment digit(s)
    let align_str = align_part.strip_suffix("Imm").unwrap_or(align_part);
    let align: u32 = align_str.parse().ok()?;
    Some((bits, align))
}

/// Check whether an immediate type name represents an unsigned value.
fn is_imm_unsigned(imm_type: &str) -> bool {
    imm_type.starts_with('u')
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_dump::types::{InstructionDef, InstructionSetDump, Operand};

    fn make_simple_db() -> InstructionDb {
        let mut insns = Vec::new();

        // A simple ALU instruction
        let mut add = InstructionDef::new("A2_add".to_string());
        add.itype = "TypeALU32_3op".to_string();
        add.asm_syntax = "$Rd32 = add($Rs32,$Rt32)".to_string();
        add.has_new_value = true;
        add.outs = vec![Operand {
            name: "Rd32".to_string(),
            reg_class: Some("IntRegs".to_string()),
            is_immediate: false,
            imm_type: None,
        }];
        add.ins = vec![
            Operand {
                name: "Rs32".to_string(),
                reg_class: Some("IntRegs".to_string()),
                is_immediate: false,
                imm_type: None,
            },
            Operand {
                name: "Rt32".to_string(),
                reg_class: Some("IntRegs".to_string()),
                is_immediate: false,
                imm_type: None,
            },
        ];
        insns.push(add);

        // Another ALU instruction
        let mut sub = InstructionDef::new("A2_sub".to_string());
        sub.itype = "TypeALU32_3op".to_string();
        sub.asm_syntax = "$Rd32 = sub($Rt32,$Rs32)".to_string();
        sub.has_new_value = true;
        sub.outs = vec![Operand {
            name: "Rd32".to_string(),
            reg_class: Some("IntRegs".to_string()),
            is_immediate: false,
            imm_type: None,
        }];
        sub.ins = vec![
            Operand {
                name: "Rt32".to_string(),
                reg_class: Some("IntRegs".to_string()),
                is_immediate: false,
                imm_type: None,
            },
            Operand {
                name: "Rs32".to_string(),
                reg_class: Some("IntRegs".to_string()),
                is_immediate: false,
                imm_type: None,
            },
        ];
        insns.push(sub);

        // An S-type instruction
        let mut asr = InstructionDef::new("S2_asr_r_r".to_string());
        asr.itype = "TypeS_3op".to_string();
        asr.asm_syntax = "$Rd32 = asr($Rs32,$Rt32)".to_string();
        asr.has_new_value = true;
        asr.outs = vec![Operand {
            name: "Rd32".to_string(),
            reg_class: Some("IntRegs".to_string()),
            is_immediate: false,
            imm_type: None,
        }];
        asr.ins = vec![
            Operand {
                name: "Rs32".to_string(),
                reg_class: Some("IntRegs".to_string()),
                is_immediate: false,
                imm_type: None,
            },
            Operand {
                name: "Rt32".to_string(),
                reg_class: Some("IntRegs".to_string()),
                is_immediate: false,
                imm_type: None,
            },
        ];
        insns.push(asr);

        let dump = InstructionSetDump {
            version: "1.0".to_string(),
            total_parsed: 3,
            instructions: insns,
        };
        InstructionDb::from_dump(dump)
    }

    #[test]
    fn test_synthesize_single_packet() {
        let db = make_simple_db();
        let config = SynthConfig::default();
        let synth = PacketSynthesizer::new(&db, config);
        let mut rng = StdRng::seed_from_u64(42);
        let packet = synth.synthesize_packet(&mut rng);
        assert!(!packet.insns.is_empty());
        assert!(packet.insns.len() <= 4);
    }

    #[test]
    fn test_synthesize_deterministic() {
        let db = make_simple_db();
        let config1 = SynthConfig::default();
        let config2 = SynthConfig::default();
        let synth1 = PacketSynthesizer::new(&db, config1);
        let synth2 = PacketSynthesizer::new(&db, config2);
        let mut rng1 = StdRng::seed_from_u64(42);
        let mut rng2 = StdRng::seed_from_u64(42);
        let p1 = synth1.synthesize_packet(&mut rng1);
        let p2 = synth2.synthesize_packet(&mut rng2);
        assert_eq!(p1.insns.len(), p2.insns.len());
        for (a, b) in p1.insns.iter().zip(p2.insns.iter()) {
            assert_eq!(a.asm_text, b.asm_text);
        }
    }

    #[test]
    fn test_asm_output_no_dollar_signs() {
        let db = make_simple_db();
        let config = SynthConfig::default();
        let synth = PacketSynthesizer::new(&db, config);
        let mut rng = StdRng::seed_from_u64(123);
        let packet = synth.synthesize_packet(&mut rng);
        for insn in &packet.insns {
            assert!(
                !insn.asm_text.contains('$'),
                "Assembly still has $: {}",
                insn.asm_text
            );
        }
    }

    fn make_mem_db() -> InstructionDb {
        let mut insns = Vec::new();

        // A word load: L2_loadri_io
        let mut load = InstructionDef::new("L2_loadri_io".to_string());
        load.itype = "TypeLD".to_string();
        load.asm_syntax = "$Rd32 = memw($Rs32+#$Ii)".to_string();
        load.may_load = true;
        load.has_new_value = true;
        load.is_extendable = true;
        load.is_extent_signed = true;
        load.op_extent_bits = Some(13);
        load.op_extent_align = Some(2);
        load.outs = vec![Operand {
            name: "Rd32".to_string(),
            reg_class: Some("IntRegs".to_string()),
            is_immediate: false,
            imm_type: None,
        }];
        load.ins = vec![
            Operand {
                name: "Rs32".to_string(),
                reg_class: Some("IntRegs".to_string()),
                is_immediate: false,
                imm_type: None,
            },
            Operand {
                name: "Ii".to_string(),
                reg_class: None,
                is_immediate: true,
                imm_type: Some("s30_2Imm".to_string()),
            },
        ];
        insns.push(load);

        // A word store: S2_storeri_io
        let mut store = InstructionDef::new("S2_storeri_io".to_string());
        store.itype = "TypeST".to_string();
        store.asm_syntax = "memw($Rs32+#$Ii) = $Rt32".to_string();
        store.may_store = true;
        store.is_extendable = true;
        store.is_extent_signed = true;
        store.op_extent_bits = Some(13);
        store.op_extent_align = Some(2);
        store.outs = vec![];
        store.ins = vec![
            Operand {
                name: "Rs32".to_string(),
                reg_class: Some("IntRegs".to_string()),
                is_immediate: false,
                imm_type: None,
            },
            Operand {
                name: "Ii".to_string(),
                reg_class: None,
                is_immediate: true,
                imm_type: Some("s30_2Imm".to_string()),
            },
            Operand {
                name: "Rt32".to_string(),
                reg_class: Some("IntRegs".to_string()),
                is_immediate: false,
                imm_type: None,
            },
        ];
        insns.push(store);

        let dump = InstructionSetDump {
            version: "1.0".to_string(),
            total_parsed: 2,
            instructions: insns,
        };
        InstructionDb::from_dump(dump)
    }

    #[test]
    fn test_mem_ops_base_register_forced_to_r27() {
        let db = make_mem_db();
        let config = SynthConfig {
            max_packet_size: 1,
            allow_mem_ops: true,
            mem_region_size: 65536,
            ..SynthConfig::default()
        };
        let synth = PacketSynthesizer::new(&db, config);
        let mut rng = StdRng::seed_from_u64(42);

        for _ in 0..50 {
            let packet = synth.synthesize_packet(&mut rng);
            for insn in &packet.insns {
                // Base register must always be r27
                assert!(
                    insn.asm_text.contains("r27"),
                    "Memory op should use r27 as base: {}",
                    insn.asm_text
                );
                // No unresolved placeholders
                assert!(
                    !insn.asm_text.contains('$'),
                    "Unresolved placeholder: {}",
                    insn.asm_text
                );
            }
        }
    }

    #[test]
    fn test_mem_ops_r27_not_used_as_dest() {
        let db = make_mem_db();
        let config = SynthConfig {
            max_packet_size: 1,
            allow_mem_ops: true,
            mem_region_size: 65536,
            ..SynthConfig::default()
        };
        let synth = PacketSynthesizer::new(&db, config);
        let mut rng = StdRng::seed_from_u64(42);

        for _ in 0..100 {
            let packet = synth.synthesize_packet(&mut rng);
            for insn in &packet.insns {
                // r27 must not appear as a destination
                for dest in &insn.dest_regs {
                    assert_ne!(
                        dest, "r27",
                        "r27 should not be used as destination: {}",
                        insn.asm_text
                    );
                }
            }
        }
    }

    #[test]
    fn test_mem_ops_offset_within_bounds() {
        let db = make_mem_db();
        let region_size = 1024; // small region for tighter testing
        let config = SynthConfig {
            max_packet_size: 1,
            allow_mem_ops: true,
            mem_region_size: region_size,
            ..SynthConfig::default()
        };
        let synth = PacketSynthesizer::new(&db, config);
        let mut rng = StdRng::seed_from_u64(42);

        for _ in 0..100 {
            let packet = synth.synthesize_packet(&mut rng);
            for insn in &packet.insns {
                for &imm in &insn.immediates {
                    assert!(
                        imm >= 0,
                        "Memory offset should be non-negative: {} in {}",
                        imm,
                        insn.asm_text
                    );
                    let access = mem_access_size(&insn.def.asm_syntax) as i64;
                    assert!(
                        imm + access <= region_size as i64,
                        "Memory offset {} + access {} exceeds region {} in {}",
                        imm,
                        access,
                        region_size,
                        insn.asm_text
                    );
                }
            }
        }
    }

    #[test]
    fn test_mem_access_size_detection() {
        assert_eq!(mem_access_size("$Rd32 = memw($Rs32+#$Ii)"), 4);
        assert_eq!(mem_access_size("$Rdd32 = memd($Rs32+#$Ii)"), 8);
        assert_eq!(mem_access_size("$Rd32 = memh($Rs32+#$Ii)"), 2);
        assert_eq!(mem_access_size("$Rd32 = memuh($Rs32+#$Ii)"), 2);
        assert_eq!(mem_access_size("$Rd32 = memb($Rs32+#$Ii)"), 1);
        assert_eq!(mem_access_size("$Rd32 = memub($Rs32+#$Ii)"), 1);
        // Store variants
        assert_eq!(mem_access_size("memw($Rs32+#$Ii) = $Rt32"), 4);
        assert_eq!(mem_access_size("memb($Rs32+#$Ii) = $Rt32"), 1);
    }

    #[test]
    fn test_parse_imm_bits() {
        assert_eq!(parse_imm_bits("s8_0Imm"), Some(8));
        assert_eq!(parse_imm_bits("u10_0Imm"), Some(10));
        assert_eq!(parse_imm_bits("s32_0Imm"), Some(32));
        assert_eq!(parse_imm_bits("b30_2Imm"), None); // starts with 'b'
    }

    #[test]
    fn test_parse_imm_bits_and_align() {
        assert_eq!(parse_imm_bits_and_align("u6_2Imm"), Some((6, 2)));
        assert_eq!(parse_imm_bits_and_align("s11_1Imm"), Some((11, 1)));
        assert_eq!(parse_imm_bits_and_align("s30_2Imm"), Some((30, 2)));
        assert_eq!(parse_imm_bits_and_align("s32_0Imm"), Some((32, 0)));
        assert_eq!(parse_imm_bits_and_align("u6_0Imm"), Some((6, 0)));
        assert_eq!(parse_imm_bits_and_align("b30_2Imm"), None); // bad prefix
        assert!(is_imm_unsigned("u6_2Imm"));
        assert!(!is_imm_unsigned("s11_1Imm"));
    }

    #[test]
    fn test_packet_asm_format() {
        let db = make_simple_db();
        let config = SynthConfig {
            max_packet_size: 2,
            ..SynthConfig::default()
        };
        let synth = PacketSynthesizer::new(&db, config);
        let mut rng = StdRng::seed_from_u64(42);
        let packet = synth.synthesize_packet(&mut rng);
        let asm = packet.to_asm();
        assert!(asm.starts_with("    {"));
        assert!(asm.ends_with('}'));
    }
}
