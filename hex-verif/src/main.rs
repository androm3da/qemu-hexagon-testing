//! Hexagon ISA emulator verification tool.
//!
//! `hex-verif` is a differential testing tool that compares the execution of
//! randomly generated Hexagon programs on two backends: a reference emulator
//! (hexagon-sim) and a test emulator (QEMU). It catches emulator bugs by
//! detecting register state mismatches after executing identical programs on
//! both backends.
//!
//! # How it works
//!
//! For each test iteration, hex-verif:
//!
//! 1. **Generates** a random Hexagon assembly program using [`hex_prog`].
//!    The program contains synthesized VLIW packets with randomized register
//!    and immediate operands, plus periodic mutation blocks for state entropy.
//!
//! 2. **Compiles** the assembly with `hexagon-clang` into an ELF binary.
//!
//! 3. **Runs** the ELF on hexagon-sim (reference) via GDB RSP,
//!    collecting register state at breakpoints.
//!
//! 4. **Runs** the same ELF on QEMU via GDB RSP,
//!    collecting register state at the same breakpoints.
//!
//! 5. **Compares** the register states. ISA-relevant registers (GPRs r0-r27,
//!    predicates, HVX vectors, loop/modifier regs, USR) are compared with
//!    hex normalization. Performance counters and revision registers are
//!    excluded since they legitimately differ between backends.
//!
//! 6. **On mismatch**, runs ddmin (delta debugging) to minimize the failing
//!    program to the smallest set of packets that still reproduces the bug.
//!
//! # Usage
//!
//! ```sh
//! # Basic run: 10 iterations, 10 packets each, seed 42
//! hex-verif --instset instructions.json
//!
//! # Larger test with more coverage
//! hex-verif --instset instructions.json -n 100 -p 50 --seed 0 -t 4
//!
//! # With a TOML recipe file for custom instruction filtering
//! hex-verif --instset instructions.json --recipe my_recipe.toml -n 50 -p 20
//!
//! # Custom toolchain and results directory
//! hex-verif --instset instructions.json \
//!     --toolchain /path/to/hexagon/tools \
//!     --qemu /path/to/qemu-system-hexagon \
//!     --results-dir /tmp/hex_verif_results
//! ```
//!
//! # Options
//!
//! | Flag | Default | Description |
//! |------|---------|-------------|
//! | `--instset` | (required) | Path to instruction database JSON |
//! | `-n`, `--iterations` | 10 | Number of test iterations |
//! | `-p`, `--packets` | 10 | VLIW packets per test program |
//! | `--seed` | 42 | Base random seed (deterministic) |
//! | `-t`, `--threads` | 1 | Worker threads for parallel testing |
//! | `--toolchain` | `/pkg/qct/.../21.0.01` | Hexagon toolchain root |
//! | `--qemu` | `/opt/Hexagon_SDK/.../qemu-system-hexagon` | QEMU binary |
//! | `--isa-version` | `v73` | ISA version for compilation |
//! | `--results-dir` | `results_<timestamp>` | Output directory |
//! | `--recipe` | (none) | TOML recipe file for instruction filtering |
//!
//! # Output
//!
//! On completion, hex-verif prints a verification summary (pass/fail/mismatch
//! counts, throughput) and an encoding space coverage report showing how much
//! of the instruction encoding space was exercised.
//!
//! Failing iterations are preserved in the results directory with:
//! - `test.S` -- the generated assembly
//! - `ref_output.txt` / `test_output.txt` -- register states from hexagon-sim / QEMU
//! - `diff_N.txt` -- register diffs at each breakpoint
//! - `minimized.S` -- the delta-debugged minimal reproducer
//!
//! Passing iterations are cleaned up to save disk space.
//!
//! # Crate architecture
//!
//! hex-verif is built from five workspace crates:
//!
//! - [`hex_dump`] -- Parses LLVM tablegen into structured instruction definitions
//! - [`hex_instset`] -- Indexed instruction database with filtering and register classes
//! - [`hex_packet`] -- VLIW packet synthesis with constraint validation
//! - [`hex_prog`] -- Complete test program generation (assembly templates)
//! - `hex-verif` (this crate) -- Differential testing orchestration

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;

