use anyhow::Result;
use hex_dump::types::InstructionDef;
use hex_instset::database::InstructionDb;
use hex_instset::filter::Filter;
use hex_instset::register::RegisterClass;
use rand::prelude::*;
use rand::rngs::StdRng;

use hex_instset::database::is_early_predicate_producer;

use crate::constraint::{
    implicit_int_reg_outputs, implicit_pred_output, validate_concrete_packet,
    validate_dot_new_pairing, validate_nv_store_pairing, validate_packet,
};
use crate::slot::assign_slots_with_restrictions;

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
    /// Reserve r26 (exclude from register allocation) for pageable base.
    pub reserve_r26: bool,
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
            reserve_r26: false,
        }
    }
}

/// Synthesizes legal VLIW packets from an instruction database.
pub struct PacketSynthesizer<'a> {
    _db: &'a InstructionDb,
    config: SynthConfig,
    /// Prefiltered candidate instructions.
    candidates: Vec<&'a InstructionDef>,
    /// Early predicate producer candidates (for .new group synthesis).
    pred_producer_candidates: Vec<&'a InstructionDef>,
    /// Instructions with `is_predicated_new == true` (for .new group synthesis).
    dot_new_candidates: Vec<&'a InstructionDef>,
    /// New-value producer candidates (for NV-store group synthesis).
    nv_producer_candidates: Vec<&'a InstructionDef>,
    /// New-value store candidates (for NV-store group synthesis).
    nv_store_candidates: Vec<&'a InstructionDef>,
}

/// Return safe register indices, excluding r26 from IntRegs when reserved.
fn safe_indices_for(rc: RegisterClass, reserve_r26: bool) -> Vec<usize> {
    let mut indices = rc.safe_indices();
    if reserve_r26 && rc == RegisterClass::IntRegs {
        indices.retain(|&i| i != 26);
    }
    indices
}

impl<'a> PacketSynthesizer<'a> {
    pub fn new(db: &'a InstructionDb, config: SynthConfig) -> Self {
        let candidates = Self::build_candidate_list(db, &config);

        // Build separate pools for .new group synthesis
        let (pred_producer_candidates, dot_new_candidates) = if config.allow_predicated_new {
            Self::build_dot_new_pools(db, &config)
        } else {
            (Vec::new(), Vec::new())
        };

        // Build separate pools for NV-store group synthesis
        let (nv_producer_candidates, nv_store_candidates) = if config.allow_new_value {
            Self::build_nv_store_pools(db, &config)
        } else {
            (Vec::new(), Vec::new())
        };

        Self {
            _db: db,
            config,
            candidates,
            pred_producer_candidates,
            dot_new_candidates,
            nv_producer_candidates,
            nv_store_candidates,
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

                // Instructions requiring producer pairing are excluded from
                // the regular pool and only generated through specialized paths:
                //   - is_predicated_new → try_synthesize_dot_new_group
                //   - is_nv_store → try_synthesize_nv_store_group
                //   - is_new_value (NV compare-and-jump) → no path yet
                if insn.is_predicated_new || insn.is_nv_store || insn.is_new_value {
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
                if config.skip_terms.iter().any(|term| {
                    insn.asm_syntax
                        .to_lowercase()
                        .contains(&term.to_lowercase())
                        || insn.name.to_lowercase().contains(&term.to_lowercase())
                }) {
                    return false;
                }

                // Reject instructions with unparseable register classes or
                // register classes with no safe indices — these would fail at
                // operand assignment time, wasting synthesis attempts.
                let has_unusable_operand = insn.ins.iter().chain(insn.outs.iter()).any(|op| {
                    if let Some(ref rc_name) = op.reg_class {
                        match RegisterClass::parse(rc_name) {
                            None => true,
                            Some(rc) => safe_indices_for(rc, config.reserve_r26).is_empty(),
                        }
                    } else {
                        false
                    }
                });
                !has_unusable_operand
            })
            .collect();

        // Sort for deterministic iteration
        result.sort_by(|a, b| a.name.cmp(&b.name));
        result
    }

