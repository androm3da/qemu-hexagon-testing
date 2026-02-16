use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use hex_dump::parser::parse_tablegen;
use hex_instset::database::InstructionDb;
use hex_prog::recipe::{Recipe, SynthSettings};
use hex_prog::template::ProgramGenerator;

const TABLEGEN_PATH: &str =
    "/local/mnt/workspace/upstream/llvm-project/llvm/lib/Target/Hexagon/HexagonDepInstrInfo.td";
const HEXAGON_CLANG: &str =
    "/pkg/qct/software/hexagon/releases/tools/21.0.01/Tools/bin/hexagon-clang";

fn load_real_db() -> Option<InstructionDb> {
    let path = Path::new(TABLEGEN_PATH);
    if !path.exists() {
        eprintln!("Skipping: {} not found", TABLEGEN_PATH);
        return None;
    }
    let content = std::fs::read_to_string(path).unwrap();
    let dump = parse_tablegen(&content).unwrap();
    Some(InstructionDb::from_dump(dump))
}

fn have_clang() -> bool {
    Path::new(HEXAGON_CLANG).exists()
}

/// Extract the first assembler error line from clang stderr.
fn categorize_error(stderr: &str) -> String {
    for line in stderr.lines() {
        if line.contains("error:") {
            // Strip the file path prefix to get just the error message
            if let Some(idx) = line.find("error:") {
                return line[idx..].to_string();
            }
        }
    }
    "unknown error".to_string()
}

struct StressResult {
    pass: usize,
    fail: usize,
    gen_fail: usize,
    total_packets: usize,
    error_categories: HashMap<String, Vec<u64>>,
}

impl StressResult {
    fn new() -> Self {
        Self {
            pass: 0,
            fail: 0,
            gen_fail: 0,
            total_packets: 0,
            error_categories: HashMap::new(),
        }
    }
}

