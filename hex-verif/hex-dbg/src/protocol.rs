// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! GDB Remote Serial Protocol wire-level framing.
//!
//! Pure functions for encoding/decoding RSP packets, checksums, hex
//! conversion, and RLE expansion. No I/O types — fully unit-testable.

use std::io::{BufRead, Write};

use anyhow::{bail, Context, Result};

/// Compute the RSP checksum (sum of bytes mod 256).
pub fn checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
}

/// Encode `data` into a framed RSP packet: `$data#XX`.
pub fn encode_packet(data: &[u8]) -> Vec<u8> {
    let csum = checksum(data);
    let mut out = Vec::with_capacity(data.len() + 4);
    out.push(b'$');
    out.extend_from_slice(data);
    out.push(b'#');
    out.push(hex_nibble(csum >> 4));
    out.push(hex_nibble(csum & 0xf));
    out
}

/// Read one RSP packet from a buffered reader.
///
/// Skips leading bytes until `$`, reads until `#XX`, validates
/// the checksum, and expands RLE runs.
pub fn read_packet(reader: &mut impl BufRead) -> Result<Vec<u8>> {
    // Skip until '$'
    loop {
        let byte = read_byte(reader)?;
        if byte == b'$' {
            break;
        }
    }

    // Read payload until '#'
    let mut raw = Vec::new();
    loop {
        let byte = read_byte(reader)?;
        if byte == b'#' {
            break;
        }
        raw.push(byte);
    }

    // Read two-char hex checksum
    let hi = read_byte(reader)?;
    let lo = read_byte(reader)?;
    let expected = parse_hex_byte(hi, lo).context("invalid checksum hex")?;
    let actual = checksum(&raw);
    if actual != expected {
        bail!(
            "checksum mismatch: expected {:#04x}, got {:#04x}",
            expected,
            actual
        );
    }

    // Expand RLE
    expand_rle(&raw)
}

/// Send an ACK (`+`).
pub fn send_ack(writer: &mut impl Write) -> Result<()> {
    writer.write_all(b"+").context("send ack")?;
    writer.flush().context("flush ack")
}

/// Send a NAK (`-`).
pub fn send_nak(writer: &mut impl Write) -> Result<()> {
    writer.write_all(b"-").context("send nak")?;
    writer.flush().context("flush nak")
}

/// Decode a hex-ASCII byte string into raw bytes.
///
/// Input length must be even. Each pair of ASCII hex chars becomes one byte.
pub fn hex_decode(hex: &[u8]) -> Result<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        bail!("hex_decode: odd length {}", hex.len());
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    for pair in hex.chunks_exact(2) {
        out.push(parse_hex_byte(pair[0], pair[1]).context("hex_decode")?);
    }
    Ok(out)
}

/// Encode raw bytes into lowercase hex-ASCII.
pub fn hex_encode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(hex_nibble(b >> 4));
        out.push(hex_nibble(b & 0xf));
    }
    out
}

// ── helpers ──────────────────────────────────────────────────────────

fn hex_nibble(n: u8) -> u8 {
    match n {
        0..=9 => b'0' + n,
        10..=15 => b'a' + n - 10,
        _ => b'?',
    }
}

fn from_hex_char(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn parse_hex_byte(hi: u8, lo: u8) -> Result<u8> {
    let h = from_hex_char(hi).context("invalid hex char")?;
    let l = from_hex_char(lo).context("invalid hex char")?;
    Ok((h << 4) | l)
}

fn read_byte(reader: &mut impl BufRead) -> Result<u8> {
    let mut buf = [0u8; 1];
    reader.read_exact(&mut buf).context("read_byte")?;
    Ok(buf[0])
}

/// Expand GDB RSP run-length encoding.
///
/// `*` followed by a char `c` means "repeat the preceding byte `(c - 29)` times".
/// The preceding byte has already been emitted once, so the repeat count is
/// the number of *additional* copies.
fn expand_rle(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'*' {
            if out.is_empty() {
                bail!("RLE '*' with no preceding byte");
            }
            i += 1;
            if i >= data.len() {
                bail!("RLE '*' at end of packet");
            }
            let repeat = data[i] as usize - 29;
            let prev = *out.last().unwrap();
            for _ in 0..repeat {
                out.push(prev);
            }
        } else {
            out.push(data[i]);
        }
        i += 1;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_checksum() {
        // "OK" = 0x4f + 0x4b = 0x9a
        assert_eq!(checksum(b"OK"), 0x9a);
        assert_eq!(checksum(b""), 0);
    }

    #[test]
    fn test_encode_packet() {
        let pkt = encode_packet(b"OK");
        let s = String::from_utf8(pkt).unwrap();
        assert_eq!(s, "$OK#9a");
    }

    #[test]
    fn test_hex_roundtrip() {
        let original = vec![0x00, 0x01, 0xfe, 0xff, 0xab, 0xcd];
        let encoded = hex_encode(&original);
        let decoded = hex_decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_hex_decode_odd_length() {
        assert!(hex_decode(b"abc").is_err());
    }

    #[test]
    fn test_rle_decode() {
        // '0' repeated: "0* " means '0' + repeat (0x20 - 29) = 3 more '0's = "0000"
        let expanded = expand_rle(b"0* ").unwrap();
        assert_eq!(expanded, b"0000");
    }

    #[test]
    fn test_rle_edge_no_preceding() {
        assert!(expand_rle(b"*!").is_err());
    }

    #[test]
    fn test_read_packet() {
        // Build a valid packet for "OK"
        let wire = b"$OK#9a";
        let mut cursor = Cursor::new(wire.to_vec());
        let payload = read_packet(&mut cursor).unwrap();
        assert_eq!(payload, b"OK");
    }

    #[test]
    fn test_read_packet_skips_leading_junk() {
        let mut wire = Vec::new();
        wire.extend_from_slice(b"+++"); // ACKs before packet
        wire.extend_from_slice(&encode_packet(b"T05"));
        let mut cursor = Cursor::new(wire);
        let payload = read_packet(&mut cursor).unwrap();
        assert_eq!(payload, b"T05");
    }

    #[test]
    fn test_read_packet_checksum_mismatch() {
        let wire = b"$OK#00"; // wrong checksum
        let mut cursor = Cursor::new(wire.to_vec());
        assert!(read_packet(&mut cursor).is_err());
    }

    #[test]
    fn test_read_packet_with_rle() {
        // Encode a packet whose payload contains RLE
        // Payload: "0* " which after RLE expansion is "0000"
        // We need to compute checksum over the raw (pre-expansion) bytes
        let raw_payload = b"0* ";
        let pkt = encode_packet(raw_payload);
        let mut cursor = Cursor::new(pkt);
        let payload = read_packet(&mut cursor).unwrap();
        assert_eq!(payload, b"0000");
    }
}
