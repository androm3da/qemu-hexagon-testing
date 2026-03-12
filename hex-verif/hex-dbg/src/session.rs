// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! High-level GDB debug session.
//!
//! [`run_session`] connects to a GDB server, sets breakpoints, runs the
//! target, and collects register state at each breakpoint hit. This replaces
//! the LLDB-based register extraction in the hot path.

use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::client::{RspClient, StopReply};
use crate::registers::{self, RegisterLayout, RegisterState};

/// Which GDB server backend we're talking to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Qemu,
    HexagonSim,
}

/// Configuration for a debug session.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub backend: Backend,
    pub address: String,
    pub connect_timeout: Duration,
    pub max_retries: u32,
    pub retry_delay: Duration,
    pub session_timeout: Duration,
}

/// Result of a debug session: register states collected at each breakpoint hit.
#[derive(Debug)]
pub struct SessionResult {
    pub states: Vec<RegisterState>,
}

/// Run a debug session against a GDB server.
///
/// 1. Connects to the server with retries
/// 2. Performs RSP handshake
/// 3. Verifies the target is stopped
/// 4. Sets software breakpoints at each `breakpoint_addrs`
/// 5. Continues execution; on each breakpoint hit reads all registers
/// 6. Detaches when the target exits or all breakpoints are collected
pub fn run_session(config: &SessionConfig, breakpoint_addrs: &[u64]) -> Result<SessionResult> {
    let layout = match config.backend {
        Backend::Qemu => registers::qemu_sysemu_layout(),
        Backend::HexagonSim => registers::hexagon_sim_layout(),
    };
    run_session_with_layout(config, breakpoint_addrs, &layout)
}

