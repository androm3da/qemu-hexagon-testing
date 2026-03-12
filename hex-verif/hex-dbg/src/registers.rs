// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Hexagon register layout and GDB `g`-packet decoding.
//!
//! Defines the register ordering for QEMU sysemu and hexagon-sim GDB stubs,
//! and decodes the hex string returned by the `g` (read-all-registers) command
//! into named register values.

use anyhow::{bail, Context, Result};

use crate::protocol;

/// Description of a single register in the GDB stub's register file.
#[derive(Debug, Clone)]
pub struct RegisterDesc {
    pub name: String,
    pub gdb_index: u32,
    pub size_bytes: usize,
}

/// Ordered collection of register descriptions defining a `g`-packet layout.
#[derive(Debug, Clone)]
pub struct RegisterLayout {
    pub regs: Vec<RegisterDesc>,
}

/// Decoded register state: list of `(name, hex_value_string)` pairs.
#[derive(Debug, Clone, PartialEq)]
pub struct RegisterState {
    pub registers: Vec<(String, String)>,
}

impl RegisterLayout {
    /// Total bytes expected in the `g`-packet for this layout.
    pub fn total_bytes(&self) -> usize {
        self.regs.iter().map(|r| r.size_bytes).sum()
    }
}

/// Build the QEMU sysemu register layout.
///
/// Register ordering from `target/hexagon/gdbstub.c` + `hex_regs.h`:
///
/// | GDB idx | Name       | Size  |
/// |---------|------------|-------|
/// | 0-31    | r0-r31     | 4 ea  |
/// | 32      | sa0        | 4     |
/// | 33      | lc0        | 4     |
/// | 34      | sa1        | 4     |
/// | 35      | lc1        | 4     |
/// | 36      | p3:0       | 4     |
/// | 37      | c5 (reserved) | 4  |
/// | 38      | m0         | 4     |
/// | 39      | m1         | 4     |
/// | 40      | usr        | 4     |
/// | 41      | pc         | 4     |
/// | 42-63   | ugp..framekey | 4 ea |
/// | 64-95   | v0-v31     | 128 ea|
/// | 96-99   | q0-q3      | 16 ea |
/// | 100-163 | s0-s63     | 4 ea  |
/// | 164-195 | g0-g31     | 4 ea  |
pub fn qemu_sysemu_layout() -> RegisterLayout {
    let mut regs = Vec::new();
    let mut idx: u32 = 0;

    // r0-r31 (GPRs)
    for i in 0..32 {
        regs.push(RegisterDesc {
            name: format!("r{}", i),
            gdb_index: idx,
            size_bytes: 4,
        });
        idx += 1;
    }

    // Control registers c0-c9 mapped to named aliases
    let ctrl_names = [
        "sa0", "lc0", "sa1", "lc1", "p3:0", "c5", "m0", "m1", "usr", "pc",
    ];
    for name in &ctrl_names {
        regs.push(RegisterDesc {
            name: name.to_string(),
            gdb_index: idx,
            size_bytes: 4,
        });
        idx += 1;
    }

    // Remaining control registers c10-c31
    let upper_ctrl_names = [
        "ugp",
        "gp",
        "cs0",
        "cs1",
        "upcyclelo",
        "upcyclehi",
        "framelimit",
        "framekey",
        "pktcntlo",
        "pktcnthi",
        "c20",
        "c21",
        "c22",
        "c23",
        "c24",
        "c25",
        "c26",
        "c27",
        "c28",
        "c29",
        "utimerlo",
        "utimerhi",
    ];
    for name in &upper_ctrl_names {
        regs.push(RegisterDesc {
            name: name.to_string(),
            gdb_index: idx,
            size_bytes: 4,
        });
        idx += 1;
    }

    // HVX vector registers v0-v31 (128 bytes each)
    for i in 0..32 {
        regs.push(RegisterDesc {
            name: format!("v{}", i),
            gdb_index: idx,
            size_bytes: 128,
        });
        idx += 1;
    }

    // HVX predicate registers q0-q3 (16 bytes each)
    for i in 0..4 {
        regs.push(RegisterDesc {
            name: format!("q{}", i),
            gdb_index: idx,
            size_bytes: 16,
        });
        idx += 1;
    }

    // System registers s0-s63
    for i in 0..64 {
        regs.push(RegisterDesc {
            name: format!("s{}", i),
            gdb_index: idx,
            size_bytes: 4,
        });
        idx += 1;
    }

    // Guest registers g0-g31
    for i in 0..32 {
        regs.push(RegisterDesc {
            name: format!("g{}", i),
            gdb_index: idx,
            size_bytes: 4,
        });
        idx += 1;
    }

    RegisterLayout { regs }
}

