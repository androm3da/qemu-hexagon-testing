//! TOML recipe file loading.
//!
//! Loads a [`Recipe`] from a TOML file, parsing English predicate query
//! expressions (see [`hex_instset::query`]) in the `[filters]` section
//! into [`Filter`](hex_instset::filter::Filter) AST nodes.
//!
//! Every field in the TOML is optional. Omitted fields fall back to the
//! same defaults as [`Recipe::default()`], so an empty file produces
//! identical behaviour to running without `--recipe`.
//!
//! # File format
//!
//! ```toml
//! [generation]
//! num_packets = 50          # packets in the steps function
//! num_iterations = 3        # body-loop iterations
//! isa_version = "v73"       # compilation target
//!
//! [synthesis]
//! max_packet_size = 4       # VLIW width (1--4)
//! max_cvi_per_packet = 1    # HVX slot limit
//! allow_predicated_new = false
//! allow_new_value = false
//!
//! [synthesis.compiler]
//! hvx = true                # pass -mhvx to hexagon-clang
//!
//! [filters]
//! # Exclude expressions -- instructions matching ANY entry are dropped.
//! # Each string is an English predicate query expression.
//! exclude = [
//!     "is solo",
//!     "is call or is return",
//!     "has side_effects",
//!     "is predicated_new",
//!     "type is TypeJ or type is TypeCJ or type is TypeNCJ or type is TypeCR",
//! ]
//!
//! # Include expression -- only instructions matching this survive.
//! # Omit to keep all non-excluded instructions.
//! # include = "type is TypeALU32_3op"
//!
//! # Feature blocklist -- instructions requiring these are dropped.
//! blocked_features = [
//!     "UseAudio", "UseCompound", "UseCabac", "UseZReg",
//!     "HasV81", "UseHVXV79", "UseHVXV81",
//!     "UseHVXFloatingPoint", "UseHVXIEEEFP", "UseHVXQFloat",
//! ]
//!
//! # Skip terms -- name/syntax substring exclusions.
//! skip_terms = [
//!     "mem", "swi", "trap", "jump", "call", "dealloc",
//!     "allocframe", "r29", "r30", "r31",
//! ]
//! ```
//!
//! # Query expression language
//!
//! The `exclude` and `include` fields accept English predicate query
//! expressions. See [`hex_instset::query`] for the full grammar. Some
//! examples:
//!
//! | Expression | Meaning |
//! |---|---|
//! | `"is solo"` | Solo-only instructions |
//! | `"may load or may store"` | Memory instructions |
//! | `"type is TypeALU32_3op"` | Specific instruction type |
//! | `"has HvxVR operand"` | Uses HVX vector registers |
//! | `"syntax contains :sat"` | Saturating instructions |
//! | `"not is cvi"` | Non-CVI instructions |
//! | `"(is call or is return) and not is predicated"` | Compound expression |
//!
//! # Examples
//!
//! ```
//! use hex_prog::recipe_file::RecipeFile;
//!
//! // Parse a minimal recipe from a TOML string.
//! let toml = r#"
//! [generation]
//! num_packets = 20
//! "#;
//! let rf: RecipeFile = toml::from_str(toml).unwrap();
//! let recipe = rf.into_recipe(42).unwrap();
//! assert_eq!(recipe.num_packets, 20);
//! assert_eq!(recipe.num_iterations, 3); // default
//! ```
//!
//! ```
//! use hex_prog::recipe_file::RecipeFile;
//!
//! // A recipe targeting only saturating ALU instructions.
//! let toml = r#"
//! [filters]
//! include = "syntax contains :sat"
//! exclude = ["is solo", "is call or is return", "has side_effects"]
//! "#;
//! let rf: RecipeFile = toml::from_str(toml).unwrap();
//! let recipe = rf.into_recipe(0).unwrap();
//! assert!(recipe.filters.include.is_some());
//! assert_eq!(recipe.filters.exclude.len(), 3);
//! ```

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use hex_instset::filter::Filter;
use hex_instset::query::parse_query;

use crate::recipe::{ExecutionMode, Recipe, RecipeFilters, SynthSettings};
use crate::skip_list::DEFAULT_SKIP_TERMS;

/// A recipe loaded from a TOML file.
///
/// All sections are optional -- omitted fields inherit the defaults from
/// [`Recipe::default()`]. Use [`RecipeFile::load`] to read from disk, or
/// deserialize directly with [`toml::from_str`]. Then call
/// [`into_recipe`](RecipeFile::into_recipe) to produce a ready-to-use
/// [`Recipe`].
///
/// See the [module-level documentation](self) for the full TOML schema.
#[derive(Debug, Deserialize)]
pub struct RecipeFile {
    /// `[generation]` -- program shape settings.
    pub generation: Option<GenerationConfig>,
    /// `[synthesis]` -- packet synthesizer tuning and compiler flags.
    pub synthesis: Option<SynthesisConfig>,
    /// `[filters]` -- instruction selection queries and blocklists.
    pub filters: Option<FilterConfig>,
}

