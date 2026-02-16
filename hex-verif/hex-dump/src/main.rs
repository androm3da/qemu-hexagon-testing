//! Build the Hexagon instruction database from LLVM tablegen sources.
//!
//! This binary parses `HexagonDepInstrInfo.td` from the LLVM source tree
//! and writes a JSON instruction database (`instructions.json`) used by
//! the rest of the hex-verif toolchain.
//!
//! # Usage
//!
//! ```sh
//! hex-dump --llvm-project /path/to/llvm-project -o instructions.json
//! ```
//!
//! The `--llvm-project` path should point to the root of an llvm-project
//! checkout. The tool reads from
//! `<llvm-project>/llvm/lib/Target/Hexagon/HexagonDepInstrInfo.td`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use hex_dump::parser;

#[derive(Parser)]
#[command(
    name = "hex-dump",
    about = "Parse Hexagon tablegen into JSON instruction database"
)]
struct Cli {
    /// Path to the llvm-project source tree.
    #[arg(long)]
    llvm_project: PathBuf,

    /// Output JSON file path.
    #[arg(short, long, default_value = "instructions.json")]
    output: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let td_path = cli
        .llvm_project
        .join("llvm/lib/Target/Hexagon/HexagonDepInstrInfo.td");

    let content = std::fs::read_to_string(&td_path)
        .with_context(|| format!("Failed to read {}", td_path.display()))?;

    eprintln!("Parsing {}...", td_path.display());
    let dump = parser::parse_tablegen(&content)?;

    eprintln!(
        "Parsed {} defs total, {} real instructions after filtering",
        dump.total_parsed,
        dump.instructions.len()
    );

    let json = serde_json::to_string_pretty(&dump)?;
    std::fs::write(&cli.output, &json)
        .with_context(|| format!("Failed to write {}", cli.output.display()))?;

    eprintln!("Wrote {}", cli.output.display());
    Ok(())
}
