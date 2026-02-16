/// Generate an LLDB batch script for hexagon-sim (direct launch mode).
///
/// hexagon-lldb natively supports hexagon-sim as an execution backend.
/// Usage: `hexagon-lldb -b -s <script> <elf>`
pub fn generate_sim_script(breakpoints: &[&str]) -> String {
    let mut lines = Vec::new();

    lines.push("settings set auto-confirm true".to_string());

    for bp in breakpoints {
        lines.push(format!("breakpoint set --name {}", bp));
    }

    // Launch the process — hits first breakpoint
    lines.push("run".to_string());

    // At each breakpoint: dump registers, then continue to next
    for _bp in breakpoints {
        lines.push("register read --all".to_string());
        lines.push("continue".to_string());
    }

    lines.push("quit".to_string());
    lines.join("\n")
}

/// Generate an LLDB batch script for QEMU (gdb-remote mode).
///
/// QEMU must already be running with `-gdb tcp::<port> -S`.
/// Usage: `hexagon-lldb -b -s <script> <elf>`
pub fn generate_qemu_script(breakpoints: &[&str], gdb_port: u16) -> String {
    let mut lines = Vec::new();

    lines.push("settings set auto-confirm true".to_string());

    // Connect to QEMU's GDB server (process is stopped at entry)
    lines.push(format!("gdb-remote {}", gdb_port));

    for bp in breakpoints {
        lines.push(format!("breakpoint set --name {}", bp));
    }

    // Continue from entry point — hits first breakpoint
    lines.push("continue".to_string());

    // At each breakpoint: dump registers, then continue to next
    for _bp in breakpoints {
        lines.push("register read --all".to_string());
        lines.push("continue".to_string());
    }

    lines.push("quit".to_string());
    lines.join("\n")
}

/// Parse register state from LLDB output.
/// Returns a list of (breakpoint_name, register_map) tuples.
pub fn parse_lldb_output(output: &str) -> Vec<RegisterState> {
    let mut states = Vec::new();
    let mut current_regs: Vec<(String, String)> = Vec::new();
    let mut in_register_block = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // Detect breakpoint hits
        if trimmed.contains("stop reason = breakpoint") {
            // Save previous state if any
            if !current_regs.is_empty() {
                states.push(RegisterState {
                    registers: current_regs.clone(),
                });
                current_regs.clear();
            }
            in_register_block = false;
        }

        // Detect start of register dump
        if trimmed.starts_with("General Purpose Registers:")
            || trimmed.starts_with("general")
            || trimmed.contains("= 0x")
        {
            in_register_block = true;
        }

        // Parse register lines like "  r0 = 0x00000001"
        if in_register_block {
            if let Some((name, value)) = parse_register_line(trimmed) {
                current_regs.push((name, value));
            }
        }
    }

    // Save last state
    if !current_regs.is_empty() {
        states.push(RegisterState {
            registers: current_regs,
        });
    }

    states
}

/// A snapshot of register state at a breakpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct RegisterState {
    pub registers: Vec<(String, String)>,
}

/// Normalize register name to canonical form.
/// QEMU uses sp/fp/ra/lr aliases; hexagon-sim uses r29/r30/r31.
fn normalize_reg_name(name: &str) -> String {
    match name {
        "sp" => "r29".to_string(),
        "fp" => "r30".to_string(),
        "ra" | "lr" => "r31".to_string(),
        other => other.to_string(),
    }
}

/// Parse a single register line like "  r0 = 0x00000001".
fn parse_register_line(line: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = line.split('=').collect();
    if parts.len() >= 2 {
        let raw_name = parts[0].trim();
        let value = parts[1].split_whitespace().next()?.to_string();
        // Only accept lines that look like register names
        if raw_name.starts_with('r')
            || raw_name.starts_with('p')
            || raw_name.starts_with('v')
            || raw_name.starts_with("pc")
            || raw_name.starts_with("usr")
            || raw_name == "sp"
            || raw_name == "fp"
            || raw_name == "ra"
            || raw_name == "lr"
        {
            let name = normalize_reg_name(raw_name);
            return Some((name, value));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_sim_script() {
        let script = generate_sim_script(&["steps", "test_end"]);
        assert!(script.contains("breakpoint set --name steps"));
        assert!(script.contains("breakpoint set --name test_end"));
        assert!(script.contains("register read --all"));
        assert!(script.contains("run"));
        assert!(script.contains("quit"));
        // Should NOT contain breakpoint command add (uses sequential approach)
        assert!(!script.contains("breakpoint command add"));
    }

    #[test]
    fn test_generate_qemu_script() {
        let script = generate_qemu_script(&["steps", "test_end"], 12345);
        assert!(script.contains("gdb-remote 12345"));
        assert!(script.contains("breakpoint set --name steps"));
        assert!(script.contains("continue"));
        assert!(script.contains("register read --all"));
        // QEMU script should not use "run" — it uses "continue" from gdb-remote
        let lines: Vec<&str> = script.lines().collect();
        assert!(!lines.iter().any(|l| l.trim() == "run"));
    }

    #[test]
    fn test_parse_register_line() {
        assert_eq!(
            parse_register_line("  r0 = 0x00000001"),
            Some(("r0".to_string(), "0x00000001".to_string()))
        );
        assert_eq!(
            parse_register_line("  p0 = 0xff"),
            Some(("p0".to_string(), "0xff".to_string()))
        );
        assert_eq!(parse_register_line("some random text"), None);
    }

    #[test]
    fn test_parse_register_line_aliases() {
        // QEMU uses sp/fp/ra aliases that should be normalized
        assert_eq!(
            parse_register_line("  sp = 0x0411d7e0"),
            Some(("r29".to_string(), "0x0411d7e0".to_string()))
        );
        assert_eq!(
            parse_register_line("  fp = 0x0411d7e8"),
            Some(("r30".to_string(), "0x0411d7e8".to_string()))
        );
        assert_eq!(
            parse_register_line("  ra = 0x000070e0"),
            Some(("r31".to_string(), "0x000070e0".to_string()))
        );
    }

    #[test]
    fn test_parse_lldb_output() {
        let output = "\
Process 1234 stopped
* thread #1, stop reason = breakpoint 1.1
General Purpose Registers:
  r0 = 0x00000001
  r1 = 0x00000002
  r2 = 0x00000003
Process 1234 resuming
Process 1234 stopped
* thread #1, stop reason = breakpoint 2.1
General Purpose Registers:
  r0 = 0x00000004
  r1 = 0x00000005
";
        let states = parse_lldb_output(output);
        assert_eq!(states.len(), 2);
        assert_eq!(states[0].registers.len(), 3);
        assert_eq!(
            states[0].registers[0],
            ("r0".to_string(), "0x00000001".to_string())
        );
        assert_eq!(states[1].registers.len(), 2);
        assert_eq!(
            states[1].registers[0],
            ("r0".to_string(), "0x00000004".to_string())
        );
    }
}
