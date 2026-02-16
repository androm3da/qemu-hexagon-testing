use hex_instset::filter::{AttributeFilter, Filter};

use crate::skip_list::DEFAULT_SKIP_TERMS;

/// A recipe describing how to generate a test program.
///
/// Controls every aspect of test generation: program shape (packet count,
/// loop iterations), instruction selection (filters, skip terms, blocked
/// features), and synthesizer tuning (packet size, CVI limits).
///
/// # Defaults
///
/// `Recipe::default()` produces the same behaviour as the original
/// hardcoded logic: scalar-only, no HVX, no predicated-new or
/// new-value instructions, with the full [`DEFAULT_SKIP_TERMS`] list
/// and the standard attribute/feature blocklists.
///
/// # Examples
///
/// ```
/// use hex_prog::recipe::Recipe;
///
/// // Minimal override -- just change packet count and seed.
/// let recipe = Recipe {
///     num_packets: 50,
///     seed: 99,
///     ..Recipe::default()
/// };
/// assert_eq!(recipe.num_packets, 50);
/// assert_eq!(recipe.synth.max_packet_size, 4); // inherited default
/// ```
///
/// ```
/// use hex_prog::recipe::{Recipe, RecipeFilters, SynthSettings};
/// use hex_instset::query::parse_query;
///
/// // Build a recipe that only synthesizes ALU32 instructions.
/// let recipe = Recipe {
///     num_packets: 20,
///     seed: 0,
///     filters: RecipeFilters {
///         include: Some(parse_query("type is TypeALU32_3op").unwrap()),
///         ..RecipeFilters::default()
///     },
///     ..Recipe::default()
/// };
/// assert!(recipe.filters.include.is_some());
/// ```
///
/// For loading recipes from TOML files, see [`crate::recipe_file::RecipeFile`].
#[derive(Debug, Clone)]
pub struct Recipe {
    /// Number of synthesized packets in the `steps` function.
    pub num_packets: usize,
    /// Number of iterations for the body loop.
    pub num_iterations: usize,
    /// Seed for deterministic RNG.
    pub seed: u64,
    /// ISA version string, e.g. `"v73"`.
    pub isa_version: String,
    /// Whether to include HVX instructions.
    ///
    /// When `true`, the generated program will:
    /// - Initialize HVX vector registers in `init`
    /// - Emit an `hvx_mem_region` data section
    /// - Pass `-mhvx` to hexagon-clang during compilation
    pub hvx: bool,
    /// Parsed query filters for instruction selection.
    ///
    /// See [`RecipeFilters`] for details on each filtering stage.
    pub filters: RecipeFilters,
    /// Synthesizer tuning knobs.
    ///
    /// See [`SynthSettings`] for details on each parameter.
    pub synth: SynthSettings,
}

/// Parsed filter configuration for instruction selection.
///
/// Filters are applied in order during candidate list construction:
///
/// 1. **`exclude`** -- Each filter is tested against every instruction.
///    If *any* exclude filter matches, the instruction is dropped.
/// 2. **`include`** -- If set, only instructions matching this filter
///    survive. Instructions that passed the exclude stage but fail the
///    include filter are dropped.
/// 3. **`blocked_features`** -- Instructions whose `requires` list
///    contains any blocked feature string are dropped.
/// 4. **`skip_terms`** -- Instructions whose name or assembly syntax
///    (case-insensitive) contains any skip term are dropped.
///
/// # Defaults
///
/// `RecipeFilters::default()` reproduces the original hardcoded
/// filtering logic:
///
/// - **exclude:** solo, call, return, side-effects, predicated-new,
///   and branch types (TypeJ, TypeCJ, TypeNCJ, TypeCR)
/// - **include:** `None` (all non-excluded instructions are candidates)
/// - **blocked_features:** UseAudio, UseCompound, UseCabac, UseZReg,
///   HasV81, UseHVXV79, UseHVXV81, UseHVXFloatingPoint, UseHVXIEEEFP,
///   UseHVXQFloat
/// - **skip_terms:** the full [`DEFAULT_SKIP_TERMS`] list
///
/// # Examples
///
/// ```
/// use hex_prog::recipe::RecipeFilters;
/// use hex_instset::query::parse_query;
///
/// // Only saturating instructions, with default excludes.
/// let filters = RecipeFilters {
///     include: Some(parse_query("syntax contains :sat").unwrap()),
///     ..RecipeFilters::default()
/// };
/// ```
///
/// ```
/// use hex_prog::recipe::RecipeFilters;
/// use hex_instset::query::parse_query;
///
/// // Custom exclude list -- drop solo and memory instructions.
/// let filters = RecipeFilters {
///     exclude: vec![
///         parse_query("is solo").unwrap(),
///         parse_query("may load or may store").unwrap(),
///     ],
///     ..RecipeFilters::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct RecipeFilters {
    /// Exclude expressions (applied as NOT filters to the candidate pool).
    ///
    /// Each entry is a parsed query expression. An instruction matching
    /// *any* of these is excluded from synthesis.
    pub exclude: Vec<Filter>,
    /// Include expression (positive filter for the candidate pool).
    ///
    /// When set, only instructions matching this filter are considered
    /// for synthesis (after the exclude stage). When `None`, all
    /// non-excluded instructions are candidates.
    pub include: Option<Filter>,
    /// Feature blocklist.
    ///
    /// Instructions whose `requires` list contains any of these feature
    /// strings (substring match) are excluded. This is useful for
    /// avoiding ISA extensions not supported by the target emulator.
    pub blocked_features: Vec<String>,
    /// Skip terms (name/syntax substring exclusions).
    ///
    /// Instructions whose name **or** assembly syntax (case-insensitive)
    /// contains any of these terms are excluded from synthesis. This
    /// provides a simple, coarse exclusion mechanism complementing the
    /// query-based `exclude` filters.
    pub skip_terms: Vec<String>,
}

