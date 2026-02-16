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
//!    and immediate operands.
//!
//! 2. **Compiles** the assembly with `hexagon-clang` into an ELF binary.
//!
//! 3. **Runs** the ELF on hexagon-sim (reference) via `hexagon-lldb` in
//!    batch mode, collecting register state at breakpoints.
//!
//! 4. **Runs** the same ELF on QEMU via `hexagon-lldb` + gdb-remote,
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
//! # Run for 40 minutes
//! hex-verif --instset instructions.json --runtime 40m -p 50 -t 4
//!
//! # Run for 1.5 hours with a recipe
//! hex-verif --instset instructions.json --runtime 1h30m --recipe my_recipe.toml
//!
//! # Fixed iteration count (original behavior)
//! hex-verif --instset instructions.json -n 100 -p 50 --seed 0 -t 4
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
//! | `-n`, `--iterations` | 10 | Number of test iterations (ignored if --runtime set) |
//! | `-p`, `--packets` | 10 | VLIW packets per test program |
//! | `--runtime` | (none) | Maximum runtime duration (e.g., `1h30m`, `40m`, `2h`) |
//! | `--seed` | 42 | Base random seed (deterministic) |
//! | `-t`, `--threads` | 1 | Worker threads for parallel testing |
//! | `--toolchain` | `/pkg/qct/.../21.0.01` | Hexagon toolchain root |
//! | `--qemu` | `/opt/Hexagon_SDK/.../qemu-system-hexagon` | QEMU binary |
//! | `--isa-version` | `v73` | ISA version for compilation |
//! | `--results-dir` | `results_<timestamp>` | Output directory |
//! | `--recipe` | (none) | TOML recipe file for instruction filtering |
//!
//! # Signal handling
//!
//! Press Ctrl-C to gracefully stop verification. In-progress iterations will
//! complete, and the normal verification summary and coverage report will be
//! printed. Press Ctrl-C a second time to abort immediately.
//!
//! # Output
//!
//! On completion, hex-verif prints a verification summary (pass/fail/mismatch
//! counts, throughput) and an encoding space coverage report showing how much
//! of the instruction encoding space was exercised.
//!
//! Failing iterations are preserved in the results directory with:
//! - `test.S` -- the generated assembly
//! - `ref_output.txt` / `test_output.txt` -- raw LLDB output
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;

mod comparison;
mod coverage;
mod lldb_script;
mod reproducer;
mod runner;
mod stats;

use hex_prog::recipe_file::RecipeFile;
use runner::{ToolchainPaths, VerificationConfig};

#[derive(Parser)]
#[command(name = "hex-verif", about = "Hexagon ISA emulator verification tool")]
struct Cli {
    /// Path to the instruction set JSON database.
    #[arg(long)]
    instset: PathBuf,

    /// Number of test iterations to run. Ignored when --runtime is set.
    #[arg(short = 'n', long)]
    iterations: Option<usize>,

    /// Number of packets per test program.
    #[arg(short = 'p', long, default_value = "10")]
    packets: usize,

    /// Maximum runtime duration (e.g., "1h30m", "40m", "2h", "90s").
    /// When set, runs iterations until the time budget expires.
    #[arg(long, value_parser = parse_duration_arg)]
    runtime: Option<Duration>,

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set up Ctrl-C handler
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_hook = Arc::clone(&shutdown);
    ctrlc::set_handler(move || {
        if shutdown_hook.load(Ordering::Relaxed) {
            // Second Ctrl-C: abort immediately
            eprintln!("\nForced abort.");
            std::process::exit(1);
        }
        eprintln!("\nReceived Ctrl-C, finishing in-progress iterations...");
        shutdown_hook.store(true, Ordering::Relaxed);
    })
    .expect("Failed to set Ctrl-C handler");

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

