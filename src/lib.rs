//! Shared protocol and client helpers for the mpris-controller binaries.
//!
//! IPC is newline-delimited JSON over a unix socket: the client sends one
//! [`Request`] per line and receives one [`Response`] per line. After a
//! `subscribe` request the server keeps sending a `state` response every time
//! anything changes.

pub mod client;
pub mod protocol;

use std::path::PathBuf;

/// Location of the daemon socket (`$XDG_RUNTIME_DIR/mpris-controller.sock`).
pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join("mpris-controller.sock")
}
