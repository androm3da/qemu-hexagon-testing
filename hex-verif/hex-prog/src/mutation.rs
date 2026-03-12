use rand::prelude::*;
use rand::rngs::StdRng;

/// Generate a block of GPR mutation instructions to add entropy between test packets.
/// Uses bit-manipulation operations like brev, togglebit, rol.
pub fn gen_gpr_mutation_block(rng: &mut StdRng, num_regs: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mutations = [
        MutationType::Brev,
        MutationType::ToggleBit,
        MutationType::Rol,
        MutationType::Xor,
    ];

    // Generate a few mutation instructions for a subset of registers
    let num_mutations = rng.gen_range(2..=4).min(num_regs);
    for _ in 0..num_mutations {
        let reg = rng.gen_range(0..num_regs.min(27));
        let mutation = mutations[rng.gen_range(0..mutations.len())];
        match mutation {
            MutationType::Brev => {
                lines.push(format!("    {{ r{} = brev(r{}) }}", reg, reg));
            }
            MutationType::ToggleBit => {
                let bit = rng.gen_range(0..32);
                lines.push(format!("    {{ r{} = togglebit(r{},#{}) }}", reg, reg, bit));
            }
            MutationType::Rol => {
                let amt = rng.gen_range(1..32);
                lines.push(format!("    {{ r{} = rol(r{},#{}) }}", reg, reg, amt));
            }
            MutationType::Xor => {
                let other = rng.gen_range(0..num_regs.min(27));
                lines.push(format!("    {{ r{} = xor(r{},r{}) }}", reg, reg, other));
            }
        }
    }
    lines
}

/// Generate a block of HVX vector register mutation instructions.
pub fn gen_hvx_mutation_block(rng: &mut StdRng, num_vregs: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let num_mutations = rng.gen_range(1..=2).min(num_vregs);
    for _ in 0..num_mutations {
        let vreg = rng.gen_range(0..num_vregs.min(32));
        let other = rng.gen_range(0..num_vregs.min(32));
        let mutation = rng.gen_range(0..3);
        match mutation {
            0 => {
                // vnot: Vd = vnot(Vu)
                lines.push(format!("    {{ v{} = vnot(v{}) }}", vreg, other));
            }
            1 => {
                // vxor: Vd = vxor(Vu, Vv)
                let third = rng.gen_range(0..num_vregs.min(32));
                lines.push(format!("    {{ v{} = vxor(v{},v{}) }}", vreg, other, third));
            }
            _ => {
                // vdelta: Vd = vdelta(Vu, Vv)
                let third = rng.gen_range(0..num_vregs.min(32));
                lines.push(format!(
                    "    {{ v{} = vdelta(v{},v{}) }}",
                    vreg, other, third
                ));
            }
        }
    }
    lines
}

#[derive(Clone, Copy)]
enum MutationType {
    Brev,
    ToggleBit,
    Rol,
    Xor,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpr_mutation_nonempty() {
        let mut rng = StdRng::seed_from_u64(42);
        let block = gen_gpr_mutation_block(&mut rng, 28);
        assert!(!block.is_empty());
        for line in &block {
            assert!(line.starts_with("    {"));
            assert!(line.ends_with('}'));
        }
    }

    #[test]
    fn test_hvx_mutation_nonempty() {
        let mut rng = StdRng::seed_from_u64(42);
        let block = gen_hvx_mutation_block(&mut rng, 32);
        assert!(!block.is_empty());
        for line in &block {
            assert!(
                line.contains("vnot") || line.contains("vxor") || line.contains("vdelta"),
                "Unexpected mutation: {}",
                line
            );
        }
    }
}
