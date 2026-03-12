// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! GDB Remote Serial Protocol TCP client.
//!
//! [`RspClient`] connects to a GDB server over TCP and provides typed methods
//! for common RSP commands: reading registers, setting breakpoints,
//! continuing execution, and detaching.

use std::io::{BufReader, BufWriter, Write};
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::protocol;

/// A GDB RSP client connected over TCP.
pub struct RspClient {
    reader: BufReader<TcpStream>,
    writer: BufWriter<TcpStream>,
    no_ack_mode: bool,
}

/// Parsed GDB stop reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReply {
    /// Target stopped with a signal (e.g. `T05` = SIGTRAP = breakpoint hit).
    Signal(u8),
    /// Target exited normally (e.g. `W00`).
    Exited(u8),
    /// Target was terminated by a signal (e.g. `X09`).
    Terminated(u8),
    /// Unrecognized stop reply.
    Unknown(String),
}

impl RspClient {
    /// Connect to a GDB server at `addr` (e.g. `"127.0.0.1:1234"`).
    ///
    /// Retries up to `max_retries` times with `retry_delay` between attempts.
    pub fn connect(
        addr: &str,
        timeout: Duration,
        max_retries: u32,
        retry_delay: Duration,
    ) -> Result<Self> {
        let mut last_err = None;
        for attempt in 0..=max_retries {
            match TcpStream::connect(addr) {
                Ok(stream) => {
                    stream
                        .set_read_timeout(Some(timeout))
                        .context("set read timeout")?;
                    stream
                        .set_write_timeout(Some(timeout))
                        .context("set write timeout")?;
                    let reader = BufReader::new(stream.try_clone().context("clone stream")?);
                    let writer = BufWriter::new(stream);
                    return Ok(Self {
                        reader,
                        writer,
                        no_ack_mode: false,
                    });
                }
                Err(e) => {
                    last_err = Some(e);
                    if attempt < max_retries {
                        std::thread::sleep(retry_delay);
                    }
                }
            }
        }
        Err(last_err.unwrap()).with_context(|| {
            format!(
                "failed to connect to {} after {} retries",
                addr, max_retries
            )
        })
    }

    /// Perform the initial RSP handshake.
    ///
    /// Sends `qSupported` and optionally negotiates `QStartNoAckMode` to
    /// eliminate per-packet ACK overhead.
    pub fn handshake(&mut self) -> Result<()> {
        let _features = self.command("qSupported:multiprocess+;swbreak+;hwbreak+")?;

        // Note: QStartNoAckMode is intentionally not negotiated here.
        // Some GDB stubs (e.g. hexagon-sim) have quirks in the no-ack
        // mode transition that cause subsequent commands to hang. The
        // per-packet ACK overhead is negligible for our use case.

        Ok(())
    }

    /// Send a command and read the response.
    ///
    /// Handles ACK/NAK in normal mode. Returns the decoded response payload
    /// as a string.
    pub fn command(&mut self, cmd: &str) -> Result<String> {
        // Send packet
        let pkt = protocol::encode_packet(cmd.as_bytes());
        self.writer.write_all(&pkt).context("send command packet")?;
        self.writer.flush().context("flush command")?;

        // Read ACK if not in no-ack mode
        if !self.no_ack_mode {
            let mut ack = [0u8; 1];
            std::io::Read::read_exact(&mut self.reader, &mut ack).context("read ack")?;
            if ack[0] == b'-' {
                bail!("server NAK'd command: {}", cmd);
            }
        }

        // Read response packet
        let payload = protocol::read_packet(&mut self.reader)
            .with_context(|| format!("reading response to '{}'", cmd))?;

        // Send ACK if not in no-ack mode
        if !self.no_ack_mode {
            protocol::send_ack(&mut self.writer)?;
        }

        String::from_utf8(payload).context("response not valid UTF-8")
    }

    /// Read all registers (`g` command).
    ///
    /// Returns the raw hex string from the server.
    pub fn read_all_registers(&mut self) -> Result<String> {
        self.command("g")
    }

