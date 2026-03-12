//! Hexagon test program generation.
//!
//! This crate generates complete, compilable Hexagon assembly test programs
//! from a [`recipe::Recipe`] specification. The generated programs are
//! self-contained `.S` files that can be assembled with `hexagon-clang` and
//! executed on hexagon-sim or QEMU.
//!
//! # Program structure
//!
//! Each generated program has the following layout:
//!
//! ```text
//! main:           Entry point. Saves callee-saved registers, calls init, body.
//! init:           Initializes GPRs r0-r27 with seed-derived values, sets up
//!                 predicate registers, loop counter (r28), and memory base (r27).
//! body:           Loop wrapper that calls steps() N times (configurable).
//! steps:          The test payload -- synthesized VLIW packets with periodic
//!                 register mutation blocks for entropy.
//! test_end:       Label marking the end of test execution (breakpoint target).
//! .data:          A 64KB memory region for load/store testing.
//! ```
//!
//! # Modules
//!
//! - [`template`] -- The [`template::ProgramGenerator`] orchestrates program
//!   generation. Returns a [`template::GeneratedProgram`] containing the
//!   assembly text, per-instruction metadata, and the synthesizable candidate
//!   pool for coverage analysis.
//!
//! - [`recipe`] -- The [`recipe::Recipe`] struct configures generation:
//!   number of packets, loop iterations, RNG seed, ISA version, HVX mode,
//!   instruction filters, and synthesis settings.
//!
//! - [`recipe_file`] -- TOML recipe file loading. Parses recipe TOML files
//!   with English predicate query expressions for instruction filtering.
//!
//! - [`mutation`] -- Generates register mutation blocks (brev, togglebit,
//!   rol, xor for GPRs; vnot, vxor, vdelta for HVX) inserted between
//!   packets to increase state entropy.
//!
//! - [`skip_list`] -- Default exclusion terms for instruction synthesis.
//!   Instructions matching these terms (e.g. `mem`, `jump`, `call`, `r29`)
//!   are skipped to avoid crashes, control flow changes, or reserved
//!   register clobbers.
//!
//! # Example
//!
//! ```no_run
//! # use hex_instset::database::InstructionDb;
//! use hex_prog::recipe::Recipe;
//! use hex_prog::template::ProgramGenerator;
//!
//! # let db = InstructionDb::load_from_json(std::path::Path::new("instructions.json")).unwrap();
//! let gen = ProgramGenerator::new(&db);
//! let recipe = Recipe {
//!     num_packets: 50,
//!     seed: 42,
//!     ..Recipe::default()
//! };
//! let program = gen.generate(&recipe).unwrap();
//! std::fs::write("test.S", &program.assembly).unwrap();
//! ```

pub mod mutation;
pub mod recipe;
pub mod recipe_file;
pub mod setup;
pub mod skip_list;
pub mod template;
