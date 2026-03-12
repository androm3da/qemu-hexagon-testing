use anyhow::Result;
use hex_instset::database::InstructionDb;
use hex_packet::synthesizer::{PacketSynthesizer, SynthConfig};
use rand::rngs::StdRng;
use rand::Rng;
use rand::SeedableRng;

use crate::mutation::{gen_gpr_mutation_block, gen_hvx_mutation_block};
use crate::recipe::{ExecutionMode, Recipe};
use crate::setup::{find_pre_dependencies, gen_setup_packets};

/// Offset within mem_region where the scalar register dump is stored.
/// Located near the end of the 64KB region to avoid clobbering test data.
pub const REG_DUMP_OFFSET: u32 = 0xFF00;

/// Number of 4-byte scalar registers dumped:
/// r0-r27 (28) + p3:0 (1) + sa0, lc0, sa1, lc1, m0, m1, usr (7) = 36.
pub const REG_DUMP_COUNT: u32 = 36;

/// Offset within hvx_mem_region where the HVX register dump is stored.
pub const HVX_DUMP_OFFSET: u32 = 0x3_F000;

/// Number of HVX vector registers dumped (v0-v31).
pub const HVX_VREG_COUNT: u32 = 32;

/// Number of HVX predicate registers dumped (q0-q3).
pub const HVX_QREG_COUNT: u32 = 4;

/// Magic sentinel written before each register dump so the host parser can
/// locate real dumps in stdout even if the emulator prints crash/error text.
pub const DUMP_SENTINEL: &[u8; 4] = b"HXRG";

/// C helper source for binary stdout writes.
///
/// The hexagon-sim semihosting `write()` treats stdout as a text stream and
/// stops at null bytes. This helper opens `/dev/fd/1` as a binary stream
/// using `fopen("wb")` to bypass that limitation.
///
/// `write_binary_stdout` writes a 4-byte sentinel ("HXRG") followed by the
/// payload so the host parser can locate the dump in the output stream.
pub const DUMP_HELPER_C: &str = r#"/* Auto-generated — do not edit */
#include <stdio.h>
void write_binary_stdout(const void *buf, unsigned len) {
    FILE *f = fopen("/dev/fd/1", "wb");
    if (f) {
        fwrite("HXRG", 1, 4, f);
        fwrite(buf, 1, len, f);
        fclose(f);
    }
}
"#;

/// C helper source for pageable memory support.
///
/// Provides `pageable_init()`, `pageable_get_base()`, and `pageable_invalidate()`
/// for TLB miss + packet replay testing. Uses `add_translation()` from
/// `hexagon_standalone.h` to populate the software page table, and provides
/// a local TLB invalidation routine since the runtime's `tlb_invalidate` is
/// typically `static inline` and not linkable.
pub const PAGEABLE_HELPER_C: &str = r#"/* Auto-generated — do not edit */
#include <hexagon_standalone.h>

extern char pageable_backing[];
#define PAGEABLE_MB_OFFSET 32

/* tlb_invalidate is static inline in the runtime — provide our own */
static void pageable_tlb_invalidate(unsigned int va) {
    unsigned int vpn = va >> 12;
    unsigned int ssr;
    asm volatile("%0 = ssr" : "=r"(ssr));
    unsigned int asid = (ssr >> 8) & 0x7f;
    unsigned int probe_val = ((asid << 20) | vpn) & 0x7ffffff;
    unsigned int result;
    asm volatile("%0 = tlbp(%1)" : "=r"(result) : "r"(probe_val));
    if (result & 0x80000000u) return;  /* TLB_NOT_FOUND */
    unsigned long long entry;
    asm volatile("%0 = tlbr(%1)" : "=r"(entry) : "r"(result));
    asm volatile("isync");
    entry &= ~(1ULL << 63);  /* Clear Valid bit */
    asm volatile("tlblock; tlbw(%0,%1); isync; tlbunlock"
                 : : "r"(entry), "r"(result));
}

static unsigned int pageable_va_base;

void pageable_init(void) {
    unsigned int pa = (unsigned int)pageable_backing;
    unsigned int pa_page = pa & 0xFFF00000u;
    unsigned int va_page = pa_page + (PAGEABLE_MB_OFFSET << 20);
    add_translation((void *)va_page, (void *)pa_page, 7);
    pageable_va_base = pa + (PAGEABLE_MB_OFFSET << 20);
}

unsigned int pageable_get_base(void) { return pageable_va_base; }