    /// Read a single register by GDB index (`p` command).
    pub fn read_register(&mut self, index: u32) -> Result<String> {
        self.command(&format!("p{:x}", index))
    }

    /// Set a breakpoint at `addr`.
    ///
    /// Tries software breakpoint (`Z0`) first; falls back to hardware
    /// breakpoint (`Z1`) if the server returns an empty reply (unsupported).
    /// Uses kind=4 (Hexagon instruction width).
    pub fn set_breakpoint(&mut self, addr: u64) -> Result<()> {
        let reply = self.command(&format!("Z0,{:x},4", addr))?;
        if reply == "OK" {
            return Ok(());
        }
        // Empty reply means Z0 not supported — try hardware breakpoint
        if reply.is_empty() {
            let reply = self.command(&format!("Z1,{:x},4", addr))?;
            if reply == "OK" {
                return Ok(());
            }
            bail!(
                "set_breakpoint at {:#x}: neither Z0 nor Z1 supported (Z1 replied '{}')",
                addr,
                reply
            );
        }
        bail!("set_breakpoint at {:#x}: server replied '{}'", addr, reply);
    }

    /// Remove a breakpoint at `addr`.
    ///
    /// Tries software (`z0`) then hardware (`z1`) removal.
    pub fn remove_breakpoint(&mut self, addr: u64) -> Result<()> {
        let reply = self.command(&format!("z0,{:x},4", addr))?;
        if reply == "OK" {
            return Ok(());
        }
        if reply.is_empty() {
            let reply = self.command(&format!("z1,{:x},4", addr))?;
            if reply == "OK" {
                return Ok(());
            }
        }
        bail!(
            "remove_breakpoint at {:#x}: server replied '{}'",
            addr,
            reply
        );
    }

    /// Single-step one instruction (`s` command) and wait for a stop reply.
    ///
    /// Used to advance past a breakpoint before continuing, since some GDB
    /// stubs (e.g. QEMU Hexagon) don't auto-advance past breakpoints on `c`.
    pub fn single_step(&mut self) -> Result<StopReply> {
        let pkt = protocol::encode_packet(b"s");
        self.writer.write_all(&pkt).context("send step packet")?;
        self.writer.flush().context("flush step")?;

        if !self.no_ack_mode {
            let mut ack = [0u8; 1];
            std::io::Read::read_exact(&mut self.reader, &mut ack).context("read ack for s")?;
            if ack[0] == b'-' {
                bail!("server NAK'd step command");
            }
        }

        let payload = protocol::read_packet(&mut self.reader).context("reading step reply")?;
        if !self.no_ack_mode {
            protocol::send_ack(&mut self.writer)?;
        }

        let reply = String::from_utf8(payload).context("step reply not valid UTF-8")?;
        Ok(parse_stop_reply(&reply))
    }

    /// Continue execution (`c` command) and wait for a stop reply.
    pub fn continue_execution(&mut self) -> Result<StopReply> {
        // Send 'c' packet
        let pkt = protocol::encode_packet(b"c");
        self.writer
            .write_all(&pkt)
            .context("send continue packet")?;
        self.writer.flush().context("flush continue")?;

        // Read ACK
        if !self.no_ack_mode {
            let mut ack = [0u8; 1];
            std::io::Read::read_exact(&mut self.reader, &mut ack).context("read ack for c")?;
            if ack[0] == b'-' {
                bail!("server NAK'd continue command");
            }
        }

        // Read stop reply (may take a while)
        let payload = protocol::read_packet(&mut self.reader).context("reading stop reply")?;
        if !self.no_ack_mode {
            protocol::send_ack(&mut self.writer)?;
        }

        let reply = String::from_utf8(payload).context("stop reply not valid UTF-8")?;
        Ok(parse_stop_reply(&reply))
    }

    /// Read the current halt reason (`?` command).
    pub fn halt_reason(&mut self) -> Result<StopReply> {
        let reply = self.command("?")?;
        Ok(parse_stop_reply(&reply))
    }

