//! Filesystem durability helpers for WAL internals.
//!
//! Native builds use the strongest portable `std` primitives available here.
//! `wasm32-unknown-unknown` does not expose a real filesystem durability
//! boundary, so fsync helpers intentionally degrade to no-ops while keeping the
//! higher-level WAL state machine usable for code that is compiled into the
//! wasm binding but never opens a filesystem-backed WAL.

use std::fs::File;
use std::io;
use std::path::Path;

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) fn sync_file(file: &File) -> io::Result<()> {
    file.sync_all()
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) fn sync_file(_file: &File) -> io::Result<()> {
    Ok(())
}

#[cfg(all(unix, not(all(target_arch = "wasm32", target_os = "unknown"))))]
pub(crate) fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(all(unix, not(all(target_arch = "wasm32", target_os = "unknown")))))]
pub(crate) fn sync_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}