/// Build the hexagon-sim register layout.
///
/// hexagon-sim's GDB stub exposes the same thread-level registers as QEMU
/// for indices 0-63, plus HVX. System and guest registers may differ;
/// this initial implementation mirrors QEMU's layout.
pub fn hexagon_sim_layout() -> RegisterLayout {
    // For now identical to QEMU sysemu. Can diverge as needed.
    qemu_sysemu_layout()
}

/// Decode a `g`-packet hex string into a [`RegisterState`].
///
/// The hex string contains little-endian register values concatenated
/// according to `layout`. Each register's bytes are reversed to produce
/// a big-endian `0x...` display string.
///
/// Registers whose byte range extends beyond the available data are silently
/// skipped. This allows a single layout definition to work with backends
/// that expose different numbers of registers (e.g., hexagon-sim returns
/// fewer registers than QEMU sysemu).
pub fn decode_g_packet(hex_data: &str, layout: &RegisterLayout) -> Result<RegisterState> {
    let data_len = hex_data.len();
    let hex_bytes = hex_data.as_bytes();
    let mut registers = Vec::with_capacity(layout.regs.len());
    let mut offset = 0; // offset in hex chars

    for reg in &layout.regs {
        let hex_len = reg.size_bytes * 2;
        if offset + hex_len > data_len {
            // Remaining registers don't fit — stop decoding
            break;
        }
        let chunk = &hex_bytes[offset..offset + hex_len];

        // Decode from hex ASCII to raw bytes (little-endian)
        let le_bytes = protocol::hex_decode(chunk)
            .with_context(|| format!("decoding register {}", reg.name))?;

        // Convert to big-endian display string
        let value_str = format!("0x{}", be_hex_string(&le_bytes));
        registers.push((reg.name.clone(), value_str));

        offset += hex_len;
    }

    if registers.is_empty() {
        bail!(
            "g-packet too short to decode any registers: got {} hex chars",
            data_len,
        );
    }

    Ok(RegisterState { registers })
}

/// Unpack the `p3:0` packed predicate word into individual predicates.
///
/// The packed word stores `p0` in bits \[7:0\], `p1` in \[15:8\],
/// `p2` in \[23:16\], `p3` in \[31:24\].
pub fn unpack_predicates(p3_0: u32) -> [(String, String); 4] {
    [
        ("p0".to_string(), format!("0x{:02x}", p3_0 & 0xff)),
        ("p1".to_string(), format!("0x{:02x}", (p3_0 >> 8) & 0xff)),
        ("p2".to_string(), format!("0x{:02x}", (p3_0 >> 16) & 0xff)),
        ("p3".to_string(), format!("0x{:02x}", (p3_0 >> 24) & 0xff)),
    ]
}