void pageable_invalidate(void) {
    unsigned int pa = (unsigned int)pageable_backing;
    unsigned int va_page = (pa & 0xFFF00000u) + (PAGEABLE_MB_OFFSET << 20);
    pageable_tlb_invalidate(va_page);
}
"#;

/// Metadata for a single generated instruction.
#[derive(Debug, Clone)]
pub struct GeneratedInstruction {
    /// Instruction name, e.g. "A2_add".
    pub name: String,
    /// Instruction type, e.g. "TypeALU32_3op".
    pub itype: String,
    /// Fully resolved assembly text, e.g. "r0 = add(r1,r2)".
    pub asm_text: String,
}

/// Result of program generation including assembly and instruction metadata.
pub struct GeneratedProgram {
    /// The complete assembly program text.
    pub assembly: String,
    /// Metadata for each instruction word generated in the steps function.
    pub instructions: Vec<GeneratedInstruction>,
    /// Names of all synthesizable candidate instructions.
    pub candidate_names: Vec<String>,
}

/// Generates a complete assembly test program from a recipe.
pub struct ProgramGenerator<'a> {
    db: &'a InstructionDb,
}

impl<'a> ProgramGenerator<'a> {
    pub fn new(db: &'a InstructionDb) -> Self {
        Self { db }
    }

    /// Generate a complete assembly program with instruction metadata.
    pub fn generate(&self, recipe: &Recipe) -> Result<GeneratedProgram> {
        let mut rng = StdRng::seed_from_u64(recipe.seed);
        let mut lines = Vec::new();
        let mut instructions = Vec::new();
        let mut candidate_names = Vec::new();

        // Header
        self.emit_header(&mut lines, recipe);

        // Main function
        self.emit_main(&mut lines);

        // Init function
        self.emit_init(&mut lines, recipe, &mut rng);

        // Body function (loop wrapper)
        self.emit_body(&mut lines, recipe);

        // Steps function (synthesized packets)
        self.emit_steps(
            &mut lines,
            recipe,
            &mut rng,
            &mut instructions,
            &mut candidate_names,
        )?;

        // Data section
        self.emit_data(&mut lines, recipe);

        Ok(GeneratedProgram {
            assembly: lines.join("\n"),
            instructions,
            candidate_names,
        })
    }

    fn emit_header(&self, lines: &mut Vec<String>, _recipe: &Recipe) {
        lines.push("// Auto-generated Hexagon test program".to_string());
        lines.push("// Do not edit manually".to_string());
        lines.push(String::new());
    }

    fn emit_main(&self, lines: &mut Vec<String>) {
        lines.push(".text".to_string());
        lines.push(".globl main".to_string());
        lines.push(".type main, @function".to_string());
        lines.push("main:".to_string());
        lines.push("    { allocframe(#64) }".to_string());
        // Save callee-saved registers (r16-r27)
        lines.push("    { memd(r29+#-8) = r17:16 }".to_string());
        lines.push("    { memd(r29+#-16) = r19:18 }".to_string());
        lines.push("    { memd(r29+#-24) = r21:20 }".to_string());
        lines.push("    { memd(r29+#-32) = r23:22 }".to_string());
        lines.push("    { memd(r29+#-40) = r25:24 }".to_string());
        lines.push("    { memd(r29+#-48) = r27:26 }".to_string());
        lines.push("    { call init }".to_string());
        lines.push("    { call body }".to_string());
        // Restore callee-saved registers
        lines.push("    { r17:16 = memd(r29+#-8) }".to_string());
        lines.push("    { r19:18 = memd(r29+#-16) }".to_string());
        lines.push("    { r21:20 = memd(r29+#-24) }".to_string());
        lines.push("    { r23:22 = memd(r29+#-32) }".to_string());
        lines.push("    { r25:24 = memd(r29+#-40) }".to_string());
        lines.push("    { r27:26 = memd(r29+#-48) }".to_string());
        lines.push("    { dealloc_return }".to_string());
        lines.push(String::new());
    }

