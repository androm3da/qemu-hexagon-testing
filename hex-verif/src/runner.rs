use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use hex_dbg::port::allocate_port;
use hex_dbg::registers::RegisterState;
use hex_dbg::session::{Backend, SessionConfig};
use hex_instset::database::InstructionDb;
use hex_prog::recipe::{ExecutionMode, Recipe};
use hex_prog::template::ProgramGenerator;

use crate::comparison::{compare_states, format_diff};
use crate::coverage::{CoverageTracker, EncodingSpace};
use crate::reproducer::minimize_failure;
use crate::stats::Stats;

/// A mismatch recorded during the verification loop, to be minimized afterward.
struct DeferredMismatch {
    iter_dir: PathBuf,
    asm_content: String,
    hvx: bool,
    diff_text: String,
}

/// Execution mode for the verification loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Direct execution with stdout-based register dump (fast).
    Direct,
    /// GDB RSP-based execution with breakpoint register reads (slow, for debugging).
    Gdb,
}

impl std::fmt::Display for RunMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunMode::Direct => write!(f, "direct"),
            RunMode::Gdb => write!(f, "gdb"),
        }
    }
}

impl std::str::FromStr for RunMode {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "direct" => Ok(RunMode::Direct),
            "gdb" => Ok(RunMode::Gdb),
            _ => Err(format!(
                "unknown run mode '{}', expected 'direct' or 'gdb'",
                s
            )),
        }
    }
}

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

    pub fn hexagon_sim(&self) -> PathBuf {
        self.toolchain_root.join("Tools/bin/hexagon-sim")
    }
}

/// Configuration for a verification run.
#[derive(Debug, Clone)]
pub struct VerificationConfig {
    pub toolchain: ToolchainPaths,
    pub results_dir: PathBuf,
    pub num_iterations: usize,
    pub packets_per_test: usize,
    pub base_seed: u64,
    pub num_threads: usize,
    /// Template recipe with filter/synthesis settings. Seed is overridden per iteration.
    pub template_recipe: Recipe,
    /// Maximum time to spend on ddmin minimization per mismatch.
    pub ddmin_timeout: Duration,
    /// Execution mode (direct stdout vs GDB RSP).
    pub run_mode: RunMode,
}

