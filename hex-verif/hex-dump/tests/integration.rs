use std::path::Path;

use hex_dump::parser::parse_tablegen;

const TABLEGEN_PATH: &str =
    "/local/mnt/workspace/upstream/llvm-project/llvm/lib/Target/Hexagon/HexagonDepInstrInfo.td";

fn load_real_tablegen() -> Option<String> {
    let path = Path::new(TABLEGEN_PATH);
    if path.exists() {
        Some(std::fs::read_to_string(path).unwrap())
    } else {
        eprintln!("Skipping integration test: {} not found", TABLEGEN_PATH);
        None
    }
}

#[test]
fn test_parse_real_tablegen_count() {
    let Some(content) = load_real_tablegen() else {
        return;
    };
    let dump = parse_tablegen(&content).unwrap();
    // Should have ~2970 defs total
    assert!(
        dump.total_parsed >= 2900,
        "Expected ~2970 total defs, got {}",
        dump.total_parsed
    );
    // Should have ~1500-2500 real instructions after filtering
    assert!(
        dump.instructions.len() >= 1500,
        "Expected >= 1500 instructions, got {}",
        dump.instructions.len()
    );
    assert!(
        dump.instructions.len() <= 2500,
        "Expected <= 2500 instructions, got {}",
        dump.instructions.len()
    );
}

#[test]
fn test_a2_add_attributes() {
    let Some(content) = load_real_tablegen() else {
        return;
    };
    let dump = parse_tablegen(&content).unwrap();
    let a2_add = dump
        .instructions
        .iter()
        .find(|i| i.name == "A2_add")
        .expect("A2_add should exist");

    assert_eq!(a2_add.itype, "TypeALU32_3op");
    assert!(a2_add.has_new_value);
    assert_eq!(a2_add.op_new_value, Some(0));
    assert!(a2_add.is_commutable);
    assert!(a2_add.is_predicable);
    assert!(!a2_add.is_pseudo);
    assert!(!a2_add.is_code_gen_only);
    assert!(!a2_add.is_solo);
    assert_eq!(a2_add.outs.len(), 1);
    assert_eq!(a2_add.ins.len(), 2);
    assert_eq!(a2_add.outs[0].reg_class.as_deref(), Some("IntRegs"));
    assert_eq!(a2_add.ins[0].reg_class.as_deref(), Some("IntRegs"));
    assert_eq!(a2_add.ins[1].reg_class.as_deref(), Some("IntRegs"));
}

#[test]
fn test_load_store_instructions_present() {
    let Some(content) = load_real_tablegen() else {
        return;
    };
    let dump = parse_tablegen(&content).unwrap();

    let loads: Vec<_> = dump
        .instructions
        .iter()
        .filter(|i| i.itype == "TypeLD")
        .collect();
    assert!(
        loads.len() >= 100,
        "Expected >= 100 load instructions, got {}",
        loads.len()
    );

    let stores: Vec<_> = dump
        .instructions
        .iter()
        .filter(|i| i.itype == "TypeST")
        .collect();
    assert!(
        stores.len() >= 100,
        "Expected >= 100 store instructions, got {}",
        stores.len()
    );
}

#[test]
fn test_cvi_instructions_present() {
    let Some(content) = load_real_tablegen() else {
        return;
    };
    let dump = parse_tablegen(&content).unwrap();

    let cvi: Vec<_> = dump.instructions.iter().filter(|i| i.is_cvi).collect();
    assert!(
        cvi.len() >= 300,
        "Expected >= 300 CVI instructions, got {}",
        cvi.len()
    );

    // Most CVI instructions have itype starting with TypeCVI,
    // but a few (like V6_extractw) use TypeLD/TypeST for scalar extract/insert.
    let cvi_typed: Vec<_> = cvi
        .iter()
        .filter(|i| i.itype.starts_with("TypeCVI"))
        .collect();
    assert!(
        cvi_typed.len() >= 280,
        "Expected >= 280 TypeCVI_* instructions, got {}",
        cvi_typed.len()
    );
}

#[test]
fn test_no_pseudo_in_output() {
    let Some(content) = load_real_tablegen() else {
        return;
    };
    let dump = parse_tablegen(&content).unwrap();

    for insn in &dump.instructions {
        assert!(
            !insn.is_pseudo,
            "{} should have been filtered (isPseudo)",
            insn.name
        );
        assert!(
            !insn.is_code_gen_only,
            "{} should have been filtered (isCodeGenOnly)",
            insn.name
        );
        assert!(
            !matches!(
                insn.itype.as_str(),
                "TypeMAPPING"
                    | "TypePSEUDO"
                    | "TypeSUBINSN"
                    | "TypeDUPLEX"
                    | "TypeENDLOOP"
                    | "TypeEXTENDER"
            ),
            "{} should have been filtered (itype={})",
            insn.name,
            insn.itype
        );
    }
}

#[test]
fn test_predicated_instructions() {
    let Some(content) = load_real_tablegen() else {
        return;
    };
    let dump = parse_tablegen(&content).unwrap();

    let predicated: Vec<_> = dump
        .instructions
        .iter()
        .filter(|i| i.is_predicated)
        .collect();
    // Should have many predicated variants
    assert!(
        predicated.len() >= 200,
        "Expected >= 200 predicated instructions, got {}",
        predicated.len()
    );

    // All predicated false should also be predicated
    for insn in &dump.instructions {
        if insn.is_predicated_false {
            assert!(
                insn.is_predicated,
                "{} is predicatedFalse but not isPredicated",
                insn.name
            );
        }
        if insn.is_predicated_new {
            assert!(
                insn.is_predicated,
                "{} is predicatedNew but not isPredicated",
                insn.name
            );
        }
    }
}

#[test]
fn test_json_roundtrip() {
    let Some(content) = load_real_tablegen() else {
        return;
    };
    let dump = parse_tablegen(&content).unwrap();
    let json = serde_json::to_string(&dump).unwrap();
    let dump2: hex_dump::types::InstructionSetDump = serde_json::from_str(&json).unwrap();
    assert_eq!(dump.instructions.len(), dump2.instructions.len());
    assert_eq!(dump.total_parsed, dump2.total_parsed);
    // Spot-check a few
    for (a, b) in dump.instructions.iter().zip(dump2.instructions.iter()) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.itype, b.itype);
        assert_eq!(a.asm_syntax, b.asm_syntax);
    }
}