/// Run a debug session with a caller-provided register layout.
///
/// This is the implementation behind [`run_session`] and is also useful
/// for testing with simplified register layouts.
pub fn run_session_with_layout(
    config: &SessionConfig,
    breakpoint_addrs: &[u64],
    layout: &RegisterLayout,
) -> Result<SessionResult> {
    let mut client = RspClient::connect(
        &config.address,
        config.connect_timeout,
        config.max_retries,
        config.retry_delay,
    )
    .context("session: connect")?;

    client.handshake().context("session: handshake")?;

    // Verify target is stopped
    let reason = client.halt_reason().context("session: halt_reason")?;
    match reason {
        StopReply::Signal(_) => {} // Good — target is stopped
        other => bail!("session: unexpected halt reason: {:?}", other),
    }

    // Set breakpoints
    for &addr in breakpoint_addrs {
        client
            .set_breakpoint(addr)
            .with_context(|| format!("session: set breakpoint at {:#x}", addr))?;
    }

    // Run and collect
    let mut states = Vec::new();
    let deadline = std::time::Instant::now() + config.session_timeout;

    loop {
        if std::time::Instant::now() > deadline {
            bail!("session: timed out after {:?}", config.session_timeout);
        }

        // Use the full session timeout for the first continue (target may
        // need time to reach the first breakpoint), but a shorter timeout
        // for subsequent continues after we've collected data. Some GDB
        // stubs (e.g. hexagon-sim) don't send W00 on exit, so we detect
        // program termination by the read timing out.
        let read_timeout = if states.is_empty() {
            config.session_timeout
        } else {
            Duration::from_secs(10)
        };
        client
            .set_read_timeout(read_timeout)
            .context("session: set read timeout")?;

        let reply = match client.continue_execution() {
            Ok(reply) => reply,
            Err(_) if !states.is_empty() => {
                // Timeout with states collected — target likely exited
                // without sending a proper exit reply.
                break;
            }
            Err(e) => return Err(e).context("session: continue"),
        };

        match reply {
            StopReply::Signal(5) => {
                // SIGTRAP — breakpoint hit. Read registers first.
                let hex_data = client
                    .read_all_registers()
                    .context("session: read registers")?;
                let state = registers::decode_g_packet(&hex_data, layout)
                    .context("session: decode g-packet")?;
                states.push(state);

                // Single-step past the breakpoint before the next continue.
                // Some GDB stubs (e.g. QEMU Hexagon) don't auto-advance the
                // PC past breakpoints on `c`, causing infinite re-triggering.
                client
                    .single_step()
                    .context("session: step past breakpoint")?;
            }
            StopReply::Signal(sig) => {
                bail!("session: target received signal {}", sig);
            }
            StopReply::Exited(code) => {
                if code != 0 {
                    bail!("session: target exited with code {}", code);
                }
                break;
            }
            StopReply::Terminated(sig) => {
                bail!("session: target terminated by signal {}", sig);
            }
            StopReply::Unknown(reply) => {
                bail!("session: unexpected stop reply: {}", reply);
            }
        }
    }

    // Best-effort detach (target may have already exited)
    let _ = client.detach();

    Ok(SessionResult { states })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol;
    use crate::registers::RegisterDesc;
    use std::io::{BufReader, BufWriter, Read, Write};
    use std::net::TcpListener;

    /// A mock GDB server that simulates a full session:
    /// qSupported → ? → Z0 breakpoints → c/T05/g/s/T05 cycles → W00 exit
    fn run_mock_session_server(
        listener: TcpListener,
        num_breakpoints: usize,
        register_data: Vec<String>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = BufWriter::new(stream);

            let respond = |reader: &mut BufReader<_>, writer: &mut BufWriter<_>, response: &str| {
                let payload = protocol::read_packet(reader).unwrap();
                let _cmd = String::from_utf8(payload).unwrap();
                protocol::send_ack(writer).unwrap();
                let pkt = protocol::encode_packet(response.as_bytes());
                writer.write_all(&pkt).unwrap();
                writer.flush().unwrap();
                let mut ack = [0u8; 1];
                Read::read_exact(reader, &mut ack).unwrap();
            };

            let expect_cmd = |reader: &mut BufReader<_>,
                              writer: &mut BufWriter<_>,
                              expected: &str,
                              response: &str| {
                let payload = protocol::read_packet(reader).unwrap();
                let cmd = String::from_utf8(payload).unwrap();
                assert_eq!(cmd, expected, "expected '{}', got '{}'", expected, cmd);
                protocol::send_ack(writer).unwrap();
                let pkt = protocol::encode_packet(response.as_bytes());
                writer.write_all(&pkt).unwrap();
                writer.flush().unwrap();
                let mut ack = [0u8; 1];
                Read::read_exact(reader, &mut ack).unwrap();
            };

            // qSupported
            respond(&mut reader, &mut writer, "PacketSize=4000");

            // ?
            respond(&mut reader, &mut writer, "T05");

            // Z0 for each breakpoint
            for _ in 0..num_breakpoints {
                respond(&mut reader, &mut writer, "OK");
            }

            // c/T05/g/s/T05 cycles
            for reg_data in &register_data {
                // 'c' -> T05 (breakpoint hit)
                expect_cmd(&mut reader, &mut writer, "c", "T05");

                // 'g' -> register data
                respond(&mut reader, &mut writer, reg_data);

                // 's' -> T05 (step past breakpoint)
                expect_cmd(&mut reader, &mut writer, "s", "T05");
            }

            // Final 'c' -> W00 (normal exit)
            expect_cmd(&mut reader, &mut writer, "c", "W00");

            // Client may try to detach — just accept what comes
            let _ = protocol::read_packet(&mut reader);
        })
    }

    /// Minimal 2-register layout for testing.
    fn test_layout() -> RegisterLayout {
        RegisterLayout {
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
        }
    }

    #[test]
    fn test_session_happy_path() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        // Build register data for 2 GPRs (4 bytes each = 16 hex chars)
        let reg_data_1 = "01000000efbeadde"; // r0=1, r1=0xdeadbeef
        let reg_data_2 = "02000000decafeba"; // r0=2, r1=0xbafecade

        let handle = run_mock_session_server(
            listener,
            2,
            vec![reg_data_1.to_string(), reg_data_2.to_string()],
        );

        let config = SessionConfig {
            backend: Backend::Qemu,
            address: format!("127.0.0.1:{}", port),
            connect_timeout: Duration::from_secs(5),
            max_retries: 0,
            retry_delay: Duration::from_millis(100),
            session_timeout: Duration::from_secs(10),
        };

        let result = run_session_with_layout(&config, &[0x1000, 0x2000], &test_layout()).unwrap();
        assert_eq!(result.states.len(), 2);

        // Verify first state
        assert_eq!(
            result.states[0].registers[0],
            ("r0".into(), "0x00000001".into())
        );
        assert_eq!(
            result.states[0].registers[1],
            ("r1".into(), "0xdeadbeef".into())
        );

        // Verify second state
        assert_eq!(
            result.states[1].registers[0],
            ("r0".into(), "0x00000002".into())
        );
        assert_eq!(
            result.states[1].registers[1],
            ("r1".into(), "0xbafecade".into())
        );

        handle.join().unwrap();
    }

    #[test]
    fn test_session_early_exit() {
        // Server sends W00 immediately after continue (no breakpoints hit)
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = run_mock_session_server(listener, 1, vec![]);

        let config = SessionConfig {
            backend: Backend::Qemu,
            address: format!("127.0.0.1:{}", port),
            connect_timeout: Duration::from_secs(5),
            max_retries: 0,
            retry_delay: Duration::from_millis(100),
            session_timeout: Duration::from_secs(10),
        };

        let result = run_session_with_layout(&config, &[0x1000], &test_layout()).unwrap();
        assert_eq!(result.states.len(), 0);

        handle.join().unwrap();
    }
}