/// Precompile the dump helper C file into an object file.
/// Returns the path to the compiled .o file.
pub fn precompile_helper(config: &VerificationConfig) -> Result<PathBuf> {
    let helper_c = config.results_dir.join("dump_helper.c");
    let helper_o = config.results_dir.join("dump_helper.o");
    std::fs::write(&helper_c, hex_prog::template::DUMP_HELPER_C)?;

    let clang = config.toolchain.hexagon_clang();
    let mut cmd = Command::new(&clang);
    cmd.args([
        "-O0",
        "-G0",
        "-c",
        &format!("-m{}", config.toolchain.isa_version),
    ]);
    if config.template_recipe.hvx {
        cmd.arg("-mhvx");
    }
    let output = cmd
        .arg(&helper_c)
        .arg("-o")
        .arg(&helper_o)
        .output()
        .with_context(|| format!("Failed to precompile helper: {}", clang.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Helper precompilation failed: {}", stderr);
    }
    Ok(helper_o)
}

/// Precompile the pageable helper C file into an object file.
/// Returns the path to the compiled .o file.
pub fn precompile_pageable_helper(config: &VerificationConfig) -> Result<PathBuf> {
    let helper_c = config.results_dir.join("pageable_helper.c");
    let helper_o = config.results_dir.join("pageable_helper.o");
    std::fs::write(&helper_c, hex_prog::template::PAGEABLE_HELPER_C)?;

    let clang = config.toolchain.hexagon_clang();
    let mut cmd = Command::new(&clang);
    cmd.args([
        "-O0",
        "-G0",
        "-c",
        &format!("-m{}", config.toolchain.isa_version),
    ]);
    if config.template_recipe.hvx {
        cmd.arg("-mhvx");
    }
    let output = cmd
        .arg(&helper_c)
        .arg("-o")
        .arg(&helper_o)
        .output()
        .with_context(|| format!("Failed to precompile pageable helper: {}", clang.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Pageable helper precompilation failed: {}", stderr);
    }
    Ok(helper_o)
}

/// Run the full verification loop.
pub fn run_verification(config: &VerificationConfig, db: &InstructionDb) -> Result<()> {
    std::fs::create_dir_all(&config.results_dir)
        .with_context(|| format!("Failed to create {}", config.results_dir.display()))?;

    // Precompile the C helper(s) once for all iterations
    let helper_obj = precompile_helper(config)?;
    let pageable_obj = if config.template_recipe.synth.allow_pageable {
        Some(precompile_pageable_helper(config)?)
    } else {
        None
    };

    let stats = Arc::new(Stats::new());
    let coverage = Arc::new(CoverageTracker::new());
    let deferred: Arc<Mutex<Vec<DeferredMismatch>>> = Arc::new(Mutex::new(Vec::new()));
    let full_space = EncodingSpace::compute(db);
    let db = Arc::new(db.clone());
    let config = Arc::new(config.clone());
    let helper_obj = Arc::new(helper_obj);
    let pageable_obj: Arc<Option<PathBuf>> = Arc::new(pageable_obj);

    std::thread::scope(|scope| {
        let mut handles = Vec::new();

        for thread_id in 0..config.num_threads {
            let stats = Arc::clone(&stats);
            let coverage = Arc::clone(&coverage);
            let deferred = Arc::clone(&deferred);
            let db = Arc::clone(&db);
            let config = Arc::clone(&config);
            let helper_obj = Arc::clone(&helper_obj);
            let pageable_obj = Arc::clone(&pageable_obj);

            let handle = scope.spawn(move || {
                let iters_per_thread = config.num_iterations / config.num_threads;
                let extra = if thread_id < config.num_iterations % config.num_threads {
                    1
                } else {
                    0
                };

                for local_i in 0..(iters_per_thread + extra) {
                    let global_i = thread_id * iters_per_thread + local_i;
                    if let Err(e) = run_single_iteration(
                        &config,
                        &db,
                        &stats,
                        &coverage,
                        &deferred,
                        global_i,
                        &helper_obj,
                        pageable_obj.as_deref(),
                    ) {
                        eprintln!("Iteration {} failed: {:#}", global_i, e);
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

    // Compute synthesizable encoding space from candidate names
    let synth_space = match coverage.candidate_names() {
        Some(names) => EncodingSpace::compute_from_names(&db, &names),
        None => EncodingSpace::compute_from_names(&db, &[]),
    };

    // Print verification results
    println!("{}", stats.report());
    println!("{}", coverage.report(&full_space, &synth_space));

    // Minimize deferred mismatches after the main loop
    let mismatches = deferred.lock().unwrap();
    if !mismatches.is_empty() && config.ddmin_timeout > Duration::ZERO {
        eprintln!(
            "\nMinimizing {} mismatch(es) (timeout {}s each)...",
            mismatches.len(),
            config.ddmin_timeout.as_secs(),
        );
        for (i, m) in mismatches.iter().enumerate() {
            eprint!(
                "  [{}/{}] {} ... ",
                i + 1,
                mismatches.len(),
                m.iter_dir.display()
            );
            match minimize_failure(
                &config.toolchain,
                &m.iter_dir,
                &m.asm_content,
                &config.toolchain.isa_version,
                config.ddmin_timeout,
                config.run_mode,
                m.hvx,
                &m.diff_text,
            ) {
                Ok(min_result) => {
                    eprintln!(
                        "{} -> {} packets\n    {}",
                        min_result.original_packets,
                        min_result.minimized_packets,
                        min_result
                            .diff_text
                            .lines()
                            .collect::<Vec<_>>()
                            .join("\n    "),
                    );
                    let _ =
                        std::fs::write(m.iter_dir.join("minimized.S"), &min_result.minimized_asm);
                }
                Err(e) => {
                    eprintln!("failed: {}", e);
                }
            }
        }
    }

    // Clean up results directory if no mismatches were found
    if mismatches.is_empty() {
        let _ = std::fs::remove_dir_all(&config.results_dir);
    }

    Ok(())
}

/// RAII guard that removes an iteration directory on drop unless explicitly kept.
/// Ensures cleanup on all exit paths: success, build failure, execution error.
struct IterDirGuard {
    path: PathBuf,
    keep: bool,
}

impl IterDirGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, keep: false }
    }

    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for IterDirGuard {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Run a single verification iteration.
#[allow(clippy::too_many_arguments)]
fn run_single_iteration(
    config: &VerificationConfig,
    db: &InstructionDb,
    stats: &Stats,
    coverage: &CoverageTracker,
    deferred: &Mutex<Vec<DeferredMismatch>>,
    iteration: usize,
    helper_obj: &Path,
    pageable_obj: Option<&Path>,
) -> Result<()> {
    let iter_dir = config.results_dir.join(format!("iter_{}", iteration));
    std::fs::create_dir_all(&iter_dir)?;
    let mut guard = IterDirGuard::new(iter_dir.clone());

    let seed = config.base_seed.wrapping_add(iteration as u64);

    // Generate test program from template recipe with per-iteration seed
    let mut recipe = config.template_recipe.clone();
    recipe.seed = seed;
    recipe.num_packets = config.packets_per_test;
    recipe.isa_version = config.toolchain.isa_version.clone();
    recipe.execution_mode = match config.run_mode {
        RunMode::Direct => ExecutionMode::Direct,
        RunMode::Gdb => ExecutionMode::Gdb,
    };

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

    // Build (assembly + precompiled helper .o + optional pageable helper .o)
    let build_ok = build_test(
        config,
        &recipe,
        &asm_path,
        helper_obj,
        pageable_obj,
        &elf_path,
    )?;
    if !build_ok {
        stats.record_build_failure();
        // guard drops here → removes iter_dir
        return Ok(());
    }

    // Run on both backends and collect register states
    let (ref_states, test_states) = match config.run_mode {
        RunMode::Direct => {
            // Direct execution: run sim and QEMU in parallel, parse stdout
            let sim_path = config.toolchain.hexagon_sim();
            let qemu_path = config.toolchain.qemu_path.clone();
            let elf_clone = elf_path.clone();
            let hvx = recipe.hvx;

            std::thread::scope(|s| {
                let ref_handle = s.spawn(|| run_direct_sim_pub(&sim_path, &elf_clone, hvx));
                let test_handle = s.spawn(|| run_direct_qemu_pub(&qemu_path, &elf_path, hvx));
                let ref_result = ref_handle.join().expect("sim thread panicked");
                let test_result = test_handle.join().expect("qemu thread panicked");
                Ok::<_, anyhow::Error>((ref_result?, test_result?))
            })?
        }
        RunMode::Gdb => {
            // GDB RSP: sequential execution with breakpoint-based register reads
            let breakpoint_addrs = resolve_breakpoint_addrs(&elf_path, config.run_mode)?;

            let ref_states = run_on_sim(
                &config.toolchain.hexagon_sim(),
                &elf_path,
                &breakpoint_addrs,
            )?;
            let test_states =
                run_on_qemu(&config.toolchain.qemu_path, &elf_path, &breakpoint_addrs)?;
            (ref_states, test_states)
        }
    };

    // Compare
    let mut diff_texts = Vec::new();
    let min_len = ref_states.len().min(test_states.len());
    for i in 0..min_len {
        let result = compare_states(&ref_states[i], &test_states[i]);
        if !result.matches {
            let diff_text = format_diff(&result);
            eprintln!(
                "MISMATCH at breakpoint {} in iteration {}:\n{}",
                i, iteration, diff_text
            );
            std::fs::write(iter_dir.join(format!("diff_{}.txt", i)), &diff_text)?;
            diff_texts.push(diff_text);
        }
    }

    if !diff_texts.is_empty() {
        // Write diagnostic files only on mismatch
        std::fs::write(
            iter_dir.join("ref_output.txt"),
            format_register_states(&ref_states),
        )?;
        std::fs::write(
            iter_dir.join("test_output.txt"),
            format_register_states(&test_states),
        )?;

        // Defer minimization to after the main loop — keep the iter dir
        guard.keep();
        deferred.lock().unwrap().push(DeferredMismatch {
            iter_dir: iter_dir.clone(),
            asm_content: asm_content.clone(),
            hvx: recipe.hvx,
            diff_text: diff_texts.join("\n"),
        });
        stats.record_mismatch(recipe.num_packets, recipe.num_packets * 3);
    } else {
        stats.record_success(recipe.num_packets, recipe.num_packets * 3);
        // guard drops at function exit → removes iter_dir
    }

    Ok(())
}

/// Build test assembly + precompiled helper object(s) into an ELF.
fn build_test(
    config: &VerificationConfig,
    recipe: &Recipe,
    asm_path: &Path,
    helper_obj: &Path,
    pageable_obj: Option<&Path>,
    elf_path: &Path,
) -> Result<bool> {
    let clang = config.toolchain.hexagon_clang();
    let mut cmd = Command::new(&clang);
    cmd.args(["-O0", "-G0", &format!("-m{}", config.toolchain.isa_version)]);
    if recipe.hvx {
        cmd.arg("-mhvx");
    }
    cmd.arg(asm_path).arg(helper_obj);
    if let Some(pageable) = pageable_obj {
        cmd.arg(pageable);
    }
    let output = cmd
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

/// Timeout in seconds for simulator/emulator sessions.
const RUN_TIMEOUT_SECS: u64 = 10;

/// Breakpoint symbol names for a given execution mode.
fn breakpoint_symbols(mode: RunMode) -> &'static [&'static str] {
    match mode {
        RunMode::Gdb => &["check_point"],
        RunMode::Direct => &["steps", "test_end"],
    }
}

/// Resolve breakpoint symbol names to addresses from the ELF.
pub fn resolve_breakpoint_addrs(elf_path: &Path, mode: RunMode) -> Result<Vec<u64>> {
    let symbols = hex_dbg::elf::resolve_symbols(elf_path, breakpoint_symbols(mode))?;
    Ok(symbols.into_iter().map(|(_, addr)| addr).collect())
}

/// Run a Hexagon ELF on hexagon-sim via GDB RSP.
///
/// 1. Allocate a port and start hexagon-sim with `--gdbserv <port>`
/// 2. Connect hex-dbg RSP client with retries
/// 3. Set breakpoints, run, and collect register states
/// 4. Clean up the sim process
pub fn run_on_sim(sim: &Path, elf: &Path, breakpoint_addrs: &[u64]) -> Result<Vec<RegisterState>> {
    let port_guard = allocate_port()?;
    let port = port_guard.port;
    drop(port_guard); // Release so sim can bind

    let mut sim_proc = start_sim(sim, elf, port)?;

    let config = SessionConfig {
        backend: Backend::HexagonSim,
        address: format!("127.0.0.1:{}", port),
        connect_timeout: Duration::from_secs(5),
        max_retries: 20,
        retry_delay: Duration::from_millis(100),
        session_timeout: Duration::from_secs(RUN_TIMEOUT_SECS),
    };

    let result = hex_dbg::session::run_session(&config, breakpoint_addrs);

    // Always clean up sim
    let _ = sim_proc.kill();
    let _ = sim_proc.wait();

    Ok(result?.states)
}

/// Run a Hexagon ELF on QEMU via GDB RSP.
///
/// 1. Allocate a port and start QEMU with `-gdb tcp::<port> -S`
/// 2. Connect hex-dbg RSP client with retries
/// 3. Set breakpoints, run, and collect register states
/// 4. Clean up the QEMU process
pub fn run_on_qemu(
    qemu: &Path,
    elf: &Path,
    breakpoint_addrs: &[u64],
) -> Result<Vec<RegisterState>> {
    let port_guard = allocate_port()?;
    let port = port_guard.port;
    drop(port_guard); // Release so QEMU can bind

    let mut qemu_proc = start_qemu(qemu, elf, port)?;

    let config = SessionConfig {
        backend: Backend::Qemu,
        address: format!("127.0.0.1:{}", port),
        connect_timeout: Duration::from_secs(5),
        max_retries: 20,
        retry_delay: Duration::from_millis(100),
        session_timeout: Duration::from_secs(RUN_TIMEOUT_SECS),
    };

    let result = hex_dbg::session::run_session(&config, breakpoint_addrs);

    // Always clean up QEMU
    let _ = qemu_proc.kill();
    let _ = qemu_proc.wait();

    Ok(result?.states)
}

/// Start hexagon-sim in the background with GDB server.
fn start_sim(sim: &Path, elf: &Path, gdb_port: u16) -> Result<Child> {
    Command::new(sim)
        .arg("--gdbserv")
        .arg(gdb_port.to_string())
        .arg("--quiet")
        .arg(elf)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to start hexagon-sim: {}", sim.display()))
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

/// Run a command with a timeout, collecting stdout and stderr.
///
/// Spawns the command, reads stdout/stderr in background threads, and polls
/// for completion. If the process doesn't exit within `timeout`, it is killed
/// and an error is returned.
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Result<Output> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn process")?;

    let mut stdout_pipe = child.stdout.take().expect("stdout piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr piped");

    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stdout_pipe.read_to_end(&mut buf).ok();
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stderr_pipe.read_to_end(&mut buf).ok();
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("Process timed out after {}s", timeout.as_secs());
            }
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Run a Hexagon ELF directly on hexagon-sim and parse register state from stdout.
pub fn run_direct_sim_pub(sim: &Path, elf: &Path, hvx: bool) -> Result<Vec<RegisterState>> {
    let timeout = Duration::from_secs(RUN_TIMEOUT_SECS);
    let output = run_with_timeout(Command::new(sim).arg("--quiet").arg(elf), timeout)
        .with_context(|| format!("hexagon-sim execution failed: {}", sim.display()))?;

    if hvx {
        crate::stdout_parser::parse_scalar_and_hvx_dump(&output.stdout)
    } else {
        crate::stdout_parser::parse_scalar_dump(&output.stdout)
    }
}

/// Run a Hexagon ELF directly on QEMU and parse register state from stdout.
pub fn run_direct_qemu_pub(qemu: &Path, elf: &Path, hvx: bool) -> Result<Vec<RegisterState>> {
    let timeout = Duration::from_secs(RUN_TIMEOUT_SECS);
    let output = run_with_timeout(
        Command::new(qemu)
            .arg("-kernel")
            .arg(elf)
            .arg("-nographic")
            .arg("-display")
            .arg("none")
            .arg("-monitor")
            .arg("none")
            .arg("-serial")
            .arg("none"),
        timeout,
    )
    .with_context(|| format!("QEMU execution failed: {}", qemu.display()))?;

    if hvx {
        crate::stdout_parser::parse_scalar_and_hvx_dump(&output.stdout)
    } else {
        crate::stdout_parser::parse_scalar_dump(&output.stdout)
    }
}

/// Format register states as text for diagnostic output.
fn format_register_states(states: &[RegisterState]) -> String {
    let mut out = String::new();
    for (i, state) in states.iter().enumerate() {
        out.push_str(&format!("=== Breakpoint {} ===\n", i));
        for (name, value) in &state.registers {
            out.push_str(&format!("  {} = {}\n", name, value));
        }
        out.push('\n');
    }
    out
}
