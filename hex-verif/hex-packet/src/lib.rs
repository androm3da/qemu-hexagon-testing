//! Hexagon VLIW packet synthesis and validation.
//!
//! This crate generates legal Hexagon VLIW packets -- groups of 1-4
//! instructions that execute in parallel in a single clock cycle. It handles
//! the complex constraints of Hexagon's VLIW architecture:
//!
//! - **Slot assignment**: each instruction type can only execute in certain
//!   slots (0-3). The synthesizer ensures a valid slot mapping exists.
//! - **Resource limits**: at most 2 memory operations, at most 1 store,
//!   CVI resource budgets, branch count limits.
//! - **Register conflicts**: no unconditional double-writes to the same
//!   register within a packet.
//! - **Solo/SoloAX constraints**: some instructions must execute alone or
//!   only alongside ALU32-type instructions.
//!
//! # Modules
//!
//! - [`synthesizer`] -- The [`synthesizer::PacketSynthesizer`] is the main
//!   entry point. It pre-filters the instruction database into a candidate
//!   pool, then generates random legal packets with concrete register and
//!   immediate assignments. Key types:
//!   - [`synthesizer::ConcreteInsn`] -- a fully resolved instruction with
//!     register names, immediate values, and assembly text.
//!   - [`synthesizer::Packet`] -- a VLIW packet of 1-4 concrete instructions.
//!   - [`synthesizer::SynthConfig`] -- controls packet size, predication,
//!     new-value usage, CVI limits, and skip terms.
//!
//! - [`constraint`] -- Packet validation rules. [`constraint::validate_packet`]
//!   checks instruction-level rules (solo, slot, memory, branch limits) and
//!   [`constraint::validate_concrete_packet`] adds register-conflict checks.
//!
//! - [`slot`] -- VLIW slot assignment via a greedy most-restrictive-first
//!   algorithm. [`slot::assign_slots`] maps instruction types to slots 0-3.
//!
//! # Example
//!
//! ```no_run
//! # use hex_instset::database::InstructionDb;
//! use hex_packet::synthesizer::{PacketSynthesizer, SynthConfig};
//! use rand::rngs::StdRng;
//! use rand::SeedableRng;
//!
//! # let db = InstructionDb::load_from_json(std::path::Path::new("instructions.json")).unwrap();
//! let config = SynthConfig::default();
//! let synth = PacketSynthesizer::new(&db, config);
//! let mut rng = StdRng::seed_from_u64(42);
//!
//! let packet = synth.synthesize_packet(&mut rng).unwrap();
//! println!("{}", packet.to_asm());
//! // Output: "    { r5 = add(r12,r3) ; r7 = sub(r20,r1) ; ... }"
//! ```

pub mod constraint;
pub mod slot;
pub mod synthesizer;
