use anyhow::{Context, Result};
use askama::Template;
use hex_instset::database::InstructionDb;
use hex_instset::filter::{AttributeFilter, Filter};
use hex_packet::synthesizer::{PacketSynthesizer, SynthConfig};
use rand::prelude::*;
use rand::rngs::StdRng;
use rand::SeedableRng;

use crate::recipe::Recipe;

/// Values that exercise boundary conditions: sign extension edges, carry
/// propagation, saturation limits, power-of-two boundaries, and mixed
/// byte patterns.
const INTERESTING_VALUES: &[u32] = &[
    0x0000_0000, // zero
    0x0000_0001, // one
    0x0000_0010, // small power of 2
    0x0000_0020, // 32
    0x0000_0040, // 64
    0x0000_007f, // max signed i8
    0x0000_0080, // min unsigned > i8, i8 sign bit
    0x0000_00ff, // max u8
    0x0000_0100, // u8 overflow
    0x0000_0200, // 512
    0x0000_0400, // 1024
    0x0000_1000, // 4096
    0x0000_7fff, // max signed i16
    0x0000_8000, // min unsigned > i16, i16 sign bit
    0x0000_ffff, // max u16
    0x0001_0000, // u16 overflow
    0x05ff_ff05, // mixed byte pattern
    0x7fff_ffff, // max signed i32
    0x8000_0000, // min signed i32 / i32 sign bit
    0xfa00_00fa, // mixed byte pattern
    0xffff_7fff, // ~0x8000
    0xffff_8000, // sign-extend of -32768
    0xffff_ff7f, // ~0x80
    0xffff_ff80, // sign-extend of -128
    0xffff_ffff, // -1 / all ones
];

/// A register initialization entry for the template.
struct InitReg {
    num: usize,
    hex: String,
}

/// The askama template for the generated assembly program.
///
/// The template file lives at `hex-prog/templates/test_program.S` and looks
/// like readable Hexagon assembly with Jinja2-style tags for the dynamic parts.
#[derive(Template)]
#[template(path = "test_program.S", escape = "none")]
struct TestProgramTemplate<'a> {
    init_regs: Vec<InitReg>,
    num_iterations: usize,
    hvx: bool,
    packets: &'a [String],
}

/// Generate initial values for `count` registers using a mix of interesting
/// edge-case values and random values. Roughly half the registers get an
/// interesting value, the other half get a random 32-bit value.
fn gen_init_values(rng: &mut StdRng, count: usize) -> Vec<InitReg> {
    (0..count)
        .map(|num| {
            let val = if rng.gen_bool(0.5) {
                INTERESTING_VALUES[rng.gen_range(0..INTERESTING_VALUES.len())]
            } else {
                rng.gen::<u32>()
            };
            InitReg {
                num,
                hex: format!("{:08x}", val),
            }
        })
        .collect()
}

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

        // Generate init register values
        let init_regs = gen_init_values(&mut rng, 28);

        // Synthesize packets and collect metadata
        let (packets, instructions, candidate_names) = self.synthesize_packets(recipe, &mut rng)?;

        // Render the assembly template
        let template = TestProgramTemplate {
            init_regs,
            num_iterations: recipe.num_iterations,
            hvx: recipe.hvx,
            packets: &packets,
        };

        let assembly = template
            .render()
            .context("Failed to render assembly template")?;

        Ok(GeneratedProgram {
            assembly,
            instructions,
            candidate_names,
        })
    }

    /// Synthesize all packets for the steps function.
    ///
    /// Returns (packet_asm_lines, instruction_metadata, candidate_names).
    fn synthesize_packets(
        &self,
        recipe: &Recipe,
        rng: &mut StdRng,
    ) -> Result<(Vec<String>, Vec<GeneratedInstruction>, Vec<String>)> {
        // Build skip_terms and exclude_filters: when mem ops are enabled,
        // remove "mem" from skip terms but only allow safe base+offset IO
        // forms (names ending in _io). Block all other memory patterns.
        let (skip_terms, exclude_filters) = if recipe.synth.allow_mem_ops {
            let terms: Vec<String> = recipe
                .filters
                .skip_terms
                .iter()
                .filter(|t| t.to_lowercase() != "mem")
                .cloned()
                .collect();

            // Start with recipe's exclude filters, then add filters that
            // only allow simple load/store _io forms. This excludes:
            // absolute-addressed loads/stores, auto-increment, register-offset,
            // memops (read-modify-write), scatter/gather, etc.
            let mut excludes = recipe.filters.exclude.clone();
            // Exclude all memory ops that don't have "_io" in their name
            excludes.push(Filter::And(vec![
                Filter::Or(vec![
                    Filter::ByAttribute(AttributeFilter::MayLoad(true)),
                    Filter::ByAttribute(AttributeFilter::MayStore(true)),
                ]),
                Filter::Not(Box::new(Filter::ByNameContains("_io".to_string()))),
            ]));
            // Exclude read-modify-write memops (even _io ones) — they have
            // two immediates, count as stores, and complicate operand assignment
            excludes.push(Filter::ByNameContains("memop".to_string()));

            (terms, excludes)
        } else {
            (
                recipe.filters.skip_terms.clone(),
                recipe.filters.exclude.clone(),
            )
        };

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
            allow_mem_ops: recipe.synth.allow_mem_ops,
            mem_region_size: 65536,
        };

        let synth = PacketSynthesizer::new(self.db, config);

        if synth.candidates().is_empty() {
            anyhow::bail!("No candidate instructions after filtering");
        }

        let candidate_names: Vec<String> =
            synth.candidates().iter().map(|c| c.name.clone()).collect();

        let mut packets = Vec::with_capacity(recipe.num_packets);
        let mut instructions = Vec::new();

        for _ in 0..recipe.num_packets {
            let packet = synth.synthesize_packet(rng);
            packets.push(packet.to_asm());
            for ci in &packet.insns {
                instructions.push(GeneratedInstruction {
                    name: ci.def.name.clone(),
                    itype: ci.def.itype.clone(),
                    asm_text: ci.asm_text.clone(),
                });
            }
        }

        Ok((packets, instructions, candidate_names))
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
        assert!(program.assembly.contains("r0 = ##0x"));
        assert!(program.assembly.contains("r27 = ##mem_region"));
    }
}