/// `[generation]` section -- controls program shape.
#[derive(Debug, Deserialize)]
pub struct GenerationConfig {
    /// Number of synthesized packets (default: 10).
    pub num_packets: Option<usize>,
    /// Body-loop iterations (default: 3).
    pub num_iterations: Option<usize>,
    /// ISA version for compilation, e.g. `"v73"` (default: `"v73"`).
    pub isa_version: Option<String>,
}

/// `[synthesis]` section -- packet synthesizer tuning.
#[derive(Debug, Deserialize)]
pub struct SynthesisConfig {
    /// Maximum instructions per packet, 1--4 (default: 4).
    pub max_packet_size: Option<usize>,
    /// Maximum CVI (HVX) instructions per packet (default: 1).
    pub max_cvi_per_packet: Option<usize>,
    /// Allow `.new` predicate forms (default: `false`).
    pub allow_predicated_new: Option<bool>,
    /// Allow new-value consumers (default: `false`).
    pub allow_new_value: Option<bool>,
    /// Allow load/store instructions (default: `false`).
    pub allow_memory_ops: Option<bool>,
    /// Allow jump instructions (default: `false`).
    pub allow_jumps: Option<bool>,
    /// Jumps only go forward (default: `true`).
    pub forward_only_jumps: Option<bool>,
    /// Enable pageable memory for TLB miss + replay testing (default: `false`).
    pub allow_pageable: Option<bool>,
    /// `[synthesis.compiler]` -- compiler-related flags.
    pub compiler: Option<CompilerConfig>,
}

/// `[synthesis.compiler]` section -- compiler flags.
#[derive(Debug, Deserialize)]
pub struct CompilerConfig {
    /// Pass `-mhvx` to hexagon-clang and enable HVX init/data
    /// sections (default: `false`).
    pub hvx: Option<bool>,
}

/// `[filters]` section -- instruction selection.
///
/// Each field maps to a stage of the candidate filtering pipeline.
/// See [`RecipeFilters`] for how the stages interact.
#[derive(Debug, Deserialize)]
pub struct FilterConfig {
    /// English predicate query expressions for excluding instructions.
    ///
    /// Each string is parsed by [`hex_instset::query::parse_query`].
    /// If omitted, the default exclude list is used (solo, call,
    /// return, side-effects, predicated-new, and branch types).
    pub exclude: Option<Vec<String>>,
    /// English predicate query expression for the positive filter.
    ///
    /// Only instructions matching this expression survive. If omitted,
    /// all non-excluded instructions are candidates.
    pub include: Option<String>,
    /// Feature blocklist (substring matches against `requires`).
    ///
    /// If omitted, the default blocklist is used.
    pub blocked_features: Option<Vec<String>>,
    /// Name/syntax substring exclusions.
    ///
    /// If omitted, [`DEFAULT_SKIP_TERMS`] is used.
    pub skip_terms: Option<Vec<String>>,
}

