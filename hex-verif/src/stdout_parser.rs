// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Parse raw binary register dumps from stdout.
//!
//! The generated test program writes register state to stdout as raw
//! little-endian binary via the `write_binary_stdout()` helper. Each dump
//! is preceded by a 4-byte sentinel ("HXRG") so the parser can locate real
//! dumps even if the emulator prints crash/error text to stdout.

use anyhow::{bail, Result};
use hex_dbg::registers::RegisterState;
use hex_prog::template::{DUMP_SENTINEL, REG_DUMP_COUNT};

/// Size in bytes of a single scalar register dump (excluding sentinel).
pub const SCALAR_DUMP_SIZE: usize = REG_DUMP_COUNT as usize * 4;

/// Register names in dump order.
/// Must match the store order in `template.rs:emit_steps` test_end sequence.
const SCALAR_REG_NAMES: [&str; 36] = [
    "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11", "r12", "r13", "r14",
    "r15", "r16", "r17", "r18", "r19", "r20", "r21", "r22", "r23", "r24", "r25", "r26", "r27",
    "p3:0", "sa0", "lc0", "sa1", "lc1", "m0", "m1", "usr",
];

/// Find all occurrences of the sentinel in a byte slice and return the
/// byte offset immediately after each sentinel.
fn find_sentinel_offsets(data: &[u8]) -> Vec<usize> {
    let sentinel = DUMP_SENTINEL;
    let mut offsets = Vec::new();
    let mut pos = 0;
    while pos + sentinel.len() <= data.len() {
        if &data[pos..pos + sentinel.len()] == sentinel {
            offsets.push(pos + sentinel.len());
            pos += sentinel.len(); // skip past this sentinel
        } else {
            pos += 1;
        }
    }
    offsets
}

/// Parse a scalar register dump from raw stdout bytes.
///
/// Scans for sentinel markers, then parses the [`SCALAR_DUMP_SIZE`] bytes
/// after each sentinel as little-endian u32 values in the order defined by
/// [`SCALAR_REG_NAMES`].
pub fn parse_scalar_dump(stdout: &[u8]) -> Result<Vec<RegisterState>> {
    let offsets = find_sentinel_offsets(stdout);
    if offsets.is_empty() {
        bail!(
            "no HXRG sentinel found in {} bytes of stdout — \
             program likely crashed before dumping registers",
            stdout.len()
        );
    }

    let mut states = Vec::new();
    for &start in &offsets {
        if start + SCALAR_DUMP_SIZE > stdout.len() {
            break; // truncated dump, skip
        }
        let chunk = &stdout[start..start + SCALAR_DUMP_SIZE];
        states.push(parse_scalar_chunk(chunk));
    }

    if states.is_empty() {
        bail!(
            "found {} sentinel(s) but no complete scalar dump ({} bytes needed after sentinel)",
            offsets.len(),
            SCALAR_DUMP_SIZE
        );
    }

    Ok(states)
}

/// Parse a combined scalar + HVX register dump from raw stdout bytes.
///
/// Expects two sentinel-prefixed writes per dump: one for scalar registers,
/// one for HVX registers (v0-v31 + q0-q3).
pub fn parse_scalar_and_hvx_dump(stdout: &[u8]) -> Result<Vec<RegisterState>> {
    const HVX_VREG_COUNT: usize = 32;
    const HVX_QREG_COUNT: usize = 4;
    const HVX_VECTOR_SIZE: usize = 128;
    const HVX_DUMP_SIZE: usize = (HVX_VREG_COUNT + HVX_QREG_COUNT) * HVX_VECTOR_SIZE;

    let offsets = find_sentinel_offsets(stdout);
    if offsets.len() < 2 {
        bail!(
            "need at least 2 HXRG sentinels for scalar+HVX dump, found {}",
            offsets.len()
        );
    }

    let mut states = Vec::new();
    // Process sentinel pairs: (scalar, hvx)
    let mut i = 0;
    while i + 1 < offsets.len() {
        let scalar_start = offsets[i];
        let hvx_start = offsets[i + 1];

        if scalar_start + SCALAR_DUMP_SIZE > stdout.len()
            || hvx_start + HVX_DUMP_SIZE > stdout.len()
        {
            break; // truncated
        }

        let scalar_chunk = &stdout[scalar_start..scalar_start + SCALAR_DUMP_SIZE];
        let mut state = parse_scalar_chunk(scalar_chunk);

        // Parse HVX vector registers v0-v31
        let hvx_chunk = &stdout[hvx_start..hvx_start + HVX_DUMP_SIZE];
        for v in 0..HVX_VREG_COUNT {
            let offset = v * HVX_VECTOR_SIZE;
            let bytes = &hvx_chunk[offset..offset + HVX_VECTOR_SIZE];
            let be_hex: String = bytes.iter().rev().map(|b| format!("{:02x}", b)).collect();
            state
                .registers
                .push((format!("v{}", v), format!("0x{}", be_hex)));
        }

        // Parse HVX predicate registers q0-q3 (128-byte expanded → 16-byte packed)
        for q in 0..HVX_QREG_COUNT {
            let offset = HVX_VREG_COUNT * HVX_VECTOR_SIZE + q * HVX_VECTOR_SIZE;
            let expanded = &hvx_chunk[offset..offset + HVX_VECTOR_SIZE];
            let mut packed = [0u8; 16];
            for (byte_idx, &val) in expanded.iter().enumerate() {
                if val != 0 {
                    packed[byte_idx / 8] |= 1 << (byte_idx % 8);
                }
            }
            let be_hex: String = packed.iter().rev().map(|b| format!("{:02x}", b)).collect();
            state
                .registers
                .push((format!("q{}", q), format!("0x{}", be_hex)));
        }

        states.push(state);
        i += 2;
    }

    if states.is_empty() {
        bail!("found sentinels but no complete scalar+HVX dump pair");
    }

    Ok(states)
}

