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
//! steps:          The test payload -- synthesized VLIW packets.
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
//! - [`skip_list`] -- Default exclusion terms for instruction synthesis.
//!   Instructions matching these terms (e.g. `mem`, `jump`, `call`, `r29`)
//!   are skipped to avoid crashes, control flow changes, or reserved
//!   register clobbers.
//!
//! # Examples
//!
//! Generate a test program with default settings:
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
//!
//! Load a recipe from a TOML file:
//!
//! ```no_run
//! # use hex_instset::database::InstructionDb;
//! use hex_prog::recipe_file::RecipeFile;
//! use hex_prog::template::ProgramGenerator;
//!
//! # let db = InstructionDb::load_from_json(std::path::Path::new("instructions.json")).unwrap();
//! let rf = RecipeFile::load(std::path::Path::new("recipe.toml")).unwrap();
//! let recipe = rf.into_recipe(42).unwrap();
//!
//! let gen = ProgramGenerator::new(&db);
//! let program = gen.generate(&recipe).unwrap();
//! std::fs::write("test.S", &program.assembly).unwrap();
//! ```
//!
//! Build a recipe programmatically with custom filters:
//!
//! ```
//! use hex_prog::recipe::{Recipe, RecipeFilters};
//! use hex_instset::query::parse_query;
//!
//! let recipe = Recipe {
//!     num_packets: 20,
//!     seed: 0,
//!     filters: RecipeFilters {
//!         include: Some(parse_query("syntax contains :sat").unwrap()),
//!         ..RecipeFilters::default()
//!     },
//!     ..Recipe::default()
//! };
//! ```

pub mod recipe;
pub mod recipe_file;
pub mod skip_list;
pub mod template;