    // Determine iteration limit:
    //   --runtime set: no iteration limit (run until time expires)
    //   --iterations set: use that value
    //   neither: default to 10
    let max_iterations = match (cli.iterations, cli.runtime) {
        (Some(n), _) => Some(n),  // explicit -n always respected
        (None, Some(_)) => None,  // runtime-only: no iteration limit
        (None, None) => Some(10), // default: 10 iterations
    };

    // Determine deadline
    let deadline = cli.runtime.map(|d| Instant::now() + d);

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
        max_iterations,
        deadline,
        packets_per_test: cli.packets,
        base_seed: cli.seed,
        num_threads: cli.threads,
        template_recipe,
    };

    // Print startup message
    let limit_desc = match (config.max_iterations, cli.runtime) {
        (Some(n), Some(d)) => format!("{} iterations or {}", n, format_duration(d)),
        (Some(n), None) => format!("{} iterations", n),
        (None, Some(d)) => format!("runtime {}", format_duration(d)),
        (None, None) => "unlimited (Ctrl-C to stop)".to_string(),
    };
    eprintln!(
        "Starting verification: {}, {} packets/test, {} thread(s), seed={}",
        limit_desc, config.packets_per_test, config.num_threads, config.base_seed
    );

    runner::run_verification(&config, &db, shutdown)?;

    Ok(())
}

/// Parse a duration string like "1h30m", "40m", "2h", "90s", "1h30m45s".
///
/// Supports hours (`h`), minutes (`m`), and seconds (`s`) in any combination.
fn parse_duration_arg(s: &str) -> Result<Duration, String> {
    parse_duration(s).ok_or_else(|| {
        format!(
            "invalid duration '{}': expected format like '1h30m', '40m', '2h', '90s'",
            s
        )
    })
}

/// Parse a duration string into a `Duration`.
///
/// Returns `None` if the format is invalid.
fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let mut total_secs: u64 = 0;
    let mut current_num = String::new();
    let mut found_any = false;

    for c in s.chars() {
        if c.is_ascii_digit() {
            current_num.push(c);
        } else {
            let n: u64 = current_num.parse().ok()?;
            current_num.clear();
            match c {
                'h' => total_secs += n * 3600,
                'm' => total_secs += n * 60,
                's' => total_secs += n,
                _ => return None,
            }
            found_any = true;
        }
    }

    // Trailing digits without a suffix are invalid
    if !current_num.is_empty() || !found_any {
        return None;
    }

    Some(Duration::from_secs(total_secs))
}

/// Format a `Duration` for human display (e.g., "1h 30m", "40m", "2h").
pub fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;

    let mut parts = Vec::new();
    if h > 0 {
        parts.push(format!("{}h", h));
    }
    if m > 0 {
        parts.push(format!("{}m", m));
    }
    if s > 0 || parts.is_empty() {
        parts.push(format!("{}s", s));
    }
    parts.join(" ")
}

/// Generate a timestamp string without requiring the chrono crate.
fn chrono_like_timestamp() -> String {
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration("40m"), Some(Duration::from_secs(40 * 60)));
    }

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(2 * 3600)));
    }

    #[test]
    fn test_parse_duration_hours_minutes() {
        assert_eq!(
            parse_duration("1h30m"),
            Some(Duration::from_secs(3600 + 30 * 60))
        );
    }

    #[test]
    fn test_parse_duration_hours_minutes_seconds() {
        assert_eq!(
            parse_duration("1h30m45s"),
            Some(Duration::from_secs(3600 + 30 * 60 + 45))
        );
    }

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("40"), None); // no suffix
        assert_eq!(parse_duration("40x"), None); // bad suffix
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_secs(40 * 60)), "40m");
        assert_eq!(format_duration(Duration::from_secs(2 * 3600)), "2h");
        assert_eq!(
            format_duration(Duration::from_secs(3600 + 30 * 60)),
            "1h 30m"
        );
        assert_eq!(
            format_duration(Duration::from_secs(3600 + 30 * 60 + 45)),
            "1h 30m 45s"
        );
        assert_eq!(format_duration(Duration::from_secs(0)), "0s");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m 30s");
    }
}