    fn emit_init(&self, lines: &mut Vec<String>, recipe: &Recipe, _rng: &mut StdRng) {
        lines.push(".globl init".to_string());
        lines.push(".type init, @function".to_string());
        lines.push("init:".to_string());

        if recipe.synth.allow_pageable {
            // C calls below clobber r31 — save the return address.
            lines.push("    { allocframe(#0) }".to_string());
        }

        // Initialize GPRs r0-r27 with seed-derived values
        // Use `r<N> = ##<32-bit-value>` (constant extender) for full initialization
        let mut val = recipe.seed;
        for reg in 0..28u32 {
            let imm = (val & 0xFFFF_FFFF) as u32;
            lines.push(format!("    {{ r{} = ##0x{:08x} }}", reg, imm));
            // Advance LFSR
            val = val
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
        }

        // Initialize predicate registers with seed-dependent values
        // (r0-r7 are already initialized with seed-derived values above)
        lines.push("    { p0 = cmp.eq(r0,r1) }".to_string());
        lines.push("    { p1 = cmp.gt(r2,r3) }".to_string());
        lines.push("    { p2 = cmp.eq(r4,r5) }".to_string());
        lines.push("    { p3 = cmp.gt(r6,r7) }".to_string());

        // Initialize memory base in a known way using mem_region
        lines.push("    { r27 = ##mem_region }".to_string());

        if recipe.synth.allow_pageable {
            // Set up pageable memory: PTE mapping + get VA base into r26.
            // These C calls may clobber r0-r5, r28, so we do them before
            // setting r28 (loop counter) below.
            lines.push("    { call pageable_init }".to_string());
            lines.push("    { call pageable_get_base }".to_string());
            lines.push("    { r26 = r0 }".to_string());
        }

        // Initialize loop counter in r28 (after C calls that may clobber it)
        lines.push(format!("    {{ r28 = #{} }}", recipe.num_iterations));

        if recipe.hvx {
            // Initialize a few HVX registers with splat values
            lines.push("    { v0 = vsplat(r0) }".to_string());
            lines.push("    { v1 = vsplat(r1) }".to_string());
            lines.push("    { v2 = vsplat(r2) }".to_string());
            lines.push("    { v3 = vsplat(r3) }".to_string());
        }

        if recipe.synth.allow_pageable {
            lines.push("    { dealloc_return }".to_string());
        } else {
            lines.push("    { jumpr r31 }".to_string());
        }
        lines.push(String::new());
    }

    fn emit_body(&self, lines: &mut Vec<String>, recipe: &Recipe) {
        lines.push(".globl body".to_string());
        lines.push(".type body, @function".to_string());
        lines.push("body:".to_string());
        // Save r31:r30 — `call steps` (and `call pageable_invalidate`)
        // clobber r31, so a plain `jumpr r31` at the end would not return
        // to main. allocframe/dealloc_return save and restore the frame.
        let frame_size = if recipe.synth.allow_pageable { 8 } else { 0 };
        lines.push(format!("    {{ allocframe(#{}) }}", frame_size));
        lines.push(".Lbody_loop:".to_string());
        if recipe.synth.allow_pageable {
            // Save the loop counter — the C call below clobbers r28
            // (caller-saved). We restore it after the call.
            lines.push("    { memw(r29+#0) = r28 }".to_string());
            // Invalidate the TLB entry so the next memory access in steps
            // causes a TLB miss → fault handler fills TLB → packet replay.
            lines.push("    { call pageable_invalidate }".to_string());
            lines.push("    { r28 = memw(r29+#0) }".to_string());
        }
        lines.push("    { call steps }".to_string());
        if recipe.execution_mode == ExecutionMode::Gdb {
            lines.push(".globl check_point".to_string());
            lines.push("check_point:".to_string());
        }
        lines.push("    { r28 = add(r28,#-1) }".to_string());
        lines.push("    { p0 = cmp.gt(r28,#0) }".to_string());
        lines.push("    { if (p0) jump .Lbody_loop }".to_string());
        lines.push("    { dealloc_return }".to_string());
        lines.push(String::new());
    }

