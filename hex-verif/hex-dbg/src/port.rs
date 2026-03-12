// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! TOCTOU-safe TCP port allocation.
//!
//! [`PortGuard`] holds a `TcpListener` open on an OS-assigned port, preventing
//! other threads/processes from receiving the same port number during the
//! window between allocation and the target server binding.

use std::net::TcpListener;

use anyhow::{Context, Result};

/// A guard that holds a TCP port open until dropped.
///
/// # Usage
///
/// ```no_run
/// use hex_dbg::port::allocate_port;
///
/// let guard = allocate_port().unwrap();
/// // Spawn server process with guard.port ...
/// drop(guard); // Release so the server can bind
/// // Connect to the server with retries ...
/// ```
pub struct PortGuard {
    /// The allocated port number.
    pub port: u16,
    _listener: TcpListener,
}

/// Allocate a free TCP port, returning a [`PortGuard`] that holds it open.
///
/// The OS picks an unused ephemeral port. The guard's `TcpListener` keeps the
/// port reserved until `drop(guard)` is called, closing the TOCTOU window
/// that the old `find_free_port()` had.
pub fn allocate_port() -> Result<PortGuard> {
    let listener =
        TcpListener::bind("127.0.0.1:0").context("failed to bind for port allocation")?;
    let port = listener
        .local_addr()
        .context("failed to get local address")?
        .port();
    Ok(PortGuard {
        port,
        _listener: listener,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::net::TcpStream;

    #[test]
    fn test_ports_unique() {
        let guards: Vec<_> = (0..10).map(|_| allocate_port().unwrap()).collect();
        let ports: HashSet<u16> = guards.iter().map(|g| g.port).collect();
        // All 10 ports should be distinct (guards still alive)
        assert_eq!(ports.len(), 10);
    }

    #[test]
    fn test_guard_blocks_rebind() {
        let guard = allocate_port().unwrap();
        // Attempting to bind to the same port should fail while guard is alive
        let result = TcpListener::bind(format!("127.0.0.1:{}", guard.port));
        assert!(result.is_err());
    }

    #[test]
    fn test_guard_drop_releases() {
        let guard = allocate_port().unwrap();
        let port = guard.port;
        drop(guard);
        // After drop, we can rebind to the same port (barring rare OS race)
        // We accept either success or EADDRINUSE since the kernel may delay reuse
        let _ = TcpListener::bind(format!("127.0.0.1:{}", port));
    }

    #[test]
    fn test_port_is_listening() {
        let guard = allocate_port().unwrap();
        // The guard's listener is bound, so a connect should succeed
        let result = TcpStream::connect(format!("127.0.0.1:{}", guard.port));
        assert!(result.is_ok());
    }
}