/// Synthesizer tuning settings.
///
/// These control VLIW packet construction behaviour. The defaults are
/// conservative choices that maximise assembly success rate.
///
/// # Defaults
///
/// | Field | Default | Description |
/// |---|---|---|
/// | `max_packet_size` | 4 | Full VLIW width |
/// | `max_cvi_per_packet` | 1 | At most 1 CVI slot used |
/// | `allow_predicated_new` | `false` | `.new` predicates disabled |
/// | `allow_new_value` | `false` | New-value consumers disabled |
/// | `allow_mem_ops` | `false` | Load/store instructions disabled |
///
/// # Examples
///
/// ```
/// use hex_prog::recipe::SynthSettings;
///
/// // Allow larger packets with up to 2 CVI instructions.
/// let synth = SynthSettings {
///     max_packet_size: 4,
///     max_cvi_per_packet: 2,
///     ..SynthSettings::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct SynthSettings {
    /// Maximum number of instructions per packet (1--4).
    pub max_packet_size: usize,
    /// Maximum number of CVI (HVX) instructions per packet.
    ///
    /// Set to 0 when `hvx` is `false` in the parent [`Recipe`] to
    /// avoid synthesizing any HVX instructions.
    pub max_cvi_per_packet: usize,
    /// Allow `.new` predicate forms (e.g., `if (p0.new)`).
    ///
    /// These require careful packet ordering and are disabled by
    /// default to avoid assembler errors.
    pub allow_predicated_new: bool,
    /// Allow new-value consumers (`.new` register reads).
    ///
    /// These require the producer to be in the same packet and are
    /// disabled by default to avoid assembler errors.
    pub allow_new_value: bool,
    /// Allow load/store instructions.
    ///
    /// When enabled, simple base+offset memory operations (e.g.
    /// `memw(r27+#offset)`) are synthesized. The base register is forced
    /// to r27 (which points at `mem_region`), offsets are constrained to
    /// stay within bounds, and r27 is protected from being used as a
    /// destination register.
    ///
    /// Auto-increment (`++`), register-offset (`<<`), and HVX memory
    /// operations are excluded for safety.
    pub allow_mem_ops: bool,
}

impl Default for Recipe {
    fn default() -> Self {
        Self {
            num_packets: 10,
            num_iterations: 3,
            seed: 42,
            isa_version: "v73".to_string(),
            hvx: false,
            filters: RecipeFilters::default(),
            synth: SynthSettings::default(),
        }
    }
}

impl Default for RecipeFilters {
    fn default() -> Self {
        Self {
            exclude: vec![
                Filter::ByAttribute(AttributeFilter::IsSolo(true)),
                Filter::ByAttribute(AttributeFilter::IsCall(true)),
                Filter::ByAttribute(AttributeFilter::IsReturn(true)),
                Filter::ByAttribute(AttributeFilter::HasSideEffects(true)),
                Filter::ByAttribute(AttributeFilter::IsPredicatedNew(true)),
                Filter::Or(vec![
                    Filter::ByType("TypeJ".to_string()),
                    Filter::ByType("TypeCJ".to_string()),
                    Filter::ByType("TypeNCJ".to_string()),
                    Filter::ByType("TypeCR".to_string()),
                ]),
            ],
            include: None,
            blocked_features: vec![
                "UseAudio".to_string(),
                "UseCompound".to_string(),
                "UseCabac".to_string(),
                "UseZReg".to_string(),
                "HasV81".to_string(),
                "UseHVXV79".to_string(),
                "UseHVXV81".to_string(),
                "UseHVXFloatingPoint".to_string(),
                "UseHVXIEEEFP".to_string(),
                "UseHVXQFloat".to_string(),
            ],
            skip_terms: DEFAULT_SKIP_TERMS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl Default for SynthSettings {
    fn default() -> Self {
        Self {
            max_packet_size: 4,
            max_cvi_per_packet: 1,
            allow_predicated_new: false,
            allow_new_value: false,
            allow_mem_ops: false,
        }
    }
}