    fn emit_steps(
        &self,
        lines: &mut Vec<String>,
        recipe: &Recipe,
        rng: &mut StdRng,
        instructions: &mut Vec<GeneratedInstruction>,
        candidate_names: &mut Vec<String>,
    ) -> Result<()> {
        lines.push(".globl steps".to_string());
        lines.push(".type steps, @function".to_string());
        lines.push("steps:".to_string());

        // Build skip terms, removing terms for enabled features
        let mut skip_terms = recipe.filters.skip_terms.clone();
        if recipe.synth.allow_memory_ops || recipe.synth.allow_pageable {
            skip_terms.retain(|t| t.to_lowercase() != "mem");
        }
        if recipe.synth.allow_jumps {
            skip_terms.retain(|t| t.to_lowercase() != "jump");
        }
        if recipe.synth.allow_predicated_new {
            skip_terms.retain(|t| t.to_lowercase() != ".new");
        }
        if recipe.synth.allow_pageable {
            // Protect the pageable base register from the synthesizer
            if !skip_terms.iter().any(|t| t == "r26") {
                skip_terms.push("r26".to_string());
            }
        }

        // Build exclude filters, removing branch type exclusion when jumps allowed
        let mut exclude_filters = recipe.filters.exclude.clone();
        if recipe.synth.allow_jumps {
            use hex_instset::filter::Filter;
            exclude_filters.retain(|f| {
                // Remove the Or filter that excludes TypeJ/TypeCJ/TypeNCJ/TypeCR
                !matches!(f, Filter::Or(inner) if inner.iter().any(|ff| {
                    matches!(ff, Filter::ByType(t) if t == "TypeJ" || t == "TypeCJ" || t == "TypeNCJ")
                }))
            });
        }

        let config = SynthConfig {
            max_packet_size: recipe.synth.max_packet_size,
            allow_predicated: true,
            allow_predicated_new: recipe.synth.allow_predicated_new,
            allow_new_value: recipe.synth.allow_new_value,
            max_cvi_per_packet: if recipe.hvx {
                recipe.synth.max_cvi_per_packet
            } else {
                0
            },
            skip_terms,
            exclude_filters,
            include_filter: recipe.filters.include.clone(),
            blocked_features: recipe.filters.blocked_features.clone(),
            reserve_r26: recipe.synth.allow_pageable,
        };

        let synth = PacketSynthesizer::new(self.db, config);

        if synth.candidates().is_empty() {
            anyhow::bail!("No candidate instructions after filtering");
        }

        // Capture candidate names for coverage analysis
        *candidate_names = synth.candidates().iter().map(|c| c.name.clone()).collect();

        // Build step context for jump label targets
        let use_labels = recipe.synth.allow_jumps;

        for i in 0..recipe.num_packets {
            // Emit step label for jump targets
            if use_labels {
                lines.push(format!("step_{}:", i));
            }

            // Add mutation block every few packets
            if i > 0 && i % 3 == 0 {
                let mutations = gen_gpr_mutation_block(rng, 28);
                for m in mutations {
                    lines.push(m);
                }
                if recipe.hvx {
                    let hvx_mutations = gen_hvx_mutation_block(rng, 32);
                    for m in hvx_mutations {
                        lines.push(m);
                    }
                }
            }

            match synth.synthesize_packet(rng) {
                Ok(packet) => {
                    // Emit pre-packet setup for memory ops
                    if recipe.synth.allow_memory_ops || recipe.synth.allow_pageable {
                        let deps = find_pre_dependencies(&packet);
                        if !deps.is_empty() {
                            let base_reg = if recipe.synth.allow_pageable {
                                "r26"
                            } else {
                                "r27"
                            };
                            let setup = gen_setup_packets(&deps, rng, 65536, base_reg);
                            for s in setup {
                                lines.push(s);
                            }
                        }
                    }

                    // Post-process assembly for jump targets
                    let asm = if use_labels {
                        self.replace_jump_targets(&packet, rng, i, recipe)
                    } else {
                        packet.to_asm()
                    };
                    lines.push(asm);

                    // Record instruction metadata
                    for ci in &packet.insns {
                        instructions.push(GeneratedInstruction {
                            name: ci.def.name.clone(),
                            itype: ci.def.itype.clone(),
                            asm_text: ci.asm_text.clone(),
                        });
                    }
                }
                Err(e) => {
                    // Emit a NOP packet on synthesis failure
                    lines.push(format!("    // synth failed: {}", e));
                    lines.push("    { nop }".to_string());
                }
            }
        }

        lines.push(".globl test_end".to_string());
        lines.push("test_end:".to_string());

        match recipe.execution_mode {
            ExecutionMode::Gdb => {
                // In GDB mode, steps just returns — the GDB client reads
                // registers at the check_point breakpoint in body.
                lines.push("    { jumpr r31 }".to_string());
            }
            ExecutionMode::Direct => {
                // === Register state dump to stdout ===
                // Save all comparable registers to a buffer at mem_region + REG_DUMP_OFFSET,
                // then call write() to emit the buffer to stdout as raw binary.
                //
                // Strategy: first save r0 and r1 using r27 (mem_region base) with
                // constant-extended offsets, then load the dump buffer address into r1
                // and use small offsets from r1 for remaining registers.

                // Save r0 and r1 before we clobber them
                lines.push(format!("    {{ memw(r27+##{REG_DUMP_OFFSET}) = r0 }}"));
                lines.push(format!(
                    "    {{ memw(r27+##{}) = r1 }}",
                    REG_DUMP_OFFSET + 4
                ));

                // Load dump buffer base into r1
                lines.push(format!("    {{ r1 = add(r27,##{REG_DUMP_OFFSET}) }}"));

                // Save r2-r26 using r1 as base with small word-aligned offsets
                for reg in 2..27u32 {
                    let off = reg * 4;
                    lines.push(format!("    {{ memw(r1+#{off}) = r{reg} }}"));
                }
                // Save r27 (known value = mem_region base)
                lines.push(format!("    {{ memw(r1+#{}) = r27 }}", 27 * 4));

                // Save predicate register pack (p3:0) — clobbers r0
                lines.push("    { r0 = p3:0 }".to_string());
                lines.push(format!("    {{ memw(r1+#{}) = r0 }}", 28 * 4));

                // Save loop/modifier/usr registers — clobbers r0
                let ctrl_regs = ["sa0", "lc0", "sa1", "lc1", "m0", "m1", "usr"];
                for (i, creg) in ctrl_regs.iter().enumerate() {
                    let off = (29 + i as u32) * 4;
                    lines.push(format!("    {{ r0 = {creg} }}"));
                    lines.push(format!("    {{ memw(r1+#{off}) = r0 }}"));
                }

                // Call write_binary_stdout(buf=r1, len=REG_DUMP_BYTES)
                // r1 already points to the dump buffer; move it to r0 for first arg.
                let dump_bytes = REG_DUMP_COUNT * 4;
                lines.push("    { r0 = r1 }".to_string());
                lines.push(format!("    {{ r1 = #{dump_bytes} }}"));
                lines.push("    { call write_binary_stdout }".to_string());

                if recipe.hvx {
                    self.emit_hvx_dump(lines);
                }

                // Explicitly exit the program so both hexagon-sim and QEMU terminate
                // cleanly via semihosting. Without this, QEMU system mode hangs
                // because the CRT return chain may not trigger a proper shutdown.
                lines.push("    { r0 = #0 }".to_string());
                lines.push("    { call exit }".to_string());
            }
        }
        lines.push(String::new());

        Ok(())
    }