impl RecipeFile {
    /// Load a recipe from a TOML file on disk.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read recipe file: {}", path.display()))?;
        let recipe: RecipeFile = toml::from_str(&content)
            .with_context(|| format!("Failed to parse recipe TOML: {}", path.display()))?;
        Ok(recipe)
    }

    /// Convert into a Recipe, using defaults for any omitted fields.
    /// The `seed` is provided externally (per-iteration).
    pub fn into_recipe(self, seed: u64) -> Result<Recipe> {
        let defaults = Recipe::default();

        // Generation settings
        let num_packets = self
            .generation
            .as_ref()
            .and_then(|g| g.num_packets)
            .unwrap_or(defaults.num_packets);
        let num_iterations = self
            .generation
            .as_ref()
            .and_then(|g| g.num_iterations)
            .unwrap_or(defaults.num_iterations);
        let isa_version = self
            .generation
            .as_ref()
            .and_then(|g| g.isa_version.clone())
            .unwrap_or(defaults.isa_version);

        // Synthesis settings
        let synth_defaults = SynthSettings::default();
        let synth = SynthSettings {
            max_packet_size: self
                .synthesis
                .as_ref()
                .and_then(|s| s.max_packet_size)
                .unwrap_or(synth_defaults.max_packet_size),
            max_cvi_per_packet: self
                .synthesis
                .as_ref()
                .and_then(|s| s.max_cvi_per_packet)
                .unwrap_or(synth_defaults.max_cvi_per_packet),
            allow_predicated_new: self
                .synthesis
                .as_ref()
                .and_then(|s| s.allow_predicated_new)
                .unwrap_or(synth_defaults.allow_predicated_new),
            allow_new_value: self
                .synthesis
                .as_ref()
                .and_then(|s| s.allow_new_value)
                .unwrap_or(synth_defaults.allow_new_value),
            allow_memory_ops: self
                .synthesis
                .as_ref()
                .and_then(|s| s.allow_memory_ops)
                .unwrap_or(synth_defaults.allow_memory_ops),
            allow_jumps: self
                .synthesis
                .as_ref()
                .and_then(|s| s.allow_jumps)
                .unwrap_or(synth_defaults.allow_jumps),
            forward_only_jumps: self
                .synthesis
                .as_ref()
                .and_then(|s| s.forward_only_jumps)
                .unwrap_or(synth_defaults.forward_only_jumps),
            allow_pageable: self
                .synthesis
                .as_ref()
                .and_then(|s| s.allow_pageable)
                .unwrap_or(synth_defaults.allow_pageable),
        };

        // Compiler/HVX setting
        let hvx = self
            .synthesis
            .as_ref()
            .and_then(|s| s.compiler.as_ref())
            .and_then(|c| c.hvx)
            .unwrap_or(defaults.hvx);

        // Filter settings
        let filters = if let Some(fc) = self.filters {
            // Parse exclude expressions
            let exclude = if let Some(excl_strs) = fc.exclude {
                excl_strs
                    .iter()
                    .map(|s| {
                        parse_query(s)
                            .map_err(|e| anyhow::anyhow!("Failed to parse exclude '{}': {}", s, e))
                    })
                    .collect::<Result<Vec<Filter>>>()?
            } else {
                RecipeFilters::default().exclude
            };

            // Parse include expression
            let include =
                if let Some(inc_str) = fc.include {
                    Some(parse_query(&inc_str).map_err(|e| {
                        anyhow::anyhow!("Failed to parse include '{}': {}", inc_str, e)
                    })?)
                } else {
                    None
                };

            // Blocked features
            let blocked_features = fc
                .blocked_features
                .unwrap_or_else(|| RecipeFilters::default().blocked_features);

            // Skip terms
            let skip_terms = fc
                .skip_terms
                .unwrap_or_else(|| DEFAULT_SKIP_TERMS.iter().map(|s| s.to_string()).collect());

            RecipeFilters {
                exclude,
                include,
                blocked_features,
                skip_terms,
            }
        } else {
            RecipeFilters::default()
        };

        Ok(Recipe {
            num_packets,
            num_iterations,
            seed,
            isa_version,
            hvx,
            execution_mode: ExecutionMode::Direct,
            filters,
            synth,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_minimal_recipe() {
        let toml_str = r#"
[generation]
num_packets = 20
"#;
        let recipe_file: RecipeFile = toml::from_str(toml_str).unwrap();
        let recipe = recipe_file.into_recipe(42).unwrap();
        assert_eq!(recipe.num_packets, 20);
        assert_eq!(recipe.num_iterations, 3); // default
        assert!(!recipe.hvx); // default
        assert!(!recipe.filters.exclude.is_empty()); // defaults applied
    }

    #[test]
    fn test_load_full_recipe() {
        let toml_str = r#"
[generation]
num_packets = 50
num_iterations = 5
isa_version = "v73"

[synthesis]
max_packet_size = 3
max_cvi_per_packet = 2
allow_predicated_new = true
allow_new_value = false

[synthesis.compiler]
hvx = true

[filters]
exclude = ["is solo", "is call or is return"]
include = "type is TypeALU32_3op"
blocked_features = ["UseAudio"]
skip_terms = ["mem", "jump"]
"#;
        let recipe_file: RecipeFile = toml::from_str(toml_str).unwrap();
        let recipe = recipe_file.into_recipe(99).unwrap();

        assert_eq!(recipe.num_packets, 50);
        assert_eq!(recipe.num_iterations, 5);
        assert_eq!(recipe.seed, 99);
        assert!(recipe.hvx);
        assert_eq!(recipe.synth.max_packet_size, 3);
        assert_eq!(recipe.synth.max_cvi_per_packet, 2);
        assert!(recipe.synth.allow_predicated_new);
        assert!(!recipe.synth.allow_new_value);
        assert_eq!(recipe.filters.exclude.len(), 2);
        assert!(recipe.filters.include.is_some());
        assert_eq!(recipe.filters.blocked_features, vec!["UseAudio"]);
        assert_eq!(recipe.filters.skip_terms, vec!["mem", "jump"]);
    }

    #[test]
    fn test_default_recipe_matches_current() {
        // Empty TOML should produce the same defaults as Recipe::default()
        let toml_str = "";
        let recipe_file: RecipeFile = toml::from_str(toml_str).unwrap();
        let recipe = recipe_file.into_recipe(42).unwrap();
        let default = Recipe::default();

        assert_eq!(recipe.num_packets, default.num_packets);
        assert_eq!(recipe.num_iterations, default.num_iterations);
        assert_eq!(recipe.isa_version, default.isa_version);
        assert_eq!(recipe.hvx, default.hvx);
        assert_eq!(recipe.synth.max_packet_size, default.synth.max_packet_size);
        assert_eq!(
            recipe.filters.blocked_features,
            default.filters.blocked_features
        );
        assert_eq!(recipe.filters.skip_terms, default.filters.skip_terms);
        assert_eq!(recipe.filters.exclude.len(), default.filters.exclude.len());
    }

    #[test]
    fn test_parse_error_in_exclude() {
        let toml_str = r#"
[filters]
exclude = ["is solo", "bogus keyword blah"]
"#;
        let recipe_file: RecipeFile = toml::from_str(toml_str).unwrap();
        let result = recipe_file.into_recipe(42);
        assert!(result.is_err());
    }
}
