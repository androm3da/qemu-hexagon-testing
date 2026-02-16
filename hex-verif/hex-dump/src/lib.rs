//! Hexagon tablegen parser and instruction definition types.
//!
//! This crate parses the `HexagonDepInstrInfo.td` tablegen file from the
//! LLVM Hexagon backend and extracts structured instruction definitions.
//! It serves as the raw data source for the rest of the hex-verif toolchain.
//!
//! # Modules
//!
//! - [`types`] -- Data types representing parsed instruction definitions
//!   ([`types::InstructionDef`], [`types::Operand`], [`types::InstructionSetDump`]).
//! - [`parser`] -- The tablegen parser that reads `HexagonDepInstrInfo.td` and
//!   produces an [`types::InstructionSetDump`].
//!
//! # Usage
//!
//! ```no_run
//! use hex_dump::parser::parse_tablegen;
//!
//! let content = std::fs::read_to_string("HexagonDepInstrInfo.td").unwrap();
//! let dump = parse_tablegen(&content).unwrap();
//! println!("Parsed {} instructions", dump.instructions.len());
//! ```
//!
//! The resulting [`types::InstructionSetDump`] can be serialized to JSON with
//! serde for consumption by [`hex_instset`].
//!
//! # What gets parsed
//!
//! For each `def` block in the tablegen, the parser extracts:
//!
//! - **Operands** (outs/ins): register class, immediate type, operand name
//! - **Assembly syntax**: the human-readable instruction format
//! - **Instruction type**: `TypeALU32_3op`, `TypeLD`, `TypeST`, `TypeCVI_VA`, etc.
//! - **Boolean attributes**: `isPseudo`, `isSolo`, `mayLoad`, `isCVI`, etc.
//! - **Numeric attributes**: `opExtentBits`, `opExtentAlign`, `opNewValue`
//! - **Defs/Uses**: implicit register defs and uses (e.g. `USR_OVF`)
//! - **Requires**: ISA feature requirements (e.g. `UseHVXV65`)
//!
//! Pseudo and codegen-only instructions are filtered out during parsing.

pub mod parser;
pub mod types;
