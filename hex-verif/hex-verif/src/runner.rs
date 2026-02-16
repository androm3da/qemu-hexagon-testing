use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use hex_instset::database::InstructionDb;
use hex_prog::recipe::Recipe;
use hex_prog::template::ProgramGenerator;

use crate::comparison::{compare_states, format_diff};
use crate::coverage::{CoverageTracker, EncodingSpace};
use crate::lldb_script::{generate_qemu_script, generate_sim_script, parse_lldb_output};
use crate::reproducer::minimize_failure;
use crate::stats::Stats;

/// Paths to toolchain binaries.
#[derive(Debug, Clone)]
pub struct ToolchainPaths {
    pub toolchain_root: PathBuf,
    pub qemu_path: PathBuf,
    pub isa_version: String,
}

impl ToolchainPaths {
    pub fn hexagon_clang(&self) -> PathBuf {
        self.toolchain_root.join("Tools/bin/hexagon-clang")
    }

    pub fn hexagon_lldb(&self) -> PathBuf {
        self.toolchain_root.join("Tools/bin/hexagon-lldb")
    }
}

/// Configuration for a verification run.
#[derive(Debug, Clone)]
pub struct VerificationConfig {
    pub toolchain: ToolchainPaths,
    pub results_dir: PathBuf,
    /// Maximum number of iterations. `None` means unlimited (run until deadline or Ctrl-C).
    pub max_iterations: Option<usize>,
    /// Deadline after which no new iterations start. `None` means no time limit.
    pub deadline: Option<Instant>,
    pub packets_per_test: usize,
    pub base_seed: u64,
    pub num_threads: usize,
    /// Template recipe with filter/synthesis settings. Seed is overridden per iteration.
    pub template_recipe: Recipe,
}