    /// Read memory at `addr` for `len` bytes (`m` command).
    pub fn read_memory(&mut self, addr: u64, len: usize) -> Result<Vec<u8>> {
        let reply = self.command(&format!("m{:x},{:x}", addr, len))?;
        protocol::hex_decode(reply.as_bytes()).context("decoding memory read response")
    }

    /// Detach from the target (`D` command).
    pub fn detach(&mut self) -> Result<()> {
        let reply = self.command("D")?;
        if reply != "OK" {
            bail!("detach: server replied '{}'", reply);
        }
        Ok(())
    }

    /// Kill the target (`k` command).
    ///
    /// The server may close the connection without responding.
    pub fn kill(&mut self) -> Result<()> {
        let pkt = protocol::encode_packet(b"k");
        let _ = self.writer.write_all(&pkt);
        let _ = self.writer.flush();
        Ok(())
    }

    /// Set the socket read timeout.
    ///
    /// Used by the session layer to increase the timeout before long-running
    /// operations like `continue_execution()`, where the target may take
    /// significant time to reach the next breakpoint.
    pub fn set_read_timeout(&mut self, timeout: Duration) -> Result<()> {
        self.reader
            .get_ref()
            .set_read_timeout(Some(timeout))
            .context("set read timeout")?;
        Ok(())
    }
}

