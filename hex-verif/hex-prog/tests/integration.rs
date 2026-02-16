use std::path::Path;
use std::process::Command;

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

#[test]
fn test_generate_and_compile_single() {
    let Some(db) = load_real_db() else {
        return;
    };
    if !have_clang() {
        eprintln!("Skipping: hexagon-clang not found");
        return;
    }

    let gen = ProgramGenerator::new(&db);
    let recipe = Recipe {
        num_packets: 5,
        num_iterations: 2,
        seed: 42,
        ..Recipe::default()
    };

    let program = gen.generate(&recipe).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let asm_path = tmp.path().join("test.S");
    let elf_path = tmp.path().join("test.elf");

    std::fs::write(&asm_path, &program.assembly).unwrap();

    let output = Command::new(HEXAGON_CLANG)
        .args(["-O2", "-g", "-G0", "-mv73"])
        .arg(&asm_path)
        .arg("-o")
        .arg(&elf_path)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "hexagon-clang failed:\n{}", stderr);
    assert!(elf_path.exists(), "ELF file not produced");
}

#[test]
fn test_generate_and_compile_many_seeds() {
    let Some(db) = load_real_db() else {
        return;
    };
    if !have_clang() {
        eprintln!("Skipping: hexagon-clang not found");
        return;
    }

    let gen = ProgramGenerator::new(&db);
    let tmp = tempfile::tempdir().unwrap();

    let mut pass = 0;
    let mut fail = 0;

    for seed in 0..50 {
        let recipe = Recipe {
            num_packets: 10,
            num_iterations: 2,
            seed,
            ..Recipe::default()
        };

        let program = gen.generate(&recipe).unwrap();
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
            pass += 1;
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("Seed {} failed:\n{}", seed, stderr);
            fail += 1;
        }
    }

    eprintln!("Compile test: {} / 50 passed ({} failures)", pass, fail);
    assert!(
        pass >= 48,
        "Too many build failures: only {} / 50 passed",
        pass
    );
}

#[test]
fn test_debug_mem_ops_assembly() {
    let Some(db) = load_real_db() else {
        return;
    };
    if !have_clang() {
        eprintln!("Skipping: hexagon-clang not found");
        return;
    }

    let gen = ProgramGenerator::new(&db);

    let recipe = Recipe {
        num_packets: 10,
        num_iterations: 1,
        seed: 4,
        synth: SynthSettings {
            allow_mem_ops: true,
            ..SynthSettings::default()
        },
        ..Recipe::default()
    };

    let program = gen.generate(&recipe).unwrap();

    eprintln!("\n=== Generated instructions ===");
    for inst in &program.instructions {
        eprintln!("  {} [{}]: {}", inst.name, inst.itype, inst.asm_text);
    }

    let tmp = tempfile::tempdir().unwrap();
    let asm_path = tmp.path().join("test_memops.S");
    let elf_path = tmp.path().join("test_memops.elf");

    std::fs::write(&asm_path, &program.assembly).unwrap();

    let output = Command::new(HEXAGON_CLANG)
        .args(["-O2", "-g", "-G0", "-mv73"])
        .arg(&asm_path)
        .arg("-o")
        .arg(&elf_path)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "hexagon-clang failed for mem_ops:\n{}",
        stderr
    );
}