    /// Emit HVX register dump to stdout.
    ///
    /// Stores v0-v31 (128 bytes each) and q0-q3 (128 bytes each expanded via vmux)
    /// into hvx_mem_region starting at HVX_DUMP_OFFSET, then calls write().
    /// vmem uses vector-length-scaled immediates: `vmem(Rx+#N)` stores at `Rx + N*128`.
    fn emit_hvx_dump(&self, lines: &mut Vec<String>) {
        // Load dump base address: hvx_mem_region + HVX_DUMP_OFFSET
        lines.push(format!(
            "    {{ r1 = ##(hvx_mem_region + {HVX_DUMP_OFFSET}) }}"
        ));

        // Store v0-v31 using vector-length-scaled offsets (#0..#31)
        for i in 0..HVX_VREG_COUNT {
            lines.push(format!("    {{ vmem(r1+#{i}) = v{i} }}"));
        }

        // Store q0-q3 expanded to 128-byte vectors via vmux
        lines.push("    { r0 = #-1 }".to_string());
        lines.push("    { v32 = vsplat(r0) }".to_string());
        lines.push("    { r0 = #0 }".to_string());
        lines.push("    { v33 = vsplat(r0) }".to_string());
        for i in 0..HVX_QREG_COUNT {
            let off = HVX_VREG_COUNT + i;
            lines.push(format!("    {{ v34 = vmux(q{i},v32,v33) }}"));
            lines.push(format!("    {{ vmem(r1+#{off}) = v34 }}"));
        }

        // Call write_binary_stdout(buf=dump_base, len=total_dump_bytes)
        let hvx_dump_bytes = (HVX_VREG_COUNT + HVX_QREG_COUNT) * 128;
        lines.push(format!(
            "    {{ r0 = ##(hvx_mem_region + {HVX_DUMP_OFFSET}) }}"
        ));
        lines.push(format!("    {{ r1 = ##{hvx_dump_bytes} }}"));
        lines.push("    { call write_binary_stdout }".to_string());
    }