/// Convert little-endian bytes to a big-endian hex display string.
fn be_hex_string(le_bytes: &[u8]) -> String {
    let be_hex = protocol::hex_encode(&le_bytes.iter().copied().rev().collect::<Vec<_>>());
    String::from_utf8(be_hex).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layout_offsets() {
        let layout = qemu_sysemu_layout();
        // 32 GPRs + 10 ctrl + 22 upper ctrl + 32 HVX vec + 4 HVX pred + 64 sys + 32 guest
        assert_eq!(layout.regs.len(), 32 + 10 + 22 + 32 + 4 + 64 + 32);
        // Total bytes: 64*4 + 32*128 + 4*16 + 64*4 + 32*4
        let expected = 64 * 4 + 32 * 128 + 4 * 16 + 64 * 4 + 32 * 4;
        assert_eq!(layout.total_bytes(), expected);
    }

    #[test]
    fn test_decode_gprs() {
        // Minimal layout: just 2 GPRs (4 bytes each)
        let layout = RegisterLayout {
            regs: vec![
                RegisterDesc {
                    name: "r0".into(),
                    gdb_index: 0,
                    size_bytes: 4,
                },
                RegisterDesc {
                    name: "r1".into(),
                    gdb_index: 1,
                    size_bytes: 4,
                },
            ],
        };

        // r0 = 0x00000001 in LE hex: "01000000"
        // r1 = 0xdeadbeef in LE hex: "efbeadde"
        let hex_data = "01000000efbeadde";
        let state = decode_g_packet(hex_data, &layout).unwrap();
        assert_eq!(state.registers.len(), 2);
        assert_eq!(state.registers[0], ("r0".into(), "0x00000001".into()));
        assert_eq!(state.registers[1], ("r1".into(), "0xdeadbeef".into()));
    }

    #[test]
    fn test_le_decode() {
        // Verify little-endian to big-endian conversion
        let le = vec![0xef, 0xbe, 0xad, 0xde];
        let s = be_hex_string(&le);
        assert_eq!(s, "deadbeef");
    }

    #[test]
    fn test_unpack_predicates() {
        // p3:0 = 0x03020100
        let preds = unpack_predicates(0x03020100);
        assert_eq!(preds[0], ("p0".into(), "0x00".into()));
        assert_eq!(preds[1], ("p1".into(), "0x01".into()));
        assert_eq!(preds[2], ("p2".into(), "0x02".into()));
        assert_eq!(preds[3], ("p3".into(), "0x03".into()));
    }

    #[test]
    fn test_vector_decode() {
        // 128-byte HVX vector register: all 0x42
        let layout = RegisterLayout {
            regs: vec![RegisterDesc {
                name: "v0".into(),
                gdb_index: 64,
                size_bytes: 128,
            }],
        };
        let hex_data = "42".repeat(128);
        let state = decode_g_packet(&hex_data, &layout).unwrap();
        assert_eq!(state.registers.len(), 1);
        assert!(state.registers[0].1.starts_with("0x"));
        // All 0x42 bytes reversed is still all 0x42
        assert_eq!(state.registers[0].1, format!("0x{}", "42".repeat(128)));
    }

    #[test]
    fn test_g_packet_too_short() {
        let layout = RegisterLayout {
            regs: vec![RegisterDesc {
                name: "r0".into(),
                gdb_index: 0,
                size_bytes: 4,
            }],
        };
        // 4 hex chars is too short for a 4-byte register (needs 8)
        assert!(decode_g_packet("0100", &layout).is_err());
    }

    #[test]
    fn test_g_packet_partial_layout() {
        // Layout defines 3 registers but g-packet only has data for the first 2
        let layout = RegisterLayout {
            regs: vec![
                RegisterDesc {
                    name: "r0".into(),
                    gdb_index: 0,
                    size_bytes: 4,
                },
                RegisterDesc {
                    name: "r1".into(),
                    gdb_index: 1,
                    size_bytes: 4,
                },
                RegisterDesc {
                    name: "r2".into(),
                    gdb_index: 2,
                    size_bytes: 4,
                },
            ],
        };
        // Only 16 hex chars = 8 bytes = 2 registers worth
        let hex_data = "01000000efbeadde";
        let state = decode_g_packet(hex_data, &layout).unwrap();
        assert_eq!(state.registers.len(), 2);
        assert_eq!(state.registers[0], ("r0".into(), "0x00000001".into()));
        assert_eq!(state.registers[1], ("r1".into(), "0xdeadbeef".into()));
    }
}
