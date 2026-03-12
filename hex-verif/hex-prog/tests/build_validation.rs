// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Build validation: generates hex-verif programs and compiles them to
//! measure the build success rate. This tests the full pipeline from
//! program generation through hexagon-clang assembly + linking.

use std::path::Path;
use std::process::Command;

use hex_instset::database::InstructionDb;
use hex_prog::recipe::Recipe;
use hex_prog::template::ProgramGenerator;

const INSTRUCTIONS_JSON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../instructions.json");

fn find_hexagon_clang() -> Option<String> {
    let candidates = ["/pkg/qct/software/hexagon/releases/tools/21.0.01/Tools/bin/hexagon-clang"];
    for c in &candidates {
        if Path::new(c).exists() {
            return Some(c.to_string());
        }
    }
    None
}

/// Generate and compile programs using the default recipe (same as hex-verif
/// without a --recipe flag). Reports the build success rate and categorizes
/// any assembly/link errors.
#[test]
#[ignore]
fn build_validation_default_recipe() {
    let clang = match find_hexagon_clang() {
        Some(c) => c,
        None => {
            eprintln!("SKIP: hexagon-clang not found");
            return;
        }
    };

    let db_path = Path::new(INSTRUCTIONS_JSON);
    if !db_path.exists() {
        eprintln!("SKIP: instructions.json not found");
        return;
    }

    let db = InstructionDb::load_from_json(db_path).expect("Failed to load instruction database");
    let gen = ProgramGenerator::new(&db);

    let tmpdir = tempfile::tempdir().expect("Failed to create temp dir");
    let total = 200;
    let mut pass = 0;
    let mut fail = 0;
    let mut error_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for i in 0..total {
        let recipe = Recipe {
            num_packets: 10,
            seed: i as u64,
            ..Recipe::default()
        };

        let program = match gen.generate(&recipe) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("  Generation failed (seed={i}): {e}");
                fail += 1;
                continue;
            }
        };

        let asm_path = tmpdir.path().join(format!("test_{}.S", i));
        let elf_path = tmpdir.path().join(format!("test_{}.elf", i));
        std::fs::write(&asm_path, &program.assembly).unwrap();

        let output = Command::new(&clang)
            .args(["-O2", "-g", "-G0", "-mv73"])
            .arg(&asm_path)
            .arg("-o")
            .arg(&elf_path)
            .output()
            .unwrap();

        if output.status.success() {
            pass += 1;
        } else {
            fail += 1;
            let stderr = String::from_utf8_lossy(&output.stderr);
            for line in stderr.lines() {
                if !line.contains("error:") {
                    continue;
                }
                let cat = if line.contains("out of slots") {
                    "out of slots"
                } else if line.contains("modified more than once") {
                    "register modified twice"
                } else if line.contains(".new") {
                    ".new not validly modified"
                } else if line.contains("invalid operand") {
                    "invalid operand"
                } else {
                    "other"
                };
                *error_counts.entry(cat.to_string()).or_insert(0) += 1;
            }
            // Print first few failures
            if fail <= 5 {
                for line in stderr.lines().filter(|l| l.contains("error:")).take(3) {
                    eprintln!("  [seed={i}] {line}");
                }
            }
        }
    }

    println!(
        "\nBuild results: {pass}/{total} passed, {fail}/{total} failed ({:.1}% success rate)",
        pass as f64 / total as f64 * 100.0
    );
    if !error_counts.is_empty() {
        println!("Error breakdown:");
        let mut sorted: Vec<_> = error_counts.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        for (cat, count) in sorted {
            println!("  {cat}: {count}");
        }
    }

    assert!(
        pass as f64 / total as f64 >= 0.95,
        "Build success rate {pass}/{total} is below 95%"
    );
}