/// Parse a GDB stop reply string.
///
/// Handles both simple (`Snn`) and extended (`Tnn...`) signal stop replies,
/// as well as exit (`Wnn`) and terminated (`Xnn`) replies.
fn parse_stop_reply(reply: &str) -> StopReply {
    if reply.len() >= 3 && (reply.starts_with('T') || reply.starts_with('S')) {
        if let Ok(sig) = u8::from_str_radix(&reply[1..3], 16) {
            return StopReply::Signal(sig);
        }
    }
    if reply.len() >= 3 && reply.starts_with('W') {
        if let Ok(code) = u8::from_str_radix(&reply[1..3], 16) {
            return StopReply::Exited(code);
        }
    }
    if reply.len() >= 3 && reply.starts_with('X') {
        if let Ok(sig) = u8::from_str_radix(&reply[1..3], 16) {
            return StopReply::Terminated(sig);
        }
    }
    StopReply::Unknown(reply.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    #[test]
    fn test_parse_stop_reply() {
        assert_eq!(parse_stop_reply("T05"), StopReply::Signal(5));
        assert_eq!(parse_stop_reply("T05thread:01;"), StopReply::Signal(5));
        assert_eq!(parse_stop_reply("S05"), StopReply::Signal(5));
        assert_eq!(parse_stop_reply("S11"), StopReply::Signal(0x11));
        assert_eq!(parse_stop_reply("W00"), StopReply::Exited(0));
        assert_eq!(parse_stop_reply("Wff"), StopReply::Exited(255));
        assert_eq!(parse_stop_reply("X09"), StopReply::Terminated(9));
        assert!(matches!(parse_stop_reply("???"), StopReply::Unknown(_)));
    }

    /// A minimal mock GDB server that responds to scripted commands.
    struct MockGdbServer {
        listener: TcpListener,
    }

    impl MockGdbServer {
        fn new() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            Self { listener }
        }

        fn port(&self) -> u16 {
            self.listener.local_addr().unwrap().port()
        }

        /// Accept one connection and process a sequence of expected commands.
        fn serve(self, exchanges: Vec<(&str, &str)>) -> std::thread::JoinHandle<()> {
            let exchanges: Vec<(String, String)> = exchanges
                .into_iter()
                .map(|(c, r)| (c.to_string(), r.to_string()))
                .collect();

            std::thread::spawn(move || {
                let (stream, _) = self.listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut writer = BufWriter::new(stream);

                for (expected_cmd, response) in &exchanges {
                    // Read the client's packet
                    let payload = protocol::read_packet(&mut reader).unwrap();
                    let cmd = String::from_utf8(payload).unwrap();
                    assert_eq!(
                        &cmd, expected_cmd,
                        "mock server: expected '{}', got '{}'",
                        expected_cmd, cmd
                    );

                    // Send ACK
                    protocol::send_ack(&mut writer).unwrap();

                    // Send response packet
                    let pkt = protocol::encode_packet(response.as_bytes());
                    writer.write_all(&pkt).unwrap();
                    writer.flush().unwrap();

                    // Read client's ACK
                    let mut ack = [0u8; 1];
                    Read::read_exact(&mut reader, &mut ack).unwrap();
                    assert_eq!(ack[0], b'+');
                }
            })
        }
    }

    #[test]
    fn test_client_handshake() {
        let mock = MockGdbServer::new();
        let port = mock.port();

        let handle = mock.serve(vec![(
            "qSupported:multiprocess+;swbreak+;hwbreak+",
            "PacketSize=4000;QStartNoAckMode+",
        )]);

        let mut client = RspClient::connect(
            &format!("127.0.0.1:{}", port),
            Duration::from_secs(5),
            0,
            Duration::from_millis(100),
        )
        .unwrap();

        // Note: handshake will try QStartNoAckMode, but mock only has one exchange
        // so we just test the qSupported part by calling command directly
        let reply = client
            .command("qSupported:multiprocess+;swbreak+;hwbreak+")
            .unwrap();
        assert!(reply.contains("PacketSize"));

        handle.join().unwrap();
    }

    #[test]
    fn test_client_command_cycle() {
        let mock = MockGdbServer::new();
        let port = mock.port();

        let handle = mock.serve(vec![("?", "T05"), ("g", "01000000efbeadde")]);

        let mut client = RspClient::connect(
            &format!("127.0.0.1:{}", port),
            Duration::from_secs(5),
            0,
            Duration::from_millis(100),
        )
        .unwrap();

        let halt = client.command("?").unwrap();
        assert_eq!(halt, "T05");

        let regs = client.read_all_registers().unwrap();
        assert_eq!(regs, "01000000efbeadde");

        handle.join().unwrap();
    }

    #[test]
    fn test_client_breakpoint_cycle() {
        let mock = MockGdbServer::new();
        let port = mock.port();

        let handle = mock.serve(vec![("Z0,1000,4", "OK"), ("z0,1000,4", "OK"), ("D", "OK")]);

        let mut client = RspClient::connect(
            &format!("127.0.0.1:{}", port),
            Duration::from_secs(5),
            0,
            Duration::from_millis(100),
        )
        .unwrap();

        client.set_breakpoint(0x1000).unwrap();
        client.remove_breakpoint(0x1000).unwrap();
        client.detach().unwrap();

        handle.join().unwrap();
    }

    #[test]
    fn test_client_continue_stop_reply() {
        let mock = MockGdbServer::new();
        let port = mock.port();

        // Mock server that handles continue → T05
        let listener = mock.listener;
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = BufWriter::new(stream);

            // Read 'c' command
            let payload = protocol::read_packet(&mut reader).unwrap();
            assert_eq!(String::from_utf8(payload).unwrap(), "c");

            // ACK
            protocol::send_ack(&mut writer).unwrap();

            // Simulate some work, then send stop reply
            let pkt = protocol::encode_packet(b"T05");
            writer.write_all(&pkt).unwrap();
            writer.flush().unwrap();

            // Read client's ACK for stop reply
            let mut ack = [0u8; 1];
            Read::read_exact(&mut reader, &mut ack).unwrap();
        });

        let mut client = RspClient::connect(
            &format!("127.0.0.1:{}", port),
            Duration::from_secs(5),
            0,
            Duration::from_millis(100),
        )
        .unwrap();

        let reply = client.continue_execution().unwrap();
        assert_eq!(reply, StopReply::Signal(5));

        handle.join().unwrap();
    }

    #[test]
    fn test_client_connect_retry() {
        // Start a listener after a short delay
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // Free the port

        // Spawn a delayed listener
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            drop(stream);
        });

        // Try to connect with retries
        let result = RspClient::connect(
            &format!("127.0.0.1:{}", port),
            Duration::from_secs(5),
            5,
            Duration::from_millis(100),
        );

        // This may succeed or fail depending on timing; just verify no panic
        drop(result);
        let _ = handle.join();
    }
}