/// Core stress test logic, parameterized by a recipe builder function.
fn run_stress_test(label: &str, recipe_fn: fn(usize, u64) -> Recipe, threshold: f64) {
    let Some(db) = load_real_db() else {
        return;
    };
    if !have_clang() {
        eprintln!("Skipping: hexagon-clang not found");
        return;
    }

    let iterations: usize = std::env::var("STRESS_ITERATIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);
    let packets_per_prog: usize = std::env::var("STRESS_PACKETS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);
    let num_threads: usize = std::env::var("STRESS_THREADS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let seed_base: u64 = std::env::var("STRESS_SEED_BASE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    eprintln!(
        "{}: {} iterations × {} packets = {} total packets, {} threads",
        label,
        iterations,
        packets_per_prog,
        iterations * packets_per_prog,
        num_threads
    );

    let db = Arc::new(db);
    let result = Arc::new(Mutex::new(StressResult::new()));
    let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    std::thread::scope(|scope| {
        let mut handles = Vec::new();

        for thread_id in 0..num_threads {
            let db = Arc::clone(&db);
            let result = Arc::clone(&result);
            let completed = Arc::clone(&completed);

            let handle = scope.spawn(move || {
                let gen = ProgramGenerator::new(&db);
                let tmp = tempfile::tempdir().unwrap();

                let mut local_pass = 0usize;
                let mut local_fail = 0usize;
                let mut local_gen_fail = 0usize;
                let mut local_packets = 0usize;
                let mut local_errors: HashMap<String, Vec<u64>> = HashMap::new();

                // Each thread handles its slice of seeds
                let mut seed_idx = thread_id;
                while seed_idx < iterations {
                    let seed = seed_base + seed_idx as u64;

                    let recipe = recipe_fn(packets_per_prog, seed);

                    let program = match gen.generate(&recipe) {
                        Ok(p) => p,
                        Err(e) => {
                            eprintln!("Seed {} generation failed: {}", seed, e);
                            local_gen_fail += 1;
                            seed_idx += num_threads;
                            continue;
                        }
                    };

                    let asm_path = tmp.path().join(format!("test_{}.S", seed));
                    let elf_path = tmp.path().join(format!("test_{}.elf", seed));

                    std::fs::write(&asm_path, &program.assembly).unwrap();

                    let output = Command::new(HEXAGON_CLANG)
                        .args(["-O2", "-g", "-G0", "-mv73"])
                        .arg(&asm_path)
                        .arg("-o")
                        .arg(&elf_path)
                        .output()
                        .unwrap();

                    if output.status.success() {
                        local_pass += 1;
                        local_packets += packets_per_prog;
                        // Clean up to save disk space
                        let _ = std::fs::remove_file(&asm_path);
                        let _ = std::fs::remove_file(&elf_path);
                    } else {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        let category = categorize_error(&stderr);
                        local_errors.entry(category).or_default().push(seed);
                        local_fail += 1;
                        // Keep failing .S files for debugging
                    }

                    let done = completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if done.is_multiple_of(500) {
                        eprintln!(
                            "Progress: {} / {} ({:.1}%)",
                            done,
                            iterations,
                            done as f64 / iterations as f64 * 100.0
                        );
                    }

                    seed_idx += num_threads;
                }

                // Merge local results
                let mut r = result.lock().unwrap();
                r.pass += local_pass;
                r.fail += local_fail;
                r.gen_fail += local_gen_fail;
                r.total_packets += local_packets;
                for (cat, seeds) in local_errors {
                    r.error_categories.entry(cat).or_default().extend(seeds);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }
    });

    let r = result.lock().unwrap();
    let total = r.pass + r.fail + r.gen_fail;
    let fail_rate = if total > 0 {
        r.fail as f64 / total as f64 * 100.0
    } else {
        0.0
    };

    eprintln!("\n========== {} RESULTS ==========", label.to_uppercase());
    eprintln!("Total iterations:   {}", total);
    eprintln!("Passed:             {}", r.pass);
    eprintln!("Build failures:     {}", r.fail);
    eprintln!("Generation failures:{}", r.gen_fail);
    eprintln!("Failure rate:       {:.2}%", fail_rate);
    eprintln!("Packets compiled:   {}", r.total_packets);
    eprintln!();

    if !r.error_categories.is_empty() {
        eprintln!("Error categories:");
        let mut cats: Vec<_> = r.error_categories.iter().collect();
        cats.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
        for (cat, seeds) in &cats {
            eprintln!("  [{:>5} hits] {}", seeds.len(), cat);
            // Show first 5 failing seeds for reproduction
            let show: Vec<_> = seeds.iter().take(5).collect();
            eprintln!("    Example seeds: {:?}", show);
        }
    }

    eprintln!("=========================================\n");

    assert!(
        fail_rate < threshold,
        "Build failure rate {:.2}% exceeds {:.1}% threshold ({} / {} failed)",
        fail_rate,
        threshold,
        r.fail,
        total
    );
}

/// Large-scale compilation stress test.
///
/// Run with: cargo test --test stress -- --ignored --nocapture
///
/// Environment variables:
///   STRESS_ITERATIONS  - number of programs to generate (default: 10000)
///   STRESS_PACKETS     - packets per program (default: 100)
///   STRESS_THREADS     - number of threads (default: 8)
///   STRESS_SEED_BASE   - starting seed (default: 0)
#[test]
#[ignore]
fn stress_test_compilation() {
    run_stress_test(
        "Stress test",
        |packets, seed| Recipe {
            num_packets: packets,
            num_iterations: 1,
            seed,
            ..Recipe::default()
        },
        1.0,
    );
}

/// Stress test with load/store instructions enabled.
///
/// Run with: cargo test --test stress stress_test_mem_ops -- --ignored --nocapture
#[test]
#[ignore]
fn stress_test_mem_ops() {
    run_stress_test(
        "Stress test (mem ops)",
        |packets, seed| Recipe {
            num_packets: packets,
            num_iterations: 1,
            seed,
            synth: SynthSettings {
                allow_mem_ops: true,
                ..SynthSettings::default()
            },
            ..Recipe::default()
        },
        1.0,
    );
}