mod comparison;
mod coverage;
mod reproducer;
mod runner;
mod stats;
mod stdout_parser;

use hex_prog::recipe_file::RecipeFile;
use runner::{RunMode, ToolchainPaths, VerificationConfig};

#[derive(Parser)]
#[command(name = "hex-verif", about = "Hexagon ISA emulator verification tool")]
struct Cli {
    /// Path to the instruction set JSON database.
    #[arg(long)]
    instset: PathBuf,

    /// Number of test iterations to run.
    #[arg(short = 'n', long, default_value = "10")]
    iterations: usize,

    /// Number of packets per test program.
    #[arg(short = 'p', long, default_value = "10")]
    packets: usize,

    /// Base random seed.
    #[arg(long, default_value = "42")]
    seed: u64,

    /// Number of worker threads.
    #[arg(short = 't', long, default_value = "1")]
    threads: usize,

    /// Path to Hexagon toolchain root.
    #[arg(
        long,
        default_value = "/pkg/qct/software/hexagon/releases/tools/21.0.01"
    )]
    toolchain: PathBuf,

    /// Path to QEMU binary.
    #[arg(
        long,
        default_value = "/opt/Hexagon_SDK/6.4.0.2/tools/Tools/QEMUHexagon/bin/qemu-system-hexagon"
    )]
    qemu: PathBuf,

    /// ISA version (e.g., v73).
    #[arg(long, default_value = "v73")]
    isa_version: String,

    /// Results output directory.
    #[arg(long)]
    results_dir: Option<PathBuf>,

    /// Path to a TOML recipe file for instruction filtering and synthesis settings.
    #[arg(long)]
    recipe: Option<PathBuf>,

    /// Maximum seconds to spend on ddmin minimization per mismatch.
    #[arg(long, default_value = "120")]
    ddmin_timeout: u64,

    /// Execution mode: 'direct' for fast stdout-based register capture,
    /// 'gdb' for GDB RSP-based register reads.
    #[arg(long, default_value = "direct")]
    mode: RunMode,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load instruction database
    let db =
        hex_instset::database::InstructionDb::load_from_json(&cli.instset).with_context(|| {
            format!(
                "Failed to load instruction database from {}",
                cli.instset.display()
            )
        })?;

    eprintln!(
        "Loaded {} instructions from {}",
        db.len(),
        cli.instset.display()
    );

    // Load recipe file if provided
    let recipe_file = if let Some(ref path) = cli.recipe {
        Some(
            RecipeFile::load(path)
                .with_context(|| format!("Failed to load recipe from {}", path.display()))?,
        )
    } else {
        None
    };

    // Build a template recipe (seed=0 placeholder, will be overridden per iteration)
    let template_recipe = if let Some(rf) = recipe_file {
        rf.into_recipe(0)?
    } else {
        hex_prog::recipe::Recipe::default()
    };

    // CLI overrides for packets and isa_version
    let mut template_recipe = template_recipe;
    template_recipe.num_packets = cli.packets;
    template_recipe.isa_version = cli.isa_version.clone();

    // Determine results directory
    let results_dir = cli.results_dir.unwrap_or_else(|| {
        let now = chrono_like_timestamp();
        PathBuf::from(format!("results_{}", now))
    });

    let config = VerificationConfig {
        toolchain: ToolchainPaths {
            toolchain_root: cli.toolchain,
            qemu_path: cli.qemu,
            isa_version: cli.isa_version,
        },
        results_dir,
        num_iterations: cli.iterations,
        packets_per_test: cli.packets,
        base_seed: cli.seed,
        num_threads: cli.threads,
        template_recipe,
        ddmin_timeout: Duration::from_secs(cli.ddmin_timeout),
        run_mode: cli.mode,
    };

    eprintln!(
        "Starting verification: {} iterations, {} packets/test, {} thread(s), seed={}, mode={}",
        config.num_iterations,
        config.packets_per_test,
        config.num_threads,
        config.base_seed,
        config.run_mode,
    );

    runner::run_verification(&config, &db)?;

    Ok(())
}

/// Generate a timestamp string without requiring the chrono crate.
fn chrono_like_timestamp() -> String {
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", duration.as_secs())
}
