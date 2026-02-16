//! Hexagon instruction set database and query infrastructure.
//!
//! This crate provides an indexed, queryable instruction database built from
//! the raw instruction definitions produced by [`hex_dump`]. It is the primary
//! interface for looking up instructions by name, type, or arbitrary predicates.
//!
//! # Modules
//!
//! - [`database`] -- The [`database::InstructionDb`] indexed store. Supports
//!   O(1) lookup by name, lookup by instruction type, and predicate-based
//!   filtering. Also provides [`database::slot_mask_for_itype`] for VLIW
//!   slot assignment.
//! - [`filter`] -- Composable [`filter::Filter`] predicates for querying
//!   instructions by type, syntax, name, or boolean attributes. Filters can
//!   be combined with `And`, `Or`, and `Not`.
//! - [`register`] -- The [`register::RegisterClass`] enum representing
//!   Hexagon register files (IntRegs, DoubleRegs, PredRegs, HvxVR, etc.)
//!   with methods for register count, naming, and safe-index computation.
//!
//! # Loading the database
//!
//! ```no_run
//! use hex_instset::database::InstructionDb;
//! use std::path::Path;
//!
//! // From a pre-built JSON file (produced by hex-dump)
//! let db = InstructionDb::load_from_json(Path::new("instructions.json")).unwrap();
//! println!("{} instructions loaded", db.len());
//!
//! // Look up a specific instruction
//! if let Some(insn) = db.get("A2_add") {
//!     println!("{}: {}", insn.name, insn.asm_syntax);
//! }
//! ```
//!
//! # Filtering
//!
//! ```no_run
//! # use hex_instset::database::InstructionDb;
//! # use hex_instset::filter::{Filter, AttributeFilter};
//! # let db = InstructionDb::load_from_json(std::path::Path::new("instructions.json")).unwrap();
//! // Find all non-solo, non-call load instructions
//! let loads = db.filter(&Filter::And(vec![
//!     Filter::ByType("TypeLD".to_string()),
//!     Filter::ByAttribute(AttributeFilter::IsSolo(false)),
//!     Filter::ByAttribute(AttributeFilter::IsCall(false)),
//! ]));
//! ```
//!
//! # VLIW slot assignment
//!
//! Each Hexagon instruction type has a set of VLIW slots it can execute in.
//! Use [`database::slot_mask_for_itype`] or [`database::InstructionDb::slot_mask`]
//! to query which slots are valid for a given instruction:
//!
//! - Slot 0: stores, CVI stores
//! - Slots 0-1: loads, CVI loads
//! - Slots 2-3: ALU64, M-type, S-type, branches
//! - Slots 0-3: ALU32, CVI ALU

pub mod database;
pub mod filter;
pub mod query;
pub mod register;
