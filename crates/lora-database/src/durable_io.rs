//! Filesystem durability helpers for snapshot/archive integration.
//!
//! Native builds call through to `std` fsync primitives. The wasm binding is
//! in-memory and `wasm32-unknown-unknown` has no filesystem durability
//! boundary, so these helpers intentionally no-op there instead of scattering
//! target-specific cfgs through snapshot and archive code.

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