    /// Build separate candidate pools for .new group synthesis.
    fn build_dot_new_pools(
        db: &'a InstructionDb,
        config: &SynthConfig,
    ) -> (Vec<&'a InstructionDef>, Vec<&'a InstructionDef>) {
        let base_filter = |insn: &&InstructionDef| -> bool {
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
            if config.skip_terms.iter().any(|term| {
                insn.asm_syntax
                    .to_lowercase()
                    .contains(&term.to_lowercase())
                    || insn.name.to_lowercase().contains(&term.to_lowercase())
            }) {
                return false;
            }
            // Skip solo, call, return
            if insn.is_solo || insn.is_call || insn.is_return {
                return false;
            }
            true
        };

        let mut producers: Vec<&InstructionDef> = db
            .all()
            .iter()
            .filter(|insn| base_filter(insn) && is_early_predicate_producer(insn))
            .collect();
        producers.sort_by(|a, b| a.name.cmp(&b.name));

        let mut consumers: Vec<&InstructionDef> = db
            .all()
            .iter()
            .filter(|insn| {
                base_filter(insn)
                    && insn.is_predicated_new
                    // Exclude NV stores/consumers — they need register pairing
                    // that the dot_new path doesn't provide.
                    && !insn.is_nv_store
                    && !insn.is_new_value
            })
            .collect();
        consumers.sort_by(|a, b| a.name.cmp(&b.name));

        (producers, consumers)
    }

    /// Build separate candidate pools for new-value store group synthesis.
    ///
    /// Producers are instructions with `has_new_value` that write an `IntRegs`
    /// destination. Consumers are `is_nv_store` instructions that read an
    /// `IntRegs` source via the `Nt8` operand.
    fn build_nv_store_pools(
        db: &'a InstructionDb,
        config: &SynthConfig,
    ) -> (Vec<&'a InstructionDef>, Vec<&'a InstructionDef>) {
        let base_filter = |insn: &&InstructionDef| -> bool {
            if insn.requires.iter().any(|r| {
                config
                    .blocked_features
                    .iter()
                    .any(|b| r.contains(b.as_str()))
            }) {
                return false;
            }
            // Apply skip terms (but ignore "mem" and ".new" for this pool,
            // since NV stores inherently contain both)
            let skip_filtered: Vec<&String> = config
                .skip_terms
                .iter()
                .filter(|t| {
                    let tl = t.to_lowercase();
                    tl != "mem" && tl != ".new"
                })
                .collect();
            if skip_filtered.iter().any(|term| {
                insn.asm_syntax
                    .to_lowercase()
                    .contains(&term.to_lowercase())
                    || insn.name.to_lowercase().contains(&term.to_lowercase())
            }) {
                return false;
            }
            if insn.is_solo || insn.is_call || insn.is_return {
                return false;
            }
            true
        };

        // Producers: has_new_value, writes IntRegs, not an NV store itself,
        // not .new predicated (would need a separate predicate producer).
        let mut producers: Vec<&InstructionDef> = db
            .all()
            .iter()
            .filter(|insn| {
                base_filter(insn)
                    && insn.has_new_value
                    && !insn.is_nv_store
                    && !insn.is_predicated_new
                    && insn
                        .outs
                        .iter()
                        .any(|op| op.reg_class.as_deref() == Some("IntRegs"))
            })
            .collect();
        producers.sort_by(|a, b| a.name.cmp(&b.name));

        // Consumers: is_nv_store
        let mut consumers: Vec<&InstructionDef> = db
            .all()
            .iter()
            .filter(|insn| base_filter(insn) && insn.is_nv_store && !insn.is_predicated_new)
            .collect();
        consumers.sort_by(|a, b| a.name.cmp(&b.name));

        (producers, consumers)
    }

    /// Access the candidate list (for external filtering).
    pub fn candidates(&self) -> &[&'a InstructionDef] {
        &self.candidates
    }

