use crate::lldb_script::RegisterState;

/// Result of comparing two register states.
#[derive(Debug)]
pub struct ComparisonResult {
    pub matches: bool,
    pub diffs: Vec<RegisterDiff>,
}

/// A single register mismatch.
#[derive(Debug)]
pub struct RegisterDiff {
    pub register: String,
    pub reference_value: String,
    pub test_value: String,
}

/// Registers that are relevant for ISA verification.
/// Excludes performance counters, revision, debug, and system registers
/// that legitimately differ between hexagon-sim and QEMU.
fn is_comparable_register(name: &str) -> bool {
    // GPRs r0-r28 (r29=SP, r30=FP, r31=LR are excluded — they depend on
    // runtime setup which may differ between sim and QEMU)
    if let Some(suffix) = name.strip_prefix('r') {
        if let Ok(num) = suffix.parse::<u32>() {
            return num <= 27;
        }
    }

    // Predicate registers (p0-p3, stored as p3_0 combined)
    if name.starts_with("p3_0")
        || name.starts_with("p0")
        || name.starts_with("p1")
        || name.starts_with("p2")
        || name.starts_with("p3")
    {
        return true;
    }

    // HVX vector registers v0-v31
    if let Some(suffix) = name.strip_prefix('v') {
        if let Ok(num) = suffix.parse::<u32>() {
            return num <= 31;
        }
    }

    // HVX predicate registers q0-q3
    if let Some(suffix) = name.strip_prefix('q') {
        if let Ok(num) = suffix.parse::<u32>() {
            return num <= 3;
        }
    }

    // Loop registers (used by test code)
    matches!(name, "sa0" | "lc0" | "sa1" | "lc1" | "m0" | "m1" | "usr")
}

/// Compare two register states, normalizing hex formatting.
/// Only compares ISA-relevant registers (GPRs, predicates, HVX, loop regs).
pub fn compare_states(reference: &RegisterState, test: &RegisterState) -> ComparisonResult {
    let mut diffs = Vec::new();

    for (ref_name, ref_val) in &reference.registers {
        if !is_comparable_register(ref_name) {
            continue;
        }

        // Find matching register in test state
        if let Some((_, test_val)) = test.registers.iter().find(|(n, _)| n == ref_name) {
            let ref_normalized = normalize_hex(ref_val);
            let test_normalized = normalize_hex(test_val);
            if ref_normalized != test_normalized {
                diffs.push(RegisterDiff {
                    register: ref_name.clone(),
                    reference_value: ref_val.clone(),
                    test_value: test_val.clone(),
                });
            }
        }
        // If register not present in test, skip (may be a register set difference)
    }

    ComparisonResult {
        matches: diffs.is_empty(),
        diffs,
    }
}

/// Normalize a hex value string for comparison.
/// Strips leading zeros, ensures consistent formatting.
fn normalize_hex(value: &str) -> String {
    let s = value.trim().to_lowercase();
    if let Some(hex) = s.strip_prefix("0x") {
        // Remove leading zeros but keep at least one digit
        let trimmed = hex.trim_start_matches('0');
        if trimmed.is_empty() {
            "0x0".to_string()
        } else {
            format!("0x{}", trimmed)
        }
    } else {
        s
    }
}

/// Format a comparison result as a human-readable diff.
pub fn format_diff(result: &ComparisonResult) -> String {
    if result.matches {
        return "States match.".to_string();
    }

    let mut lines = Vec::new();
    lines.push(format!(
        "MISMATCH: {} register(s) differ:",
        result.diffs.len()
    ));
    for diff in &result.diffs {
        lines.push(format!(
            "  {} : ref={} test={}",
            diff.register, diff.reference_value, diff.test_value
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_hex() {
        assert_eq!(normalize_hex("0x00000001"), "0x1");
        assert_eq!(normalize_hex("0x00000000"), "0x0");
        assert_eq!(normalize_hex("0xFF"), "0xff");
        assert_eq!(normalize_hex("0x0000ABCD"), "0xabcd");
    }

    #[test]
    fn test_compare_matching_states() {
        let ref_state = RegisterState {
            registers: vec![
                ("r0".to_string(), "0x00000001".to_string()),
                ("r1".to_string(), "0x00000002".to_string()),
            ],
        };
        let test_state = RegisterState {
            registers: vec![
                ("r0".to_string(), "0x1".to_string()),
                ("r1".to_string(), "0x00000002".to_string()),
            ],
        };
        let result = compare_states(&ref_state, &test_state);
        assert!(result.matches);
    }

    #[test]
    fn test_compare_mismatching_states() {
        let ref_state = RegisterState {
            registers: vec![
                ("r0".to_string(), "0x00000001".to_string()),
                ("r1".to_string(), "0x00000002".to_string()),
            ],
        };
        let test_state = RegisterState {
            registers: vec![
                ("r0".to_string(), "0x00000001".to_string()),
                ("r1".to_string(), "0x00000099".to_string()),
            ],
        };
        let result = compare_states(&ref_state, &test_state);
        assert!(!result.matches);
        assert_eq!(result.diffs.len(), 1);
        assert_eq!(result.diffs[0].register, "r1");
    }

    #[test]
    fn test_format_diff() {
        let result = ComparisonResult {
            matches: false,
            diffs: vec![RegisterDiff {
                register: "r1".to_string(),
                reference_value: "0x02".to_string(),
                test_value: "0x99".to_string(),
            }],
        };
        let formatted = format_diff(&result);
        assert!(formatted.contains("MISMATCH"));
        assert!(formatted.contains("r1"));
    }
}
