// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! GDB Remote Serial Protocol client for Hexagon emulator verification.
//!
//! This crate provides a direct GDB RSP client that connects to hexagon-sim
//! and QEMU GDB servers, replacing the heavyweight LLDB-based register
//! extraction in the hex-verif hot loop.
//!
//! # Modules
//!
//! - [`protocol`] — RSP wire protocol: framing, checksums, hex codec, RLE
//! - [`client`] — TCP-based RSP client with command/response methods
//! - [`registers`] — Hexagon register layout and `g`-packet decoding
//! - [`session`] — High-level: connect → breakpoints → run → read state → detach
//! - [`elf`] — ELF symbol table reader for resolving breakpoint addresses
//! - [`port`] — TOCTOU-safe port allocation via [`PortGuard`](port::PortGuard)

pub mod client;
pub mod elf;
pub mod port;
pub mod protocol;
pub mod registers;
pub mod session;