/// Parse a single scalar dump chunk into a RegisterState.
fn parse_scalar_chunk(chunk: &[u8]) -> RegisterState {
    let mut registers = Vec::with_capacity(SCALAR_REG_NAMES.len());
    for (i, &name) in SCALAR_REG_NAMES.iter().enumerate() {
        let offset = i * 4;
        let bytes: [u8; 4] = chunk[offset..offset + 4]
            .try_into()
            .expect("chunk is at least SCALAR_DUMP_SIZE");
        let val = u32::from_le_bytes(bytes);
        registers.push((name.to_string(), format!("0x{:08x}", val)));
    }
    RegisterState { registers }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_scalar_dump(values: &[u32]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(DUMP_SENTINEL);
        for &v in values {
            data.extend_from_slice(&v.to_le_bytes());
        }
        data
    }

    #[test]
    fn test_parse_scalar_dump_basic() {
        let values: Vec<u32> = (1..=REG_DUMP_COUNT).collect();
        let data = make_scalar_dump(&values);

        let states = parse_scalar_dump(&data).unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].registers.len(), 36);
        assert_eq!(
            states[0].registers[0],
            ("r0".to_string(), "0x00000001".to_string())
        );
        assert_eq!(
            states[0].registers[27],
            ("r27".to_string(), "0x0000001c".to_string())
        );
        assert_eq!(
            states[0].registers[28],
            ("p3:0".to_string(), "0x0000001d".to_string())
        );
        assert_eq!(
            states[0].registers[35],
            ("usr".to_string(), "0x00000024".to_string())
        );
    }

    #[test]
    fn test_parse_scalar_dump_empty() {
        assert!(parse_scalar_dump(&[]).is_err());
    }

    #[test]
    fn test_parse_scalar_dump_no_sentinel() {
        let data = vec![0u8; SCALAR_DUMP_SIZE];
        assert!(parse_scalar_dump(&data).is_err());
    }

    #[test]
    fn test_parse_scalar_dump_with_leading_garbage() {
        // Simulate crash text before the register dump
        let mut data = b"CRASH from thread 0! Misaligned Store @ 0x12345678\n".to_vec();
        let values: Vec<u32> = (1..=REG_DUMP_COUNT).collect();
        data.extend_from_slice(DUMP_SENTINEL);
        for &v in &values {
            data.extend_from_slice(&v.to_le_bytes());
        }

        let states = parse_scalar_dump(&data).unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(
            states[0].registers[0],
            ("r0".to_string(), "0x00000001".to_string())
        );
    }

    #[test]
    fn test_parse_scalar_dump_two_iterations() {
        let mut data = Vec::new();
        // First dump: values 1..36
        data.extend_from_slice(DUMP_SENTINEL);
        for i in 0..REG_DUMP_COUNT {
            data.extend_from_slice(&(i + 1).to_le_bytes());
        }
        // Second dump: values 101..136
        data.extend_from_slice(DUMP_SENTINEL);
        for i in 0..REG_DUMP_COUNT {
            data.extend_from_slice(&(i + 101).to_le_bytes());
        }

        let states = parse_scalar_dump(&data).unwrap();
        assert_eq!(states.len(), 2);
        assert_eq!(states[0].registers[0].1, "0x00000001");
        assert_eq!(states[1].registers[0].1, "0x00000065");
    }
}
