use std::path::Path;

use hex_dump::parser::parse_tablegen;
use hex_instset::database::InstructionDb;
use hex_packet::constraint::validate_packet;
use hex_packet::slot::assign_slots;
use hex_packet::synthesizer::{PacketSynthesizer, SynthConfig};
use rand::rngs::StdRng;
use rand::SeedableRng;

const TABLEGEN_PATH: &str =
    "/local/mnt/workspace/upstream/llvm-project/llvm/lib/Target/Hexagon/HexagonDepInstrInfo.td";

fn load_real_db() -> Option<InstructionDb> {
    let path = Path::new(TABLEGEN_PATH);
    if !path.exists() {
        eprintln!("Skipping property test: {} not found", TABLEGEN_PATH);
        return None;
    }
    let content = std::fs::read_to_string(path).unwrap();
    let dump = parse_tablegen(&content).unwrap();
    Some(InstructionDb::from_dump(dump))
}

#[test]
fn test_synthesize_1000_packets_all_valid() {
    let Some(db) = load_real_db() else {
        return;
    };

    let recipe = hex_prog::recipe::Recipe::default();

    let config = SynthConfig {
        max_packet_size: recipe.synth.max_packet_size,
        allow_predicated: true,
        allow_predicated_new: recipe.synth.allow_predicated_new,
        allow_new_value: recipe.synth.allow_new_value,
        max_cvi_per_packet: recipe.synth.max_cvi_per_packet,
        skip_terms: recipe.filters.skip_terms.clone(),
        exclude_filters: recipe.filters.exclude.clone(),
        include_filter: recipe.filters.include.clone(),
        blocked_features: recipe.filters.blocked_features.clone(),
        allow_mem_ops: false,
        mem_region_size: 65536,
    };

    let synth = PacketSynthesizer::new(&db, config);
    let mut rng = StdRng::seed_from_u64(12345);

    for i in 0..1000 {
        let packet = synth.synthesize_packet(&mut rng);

        // Verify that the packet passes validation
        let defs: Vec<_> = packet.insns.iter().map(|ci| ci.def).collect();
        let result = validate_packet(&defs);
        assert!(
            result.valid,
            "Packet {} failed validation: {:?}",
            i, result.reason
        );

        // Verify slot assignment succeeds
        let itypes: Vec<&str> = defs.iter().map(|d| d.itype.as_str()).collect();
        assert!(
            assign_slots(&itypes).is_some(),
            "Packet {} failed slot assignment",
            i
        );

        // Verify no dollar signs remain in assembly
        for insn in &packet.insns {
            assert!(
                !insn.asm_text.contains('$'),
                "Packet {} insn has unresolved $: {}",
                i,
                insn.asm_text
            );
        }

        // Verify packet size
        assert!(
            !packet.insns.is_empty() && packet.insns.len() <= 4,
            "Packet {} has invalid size: {}",
            i,
            packet.insns.len()
        );
    }
}

#[test]
fn test_synthesize_deterministic_with_real_db() {
    let Some(db) = load_real_db() else {
        return;
    };

    let recipe = hex_prog::recipe::Recipe::default();

    let config1 = SynthConfig {
        max_packet_size: recipe.synth.max_packet_size,
        allow_predicated: true,
        allow_predicated_new: recipe.synth.allow_predicated_new,
        allow_new_value: recipe.synth.allow_new_value,
        max_cvi_per_packet: recipe.synth.max_cvi_per_packet,
        skip_terms: recipe.filters.skip_terms.clone(),
        exclude_filters: recipe.filters.exclude.clone(),
        include_filter: recipe.filters.include.clone(),
        blocked_features: recipe.filters.blocked_features.clone(),
        allow_mem_ops: false,
        mem_region_size: 65536,
    };
    let config2 = SynthConfig {
        max_packet_size: recipe.synth.max_packet_size,
        allow_predicated: true,
        allow_predicated_new: recipe.synth.allow_predicated_new,
        allow_new_value: recipe.synth.allow_new_value,
        max_cvi_per_packet: recipe.synth.max_cvi_per_packet,
        skip_terms: recipe.filters.skip_terms.clone(),
        exclude_filters: recipe.filters.exclude.clone(),
        include_filter: recipe.filters.include.clone(),
        blocked_features: recipe.filters.blocked_features.clone(),
        allow_mem_ops: false,
        mem_region_size: 65536,
    };

    let synth1 = PacketSynthesizer::new(&db, config1);
    let synth2 = PacketSynthesizer::new(&db, config2);

    let mut rng1 = StdRng::seed_from_u64(99999);
    let mut rng2 = StdRng::seed_from_u64(99999);

    for i in 0..100 {
        let p1 = synth1.synthesize_packet(&mut rng1);
        let p2 = synth2.synthesize_packet(&mut rng2);
        assert_eq!(p1.insns.len(), p2.insns.len(), "Packet {} size mismatch", i);
        for (ia, ib) in p1.insns.iter().zip(p2.insns.iter()) {
            assert_eq!(ia.asm_text, ib.asm_text, "Packet {} asm mismatch", i);
        }
    }
}