    /// Synthesize a single legal packet.
    pub fn synthesize_packet(&self, rng: &mut StdRng) -> Result<Packet<'a>> {
        let max_attempts = 100;
        for _ in 0..max_attempts {
            // When .new is enabled and pools are available, try dot-new group ~40% of the time
            let try_dot_new = self.config.allow_predicated_new
                && !self.pred_producer_candidates.is_empty()
                && !self.dot_new_candidates.is_empty()
                && rng.gen::<f64>() < 0.4;

            if try_dot_new {
                if let Some(packet) = self.try_synthesize_dot_new_group(rng) {
                    return Ok(packet);
                }
            }

            // When NV stores are enabled and pools are available, try ~30% of the time
            let try_nv_store = self.config.allow_new_value
                && !self.nv_producer_candidates.is_empty()
                && !self.nv_store_candidates.is_empty()
                && rng.gen::<f64>() < 0.3;

            if try_nv_store {
                if let Some(packet) = self.try_synthesize_nv_store_group(rng) {
                    return Ok(packet);
                }
            }

            if let Some(packet) = self.try_synthesize(rng) {
                return Ok(packet);
            }
        }
        anyhow::bail!(
            "Failed to synthesize a legal packet after {} attempts",
            max_attempts
        )
    }

    /// Attempt to synthesize a packet with a .new predicate producer-consumer group.
    fn try_synthesize_dot_new_group(&self, rng: &mut StdRng) -> Option<Packet<'a>> {
        // 1. Pick a random early predicate producer
        let producer =
            self.pred_producer_candidates[rng.gen_range(0..self.pred_producer_candidates.len())];

        // 2. Pick a shared predicate register (p0-p3)
        let pred_idx = rng.gen_range(0..4u32);
        let shared_pred = format!("p{}", pred_idx);

        // 3. Pick 1-3 .new consumers that are slot-compatible with the producer
        let num_consumers = rng.gen_range(1..=3usize).min(3);
        let mut selected: Vec<&InstructionDef> = vec![producer];

        for _ in 0..num_consumers {
            // Find compatible .new consumers
            let compatible: Vec<&&InstructionDef> = self
                .dot_new_candidates
                .iter()
                .filter(|&&consumer| {
                    let mut test = selected.clone();
                    test.push(consumer);
                    if test.len() > 4 {
                        return false;
                    }
                    if assign_slots_with_restrictions(&test).is_none() {
                        return false;
                    }
                    // Check mem count
                    let mem_count = test.iter().filter(|i| i.may_load || i.may_store).count();
                    if mem_count > 2 {
                        return false;
                    }
                    // Check store limit
                    let store_count = test.iter().filter(|i| i.may_store).count();
                    if store_count > 1 {
                        return false;
                    }
                    // Check branch limit (using is_branch)
                    let branch_count = test.iter().filter(|i| i.is_branch).count();
                    if branch_count > 2 {
                        return false;
                    }
                    // Check cofMax1
                    let cof_count = test.iter().filter(|i| i.cof_max1).count();
                    if cof_count > 1 {
                        return false;
                    }
                    true
                })
                .collect();

            if compatible.is_empty() {
                break;
            }
            selected.push(compatible[rng.gen_range(0..compatible.len())]);
        }

        // Must have at least one consumer
        if selected.len() < 2 {
            return None;
        }

        // 4. Optionally fill remaining slots with normal candidates
        let remaining = 4 - selected.len();
        if remaining > 0 && rng.gen::<f64>() < 0.5 {
            let fill_count = rng.gen_range(0..=remaining);
            for _ in 0..fill_count {
                let compatible: Vec<&&InstructionDef> = self
                    .candidates
                    .iter()
                    .filter(|&&insn| {
                        // Skip .new instructions in fill slots
                        if insn.is_predicated_new {
                            return false;
                        }
                        // Skip late pred producers — their PredRegs output
                        // would be forced to shared_pred, creating a late +
                        // early conflict that invalidates .new usage.
                        if insn.is_predicate_late {
                            return false;
                        }
                        let mut test = selected.clone();
                        test.push(insn);
                        if test.len() > 4 {
                            return false;
                        }
                        assign_slots_with_restrictions(&test).is_some()
                    })
                    .collect();
                if compatible.is_empty() {
                    break;
                }
                selected.push(compatible[rng.gen_range(0..compatible.len())]);
            }
        }

        // Validate the packet
        if !validate_packet(&selected).valid {
            return None;
        }

        // 5. Assign concrete operands with forced predicate register
        let concrete = self.assign_operands_with_forced_pred(rng, &selected, &shared_pred)?;

        // Validate concrete packet
        if !validate_concrete_packet(&concrete).valid {
            return None;
        }

        // Validate .new pairing
        if !validate_dot_new_pairing(&concrete).valid {
            return None;
        }

        Some(Packet { insns: concrete })
    }

    /// Attempt to synthesize a packet with a new-value store producer-consumer group.
    ///
    /// Picks a `has_new_value` producer and an `is_nv_store` consumer, then
    /// forces the producer's `IntRegs` destination to match the consumer's
    /// `Nt8` source operand so the `.new` value forwarding is legal.
    fn try_synthesize_nv_store_group(&self, rng: &mut StdRng) -> Option<Packet<'a>> {
        // 1. Pick a random NV-store consumer
        let consumer = self.nv_store_candidates[rng.gen_range(0..self.nv_store_candidates.len())];

        // 2. Pick a random producer
        let producer =
            self.nv_producer_candidates[rng.gen_range(0..self.nv_producer_candidates.len())];

        // 3. Check slot compatibility between producer and consumer
        let mut selected: Vec<&InstructionDef> = vec![producer, consumer];
        assign_slots_with_restrictions(&selected)?;

        // Reject DoubleRegs producers for NV stores
        if producer
            .outs
            .iter()
            .any(|op| op.reg_class.as_deref() == Some("DoubleRegs"))
        {
            return None;
        }

        // Reject AbsoluteSet (2) or PostInc (6) address mode producers
        if producer.addr_mode == 2 || producer.addr_mode == 6 {
            return None;
        }

        // Reject if consumer is unconditional but producer is predicated
        if !consumer.is_predicated && producer.is_predicated {
            return None;
        }

        // 4. Optionally fill remaining slots with normal candidates
        let remaining = 4 - selected.len();
        if remaining > 0 && rng.gen::<f64>() < 0.5 {
            let fill_count = rng.gen_range(0..=remaining);
            for _ in 0..fill_count {
                let compatible: Vec<&&InstructionDef> = self
                    .candidates
                    .iter()
                    .filter(|&&insn| {
                        if insn.is_nv_store {
                            return false;
                        }
                        let mut test = selected.clone();
                        test.push(insn);
                        if test.len() > 4 {
                            return false;
                        }
                        if assign_slots_with_restrictions(&test).is_none() {
                            return false;
                        }
                        let mem_count = test.iter().filter(|i| i.may_load || i.may_store).count();
                        if mem_count > 2 {
                            return false;
                        }
                        let store_count = test.iter().filter(|i| i.may_store).count();
                        store_count <= 1
                    })
                    .collect();
                if compatible.is_empty() {
                    break;
                }
                selected.push(compatible[rng.gen_range(0..compatible.len())]);
            }
        }

        // Validate the packet
        if !validate_packet(&selected).valid {
            return None;
        }

        // 5. Assign concrete operands with a forced shared register for the
        //    producer dest -> consumer Nt8 source.
        let rc = RegisterClass::IntRegs;
        let safe = safe_indices_for(rc, self.config.reserve_r26);
        let shared_idx = safe[rng.gen_range(0..safe.len())];
        let shared_reg = rc.register_name(shared_idx);

        let concrete =
            self.assign_operands_with_forced_nv(rng, &selected, producer, consumer, &shared_reg)?;

        // Validate concrete packet
        if !validate_concrete_packet(&concrete).valid {
            return None;
        }

        // Validate NV-store pairing rules
        if !validate_nv_store_pairing(&concrete).valid {
            return None;
        }

        Some(Packet { insns: concrete })
    }

    /// Assign operands for a packet containing an NV-store group.
    ///
    /// Forces the producer's first `IntRegs` output to `shared_reg` and the
    /// consumer's `Nt8` input to `shared_reg`.
    fn assign_operands_with_forced_nv(
        &self,
        rng: &mut StdRng,
        insns: &[&'a InstructionDef],
        producer: &InstructionDef,
        consumer: &InstructionDef,
        shared_reg: &str,
    ) -> Option<Vec<ConcreteInsn<'a>>> {
        let mut used_dest_regs: Vec<String> = Vec::new();

        // Pre-populate with implicit register writes.
        for &insn in insns {
            // Implicit predicate writes (e.g. J2_ploop* → P3).
            if let Some(implicit_pred) = implicit_pred_output(insn) {
                used_dest_regs.extend(expand_reg_aliases(implicit_pred));
            }
            // Implicit integer register writes (e.g. deallocframe → R29).
            for &implicit_reg in implicit_int_reg_outputs(insn) {
                used_dest_regs.extend(expand_reg_aliases(implicit_reg));
            }
        }

        let mut concrete = Vec::new();

        for &insn in insns {
            let forced_dest = if std::ptr::eq(insn, producer) {
                Some(shared_reg)
            } else {
                None
            };
            let forced_nt = if std::ptr::eq(insn, consumer) {
                Some(shared_reg)
            } else {
                None
            };
            let ci =
                self.assign_single_operands_nv(rng, insn, &used_dest_regs, forced_dest, forced_nt)?;
            for dest in &ci.dest_regs {
                used_dest_regs.extend(expand_reg_aliases(dest));
            }
            concrete.push(ci);
        }

        Some(concrete)
    }

    /// Assign operands for a single instruction with optional forced NV register.
    fn assign_single_operands_nv(
        &self,
        rng: &mut StdRng,
        insn: &'a InstructionDef,
        used_dests: &[String],
        forced_dest_intreg: Option<&str>,
        forced_nt_src: Option<&str>,
    ) -> Option<ConcreteInsn<'a>> {
        let mut dest_regs = Vec::new();
        let mut src_regs = Vec::new();
        let mut immediates = Vec::new();
        let mut asm = insn.asm_syntax.clone();

        // Assign output operands
        for op in &insn.outs {
            if op.is_immediate {
                continue;
            }
            if let Some(ref rc_name) = op.reg_class {
                let rc = RegisterClass::parse(rc_name)?;

                // Force the first IntRegs output to the shared register
                if let Some(reg) = forced_dest_intreg {
                    if rc_name == "IntRegs" {
                        asm = asm.replace(&format!("${}", op.name), reg);
                        dest_regs.push(reg.to_string());
                        continue;
                    }
                }

                let safe = safe_indices_for(rc, self.config.reserve_r26);
                if safe.is_empty() {
                    return None;
                }

                let mut found = false;
                for _ in 0..30 {
                    let idx = safe[rng.gen_range(0..safe.len())];
                    let name = rc.register_name(idx);
                    let aliases = expand_reg_aliases(&name);
                    let conflicts = aliases.iter().any(|a| used_dests.contains(a));
                    if !conflicts {
                        asm = asm.replace(&format!("${}", op.name), &name);
                        dest_regs.push(name);
                        found = true;
                        break;
                    }
                }
                if !found {
                    return None;
                }
            }
        }

        // Assign input operands
        for op in &insn.ins {
            if op.is_immediate {
                let imm = self.pick_immediate(rng, insn, op.imm_type.as_deref());
                let pattern = format!("${}", op.name);
                if asm.contains(&format!("#{}", pattern)) {
                    asm = asm.replace(&pattern, &format!("{}", imm));
                } else {
                    asm = asm.replace(&pattern, &format!("#{}", imm));
                }
                immediates.push(imm);
            } else if let Some(ref rc_name) = op.reg_class {
                let rc = RegisterClass::parse(rc_name)?;

                // Force Nt8 to the shared register
                if let Some(reg) = forced_nt_src {
                    if op.name.starts_with("Nt") {
                        asm = asm.replace(&format!("${}", op.name), reg);
                        src_regs.push(reg.to_string());
                        continue;
                    }
                }

                let safe = safe_indices_for(rc, self.config.reserve_r26);
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

                // Check slot compatibility (with restriction rules).
                // Each instruction that needs a constant extender consumes
                // an extra slot, reducing the usable packet capacity.
                let mut test_insns: Vec<&InstructionDef> = selected.to_vec();
                test_insns.push(insn);
                let extender_count = test_insns
                    .iter()
                    .filter(|i| i.needs_constant_extender())
                    .count();
                if test_insns.len() + extender_count > 4 {
                    return false;
                }
                if assign_slots_with_restrictions(&test_insns).is_none() {
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

                // Check store limit (at most 1)
                let store_count = test_insns.iter().filter(|i| i.may_store).count();
                if store_count > 1 {
                    return false;
                }

                // Check branch limit (using is_branch)
                let branch_count = test_insns.iter().filter(|i| i.is_branch).count();
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
        self.assign_operands_with_forced_pred(rng, insns, "")
    }

    /// Assign operands, optionally forcing a specific predicate register for
    /// all PredRegs operands (when `forced_pred` is non-empty).
    fn assign_operands_with_forced_pred(
        &self,
        rng: &mut StdRng,
        insns: &[&'a InstructionDef],
        forced_pred: &str,
    ) -> Option<Vec<ConcreteInsn<'a>>> {
        let mut used_dest_regs: Vec<String> = Vec::new();

        // Pre-populate with implicit register writes.
        for insn in insns {
            // Implicit predicate writes (e.g. J2_ploop* → P3).
            if let Some(implicit_pred) = implicit_pred_output(insn) {
                used_dest_regs.extend(expand_reg_aliases(implicit_pred));
            }
            // Implicit integer register writes (e.g. deallocframe → R29).
            for &implicit_reg in implicit_int_reg_outputs(insn) {
                used_dest_regs.extend(expand_reg_aliases(implicit_reg));
            }
        }

        let mut concrete = Vec::new();

        for insn in insns {
            let ci = self.assign_single_operands_inner(rng, insn, &used_dest_regs, forced_pred)?;
            // Track destination registers, expanding double regs into component singles
            for dest in &ci.dest_regs {
                used_dest_regs.extend(expand_reg_aliases(dest));
            }
            concrete.push(ci);
        }

        Some(concrete)
    }

    /// Assign operands for a single instruction with optional forced predicate.
    fn assign_single_operands_inner(
        &self,
        rng: &mut StdRng,
        insn: &'a InstructionDef,
        used_dests: &[String],
        forced_pred: &str,
    ) -> Option<ConcreteInsn<'a>> {
        let mut dest_regs = Vec::new();
        let mut src_regs = Vec::new();
        let mut immediates = Vec::new();
        let mut asm = insn.asm_syntax.clone();

        // Assign output operands
        for op in &insn.outs {
            if op.is_immediate {
                continue;
            }
            if let Some(ref rc_name) = op.reg_class {
                let rc = RegisterClass::parse(rc_name)?;

                // Force predicate register if applicable
                if !forced_pred.is_empty() && rc_name == "PredRegs" {
                    asm = asm.replace(&format!("${}", op.name), forced_pred);
                    dest_regs.push(forced_pred.to_string());
                    continue;
                }

                let safe = safe_indices_for(rc, self.config.reserve_r26);
                if safe.is_empty() {
                    return None;
                }

                // Pick a register not already used as a destination
                let mut found = false;
                for _ in 0..30 {
                    let idx = safe[rng.gen_range(0..safe.len())];
                    let name = rc.register_name(idx);
                    let aliases = expand_reg_aliases(&name);
                    let conflicts = aliases.iter().any(|a| used_dests.contains(a));
                    if !conflicts {
                        asm = asm.replace(&format!("${}", op.name), &name);
                        dest_regs.push(name);
                        found = true;
                        break;
                    }
                }
                if !found {
                    // No conflict-free register available — synthesis failed
                    return None;
                }
            }
        }

        // Assign input operands
        for op in &insn.ins {
            if op.is_immediate {
                let imm = self.pick_immediate(rng, insn, op.imm_type.as_deref());
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

                // Force predicate register if applicable
                if !forced_pred.is_empty() && rc_name == "PredRegs" {
                    asm = asm.replace(&format!("${}", op.name), forced_pred);
                    src_regs.push(forced_pred.to_string());
                    continue;
                }

                let safe = safe_indices_for(rc, self.config.reserve_r26);
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

    /// Pick an immediate value appropriate for the instruction.
    ///
    /// For memory operations, the value is derived from the operand's `imm_type`
    /// (e.g. `s4_1Imm`) to stay within the encoding-valid range, then clamped so
    /// that base + offset stays within the 64KB `mem_region`.
    fn pick_immediate(
        &self,
        rng: &mut StdRng,
        insn: &InstructionDef,
        imm_type: Option<&str>,
    ) -> i64 {
        // Maximum absolute immediate value for memory ops, to keep addresses
        // within the 64KB mem_region.
        const MEM_CLAMP: i64 = 4096;

        // Determine (signed, bits, align) for the immediate.
        //
        // The imm_type spec (e.g. "u32_0Imm") gives the full EXTENDED range,
        // which may require a constant extender that consumes a packet slot.
        // Prefer the instruction's native encoding range (op_extent_bits) to
        // avoid generating extended immediates.
        let parsed = imm_type.and_then(parse_imm_spec);

        let (signed, bits, align) = {
            if let Some((s, b, a)) = parsed {
                if let Some(nb) = insn.op_extent_bits {
                    if nb < b {
                        // The imm_type range exceeds the native encoding.
                        // Values beyond op_extent_bits need a constant extender
                        // that consumes a VLIW packet slot.  Stay within the
                        // native range to avoid extenders.
                        let na = insn.op_extent_align.unwrap_or(0);
                        (s, nb, na)
                    } else {
                        (s, b, a)
                    }
                } else {
                    (s, b, a)
                }
            } else if let Some(nb) = insn.op_extent_bits {
                let s = insn.is_extent_signed;
                let na = insn.op_extent_align.unwrap_or(0);
                (s, nb, na)
            } else {
                let s = imm_type.is_some_and(|it| it.starts_with('s'));
                (s, 8, 0)
            }
        };

        // Compute the encoding-valid range
        let (mut min_val, mut max_val) = if signed {
            (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1)
        } else {
            (0i64, (1i64 << bits) - 1)
        };

        // For memory ops, clamp so base + offset stays within mem_region
        if insn.may_load || insn.may_store {
            max_val = max_val.min(MEM_CLAMP);
            if signed {
                min_val = min_val.max(-MEM_CLAMP);
            }
        }

        // Scale by alignment
        let align_step = 1i64 << align;
        // For signed negative values, round toward zero (ceiling division)
        let aligned_min = if min_val >= 0 {
            (min_val + align_step - 1) / align_step * align_step
        } else {
            // e.g. min_val=-8, align_step=2 -> -8 (already aligned)
            // e.g. min_val=-7, align_step=2 -> -6
            -((-min_val) / align_step * align_step)
        };
        let aligned_max = if max_val >= 0 {
            max_val / align_step * align_step
        } else {
            -((-max_val + align_step - 1) / align_step * align_step)
        };

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

/// Parse an immediate type spec like `"s4_1Imm"` into `(signed, bits, align)`.
///
/// Examples:
/// - `"s4_1Imm"` -> `(true, 4, 1)` — signed, 4-bit, 1-bit alignment
/// - `"u10_0Imm"` -> `(false, 10, 0)` — unsigned, 10-bit, no alignment
fn parse_imm_spec(imm_type: &str) -> Option<(bool, u32, u32)> {
    // Branch immediates (b13_2Imm, b30_2Imm) are signed PC-relative offsets
    let signed = imm_type.starts_with('s') || imm_type.starts_with('b');
    let rest = imm_type.strip_prefix(|c: char| c == 's' || c == 'u' || c == 'b')?;
    let (bits_str, remainder) = rest.split_once('_')?;
    let bits: u32 = bits_str.parse().ok()?;
    let align_str = remainder.strip_suffix("Imm")?;
    let align: u32 = align_str.parse().ok()?;
    Some((signed, bits, align))
}

/// Parse the bit width from an immediate type name like "s8_0Imm" -> 8, "u10_0Imm" -> 10.
#[cfg(test)]
fn parse_imm_bits(imm_type: &str) -> Option<u32> {
    parse_imm_spec(imm_type).map(|(_, bits, _)| bits)
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
        let packet = synth.synthesize_packet(&mut rng).unwrap();
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
        let p1 = synth1.synthesize_packet(&mut rng1).unwrap();
        let p2 = synth2.synthesize_packet(&mut rng2).unwrap();
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
        let packet = synth.synthesize_packet(&mut rng).unwrap();
        for insn in &packet.insns {
            assert!(
                !insn.asm_text.contains('$'),
                "Assembly still has $: {}",
                insn.asm_text
            );
        }
    }

    #[test]
    fn test_parse_imm_bits() {
        assert_eq!(parse_imm_bits("s8_0Imm"), Some(8));
        assert_eq!(parse_imm_bits("u10_0Imm"), Some(10));
        assert_eq!(parse_imm_bits("s32_0Imm"), Some(32));
        assert_eq!(parse_imm_bits("b30_2Imm"), Some(30)); // branch immediate
        assert_eq!(parse_imm_bits("b13_2Imm"), Some(13)); // branch immediate
    }

    #[test]
    fn test_parse_imm_spec() {
        assert_eq!(parse_imm_spec("s4_1Imm"), Some((true, 4, 1)));
        assert_eq!(parse_imm_spec("u10_0Imm"), Some((false, 10, 0)));
        assert_eq!(parse_imm_spec("s8_0Imm"), Some((true, 8, 0)));
        assert_eq!(parse_imm_spec("u2_0Imm"), Some((false, 2, 0)));
        assert_eq!(parse_imm_spec("s4_2Imm"), Some((true, 4, 2)));
        assert_eq!(parse_imm_spec("b30_2Imm"), Some((true, 30, 2))); // branch immediate (signed)
        assert_eq!(parse_imm_spec("b13_2Imm"), Some((true, 13, 2))); // branch immediate (signed)
        assert_eq!(parse_imm_spec("garbage"), None);
    }

    #[test]
    fn test_pick_immediate_memory_s4_1() {
        // s4_1Imm: signed 4-bit with 1-bit align -> range [-16, 14] step 2
        let db = make_simple_db();
        let config = SynthConfig::default();
        let synth = PacketSynthesizer::new(&db, config);
        let mut rng = StdRng::seed_from_u64(42);

        let mut insn = InstructionDef::new("test_memop".to_string());
        insn.may_store = true;

        for _ in 0..100 {
            let val = synth.pick_immediate(&mut rng, &insn, Some("s4_1Imm"));
            assert!(val >= -16 && val <= 14, "s4_1Imm out of range: {}", val);
            assert_eq!(val % 2, 0, "s4_1Imm not aligned: {}", val);
        }
    }

    #[test]
    fn test_pick_immediate_memory_u2_0() {
        // u2_0Imm: unsigned 2-bit -> range [0, 3]
        let db = make_simple_db();
        let config = SynthConfig::default();
        let synth = PacketSynthesizer::new(&db, config);
        let mut rng = StdRng::seed_from_u64(99);

        let mut insn = InstructionDef::new("test_memop".to_string());
        insn.may_load = true;

        for _ in 0..100 {
            let val = synth.pick_immediate(&mut rng, &insn, Some("u2_0Imm"));
            assert!(val >= 0 && val <= 3, "u2_0Imm out of range: {}", val);
        }
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
        let packet = synth.synthesize_packet(&mut rng).unwrap();
        let asm = packet.to_asm();
        assert!(asm.starts_with("    {"));
        assert!(asm.ends_with('}'));
    }
}