/// Run the full verification loop.
///
/// Threads claim iterations via an atomic counter and run until one of:
/// - The iteration limit is reached (`max_iterations`)
/// - The time deadline expires (`deadline`)
/// - The shutdown flag is set (Ctrl-C)
///
/// In-progress iterations always complete before the thread exits.
/// Reports are always printed, even on early shutdown.
pub fn run_verification(
    config: &VerificationConfig,
    db: &InstructionDb,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    std::fs::create_dir_all(&config.results_dir)
        .with_context(|| format!("Failed to create {}", config.results_dir.display()))?;

    let stats = Arc::new(Stats::new());
    let coverage = Arc::new(CoverageTracker::new());
    let full_space = EncodingSpace::compute(db);
    let db = Arc::new(db.clone());
    let config = Arc::new(config.clone());
    let next_iteration = Arc::new(AtomicUsize::new(0));

    std::thread::scope(|scope| {
        let mut handles = Vec::new();

        for _thread_id in 0..config.num_threads {
            let stats = Arc::clone(&stats);
            let coverage = Arc::clone(&coverage);
            let db = Arc::clone(&db);
            let config = Arc::clone(&config);
            let shutdown = Arc::clone(&shutdown);
            let next_iteration = Arc::clone(&next_iteration);

            let handle = scope.spawn(move || {
                loop {
                    // Check shutdown flag
                    if shutdown.load(Ordering::Relaxed) {
                        break;
                    }

                    // Check deadline
                    if let Some(deadline) = config.deadline {
                        if Instant::now() >= deadline {
                            break;
                        }
                    }

                    // Claim next iteration number
                    let iter_num = next_iteration.fetch_add(1, Ordering::Relaxed);

                    // Check iteration limit
                    if let Some(max) = config.max_iterations {
                        if iter_num >= max {
                            break;
                        }
                    }

                    // Run the iteration (always completes even if shutdown fires mid-run)
                    if let Err(e) = run_single_iteration(&config, &db, &stats, &coverage, iter_num)
                    {
                        eprintln!("Iteration {} failed: {:#}", iter_num, e);
                        stats.record_build_failure();
                    }
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("Thread panicked");
        }
    });

    let was_interrupted = shutdown.load(Ordering::Relaxed);

    // Compute synthesizable encoding space from candidate names
    let synth_space = match coverage.candidate_names() {
        Some(names) => EncodingSpace::compute_from_names(&db, &names),
        None => EncodingSpace::compute_from_names(&db, &[]),
    };

    // Build the full report
    let stats_report = stats.report();
    let coverage_report = coverage.report(&full_space, &synth_space);

    let mut summary = String::new();
    if was_interrupted {
        summary.push_str("(interrupted by Ctrl-C)\n\n");
    }
    summary.push_str(&stats_report);
    summary.push('\n');
    summary.push_str(&coverage_report);
    summary.push('\n');

    // Print to stdout
    println!("{}", summary);

    // Write summary file to results directory
    let summary_path = config.results_dir.join("summary.txt");
    if let Err(e) = std::fs::write(&summary_path, &summary) {
        eprintln!("Warning: failed to write {}: {}", summary_path.display(), e);
    }

    Ok(())
}

/// Run a single verification iteration.
fn run_single_iteration(
    config: &VerificationConfig,
    db: &InstructionDb,
    stats: &Stats,
    coverage: &CoverageTracker,
    iteration: usize,
) -> Result<()> {
    let iter_dir = config.results_dir.join(format!("iter_{}", iteration));
    std::fs::create_dir_all(&iter_dir)?;

    let seed = config.base_seed.wrapping_add(iteration as u64);

    // Generate test program from template recipe with per-iteration seed
    let mut recipe = config.template_recipe.clone();
    recipe.seed = seed;
    recipe.num_packets = config.packets_per_test;
    recipe.isa_version = config.toolchain.isa_version.clone();

    let gen = ProgramGenerator::new(db);
    let program = gen
        .generate(&recipe)
        .context("Failed to generate test program")?;

    // Record coverage data
    let gi: Vec<crate::coverage::GeneratedInstruction> = program
        .instructions
        .iter()
        .map(|i| crate::coverage::GeneratedInstruction {
            name: i.name.clone(),
            itype: i.itype.clone(),
            asm_text: i.asm_text.clone(),
        })
        .collect();
    coverage.record(&gi);
    coverage.set_candidate_names(program.candidate_names);

    let asm_content = &program.assembly;
    let asm_path = iter_dir.join("test.S");
    let elf_path = iter_dir.join("test.elf");
    std::fs::write(&asm_path, asm_content)?;

    // Build
    let build_ok = build_test(config, &recipe, &asm_path, &elf_path)?;
    if !build_ok {
        stats.record_build_failure();
        return Ok(());
    }

    // Run reference (hexagon-sim via hexagon-lldb)
    let sim_script = generate_sim_script(&["steps", "test_end"]);
    let sim_script_path = iter_dir.join("sim_debug.lldb");
    std::fs::write(&sim_script_path, &sim_script)?;

    let ref_output = run_on_sim(
        &config.toolchain.hexagon_lldb(),
        &sim_script_path,
        &elf_path,
    )?;
    std::fs::write(iter_dir.join("ref_output.txt"), &ref_output)?;

    // Run test (QEMU via hexagon-lldb + gdb-remote)
    let gdb_port = find_free_port()?;
    let qemu_script = generate_qemu_script(&["steps", "test_end"], gdb_port);
    let qemu_script_path = iter_dir.join("qemu_debug.lldb");
    std::fs::write(&qemu_script_path, &qemu_script)?;

    let test_output = run_on_qemu(
        &config.toolchain.hexagon_lldb(),
        &config.toolchain.qemu_path,
        &qemu_script_path,
        &elf_path,
        gdb_port,
    )?;
    std::fs::write(iter_dir.join("test_output.txt"), &test_output)?;

    // Parse and compare
    let ref_states = parse_lldb_output(&ref_output);
    let test_states = parse_lldb_output(&test_output);

    let mut any_mismatch = false;
    let min_len = ref_states.len().min(test_states.len());
    for i in 0..min_len {
        let result = compare_states(&ref_states[i], &test_states[i]);
        if !result.matches {
            any_mismatch = true;
            let diff_text = format_diff(&result);
            eprintln!(
                "MISMATCH at breakpoint {} in iteration {}:\n{}",
                i, iteration, diff_text
            );
            std::fs::write(iter_dir.join(format!("diff_{}.txt", i)), &diff_text)?;

            // Attempt minimization
            match minimize_failure(
                &config.toolchain,
                &iter_dir,
                asm_content,
                &config.toolchain.isa_version,
            ) {
                Ok(min_result) => {
                    eprintln!(
                        "  Minimized: {} -> {} packets",
                        min_result.original_packets, min_result.minimized_packets
                    );
                    std::fs::write(iter_dir.join("minimized.S"), &min_result.minimized_asm)?;
                }
                Err(e) => {
                    eprintln!("  Minimization failed: {}", e);
                }
            }
        }
    }

    if any_mismatch {
        stats.record_mismatch(recipe.num_packets, recipe.num_packets * 3);
    } else {
        stats.record_success(recipe.num_packets, recipe.num_packets * 3);
        // Clean up passing iterations to save disk space
        let _ = std::fs::remove_dir_all(&iter_dir);
    }

    Ok(())
}

/// Build a test assembly file into an ELF.
fn build_test(
    config: &VerificationConfig,
    recipe: &Recipe,
    asm_path: &Path,
    elf_path: &Path,
) -> Result<bool> {
    let clang = config.toolchain.hexagon_clang();
    let mut cmd = Command::new(&clang);
    cmd.args([
        "-O2",
        "-g",
        "-G0",
        &format!("-m{}", config.toolchain.isa_version),
    ]);
    if recipe.hvx {
        cmd.arg("-mhvx");
    }
    let output = cmd
        .arg(asm_path)
        .arg("-o")
        .arg(elf_path)
        .output()
        .with_context(|| format!("Failed to run {}", clang.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("Build failed: {}", stderr);
        return Ok(false);
    }

    Ok(true)
}

/// Timeout in seconds for simulator/emulator runs.
const RUN_TIMEOUT_SECS: u64 = 60;

/// Run a Hexagon ELF on hexagon-sim via hexagon-lldb.
///
/// hexagon-lldb natively uses hexagon-sim as its execution backend,
/// so we pass the ELF directly: `hexagon-lldb -b -s <script> <elf>`
pub fn run_on_sim(lldb: &Path, script: &Path, elf: &Path) -> Result<String> {
    let mut child = Command::new(lldb)
        .arg("-b")
        .arg("-s")
        .arg(script)
        .arg(elf)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to run {}", lldb.display()))?;

    let timeout = std::time::Duration::from_secs(RUN_TIMEOUT_SECS);
    match wait_with_timeout(&mut child, timeout) {
        Ok(output) => Ok(output),
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("hexagon-sim timed out after {}s", RUN_TIMEOUT_SECS);
        }
    }
}

/// Run a Hexagon ELF on QEMU via hexagon-lldb + gdb-remote.
///
/// 1. Start QEMU with `-kernel <elf> -gdb tcp::<port> -S`
/// 2. Connect hexagon-lldb via gdb-remote in the LLDB script
/// 3. Collect output and kill QEMU
pub fn run_on_qemu(
    lldb: &Path,
    qemu: &Path,
    script: &Path,
    elf: &Path,
    gdb_port: u16,
) -> Result<String> {
    // Start QEMU in the background with GDB server
    let mut qemu_proc = start_qemu(qemu, elf, gdb_port)?;

    // Give QEMU a moment to start up
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Run hexagon-lldb connecting to QEMU's GDB server
    let mut lldb_child = Command::new(lldb)
        .arg("-b")
        .arg("-s")
        .arg(script)
        .arg(elf)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to run {} for QEMU", lldb.display()))?;

    let timeout = std::time::Duration::from_secs(RUN_TIMEOUT_SECS);
    let result = wait_with_timeout(&mut lldb_child, timeout);

    // Always clean up QEMU
    let _ = qemu_proc.kill();
    let _ = qemu_proc.wait();

    match result {
        Ok(output) => Ok(output),
        Err(_) => {
            let _ = lldb_child.kill();
            let _ = lldb_child.wait();
            anyhow::bail!("QEMU timed out after {}s", RUN_TIMEOUT_SECS);
        }
    }
}

/// Wait for a child process with a timeout, returning combined stdout+stderr.
fn wait_with_timeout(child: &mut Child, timeout: std::time::Duration) -> Result<String> {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait()? {
            Some(_status) => {
                let stdout = child
                    .stdout
                    .take()
                    .map(|mut s| {
                        let mut buf = String::new();
                        std::io::Read::read_to_string(&mut s, &mut buf).ok();
                        buf
                    })
                    .unwrap_or_default();
                let stderr = child
                    .stderr
                    .take()
                    .map(|mut s| {
                        let mut buf = String::new();
                        std::io::Read::read_to_string(&mut s, &mut buf).ok();
                        buf
                    })
                    .unwrap_or_default();
                return Ok(format!("{}\n{}", stdout, stderr));
            }
            None => {
                if start.elapsed() > timeout {
                    anyhow::bail!("Timed out");
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

/// Start QEMU in the background with GDB server.
fn start_qemu(qemu: &Path, elf: &Path, gdb_port: u16) -> Result<Child> {
    Command::new(qemu)
        .arg("-kernel")
        .arg(elf)
        .arg("-gdb")
        .arg(format!("tcp::{}", gdb_port))
        .arg("-S")
        .arg("-nographic")
        .arg("-display")
        .arg("none")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to start QEMU: {}", qemu.display()))
}

/// Find a free TCP port for the GDB server.
pub fn find_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("Failed to bind to find free port")?;
    let port = listener.local_addr()?.port();
    Ok(port)
}