    /// Replace branch immediates in a packet with step labels.
    fn replace_jump_targets(
        &self,
        packet: &hex_packet::synthesizer::Packet,
        rng: &mut StdRng,
        current_step: usize,
        recipe: &Recipe,
    ) -> String {
        let mut asm = packet.to_asm();

        // Check if any instruction in the packet is a branch type
        let has_branch = packet
            .insns
            .iter()
            .any(|ci| matches!(ci.def.itype.as_str(), "TypeJ" | "TypeCJ" | "TypeNCJ"));

        if !has_branch {
            return asm;
        }

        // Build available labels
        let mut targets: Vec<String> = Vec::new();
        if recipe.synth.forward_only_jumps {
            for j in (current_step + 1)..recipe.num_packets {
                targets.push(format!("step_{}", j));
            }
        } else {
            for j in 0..recipe.num_packets {
                if j != current_step {
                    targets.push(format!("step_{}", j));
                }
            }
        }
        targets.push("test_end".to_string());

        // Replace numeric branch immediates with a label
        // Branch immediates look like `#<number>` or `##<number>` in jump context
        // We replace the entire `#<number>` with a label
        for ci in &packet.insns {
            if !matches!(ci.def.itype.as_str(), "TypeJ" | "TypeCJ" | "TypeNCJ") {
                continue;
            }
            // Find the branch immediate in this instruction's operands
            for op in &ci.def.ins {
                if let Some(ref it) = op.imm_type {
                    if it.starts_with('b') {
                        // This is a branch immediate -- find and replace in asm
                        let label = &targets[rng.gen_range(0..targets.len())];
                        // The immediate was rendered as a number in the asm text.
                        // Find the pattern: the resolved immediate value
                        for &imm in &ci.immediates {
                            let patterns = [format!("##{}", imm), format!("#{}", imm)];
                            for pat in &patterns {
                                if asm.contains(pat) {
                                    asm = asm.replacen(pat, label, 1);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        asm
    }

    fn emit_data(&self, lines: &mut Vec<String>, recipe: &Recipe) {
        lines.push(".data".to_string());
        lines.push(".balign 4096".to_string());
        lines.push(".globl mem_region".to_string());
        lines.push("mem_region:".to_string());
        lines.push("    .space 65536".to_string());

        if recipe.hvx {
            lines.push(".balign 4096".to_string());
            lines.push(".globl hvx_mem_region".to_string());
            lines.push("hvx_mem_region:".to_string());
            lines.push("    .space 262144".to_string());
        }

        if recipe.synth.allow_pageable {
            lines.push(".balign 4096".to_string());
            lines.push(".globl pageable_backing".to_string());
            lines.push("pageable_backing:".to_string());
            lines.push("    .space 65536".to_string());
        }

        lines.push(String::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_dump::types::{InstructionDef, InstructionSetDump, Operand};

    fn make_simple_db() -> InstructionDb {
        let mut insns = Vec::new();

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
    fn test_generate_program() {
        let db = make_simple_db();
        let gen = ProgramGenerator::new(&db);
        let recipe = Recipe {
            num_packets: 5,
            num_iterations: 2,
            seed: 42,
            ..Recipe::default()
        };
        let program = gen.generate(&recipe).unwrap();

        // Verify key labels are present
        assert!(program.assembly.contains("main:"));
        assert!(program.assembly.contains("init:"));
        assert!(program.assembly.contains("body:"));
        assert!(program.assembly.contains("steps:"));
        assert!(program.assembly.contains("test_end:"));
        assert!(program.assembly.contains("mem_region:"));

        // Verify instruction metadata was collected
        assert!(!program.instructions.is_empty());
        assert!(!program.candidate_names.is_empty());
    }

    #[test]
    fn test_generate_deterministic() {
        let db = make_simple_db();
        let gen = ProgramGenerator::new(&db);
        let recipe = Recipe {
            num_packets: 5,
            seed: 42,
            ..Recipe::default()
        };
        let p1 = gen.generate(&recipe).unwrap();
        let p2 = gen.generate(&recipe).unwrap();
        assert_eq!(p1.assembly, p2.assembly);
    }

    #[test]
    fn test_generate_has_gpr_init() {
        let db = make_simple_db();
        let gen = ProgramGenerator::new(&db);
        let recipe = Recipe::default();
        let program = gen.generate(&recipe).unwrap();
        // Should initialize registers r0 through r27
        assert!(program.assembly.contains("r0 = #"));
        assert!(program.assembly.contains("r27 = ##mem_region"));
    }
}
