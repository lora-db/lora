//! Static `--help` and `--version` text.

use super::{
    DEFAULT_HOST, DEFAULT_PORT, HOST_ENV, PORT_ENV, SNAPSHOT_PATH_ENV, WAL_DIR_ENV,
    WAL_SYNC_MODE_ENV,
};

pub fn help_text() -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!(
        "lora-server {version} — HTTP server for the Lora in-memory graph database

USAGE:
    lora-server [OPTIONS]

OPTIONS:
        --host <HOST>              Bind address. Default: {DEFAULT_HOST} (or ${HOST_ENV} if set).
        --port <PORT>              TCP port.      Default: {DEFAULT_PORT} (or ${PORT_ENV} if set).
        --snapshot-path <PATH>     Enable the snapshot admin surface. Mounts
                                   POST /admin/snapshot/save and
                                   POST /admin/snapshot/load against this file.
                                   Also acts as the default target for
                                   POST /admin/checkpoint when --wal-dir is set.
                                   Also read from ${SNAPSHOT_PATH_ENV}.
        --restore-from <PATH>      Restore the graph from this snapshot at boot.
                                   Missing file is treated as empty. When
                                   --wal-dir is also set, the WAL is replayed
                                   on top of the snapshot.
        --wal-dir <DIR>            Attach a write-ahead log at this directory.
                                   Every mutating query is bracketed by
                                   begin/commit; a crashed process recovers
                                   committed writes on next boot. Read-only
                                   queries do not touch the WAL.
                                   Also enables the WAL admin routes
                                   (POST /admin/wal/status,
                                    POST /admin/wal/truncate,
                                    POST /admin/checkpoint) — independent of
                                   --snapshot-path. /admin/checkpoint requires
                                   `path` in the request body when no
                                   --snapshot-path default is configured.
                                   Also read from ${WAL_DIR_ENV}.
        --wal-sync-mode <MODE>     WAL durability cadence. Only `group-sync`
                                   is supported (default).
                                   Also read from ${WAL_SYNC_MODE_ENV}.
        --help                     Print this help and exit.
        --version                  Print version and exit.

ENVIRONMENT:
    {HOST_ENV}            Bind address (overridden by --host).
    {PORT_ENV}            TCP port      (overridden by --port).
    {SNAPSHOT_PATH_ENV}   Path used by --snapshot-path.
    {WAL_DIR_ENV}         Directory used by --wal-dir.
    {WAL_SYNC_MODE_ENV}   Mode used by --wal-sync-mode.

EXAMPLES:
    lora-server
    lora-server --host 0.0.0.0 --port 8080
    lora-server --snapshot-path /var/lib/lora/graph.bin
    lora-server --wal-dir /var/lib/lora/wal --snapshot-path /var/lib/lora/graph.bin \\
                --restore-from /var/lib/lora/graph.bin
"
    )
}

pub fn version_text() -> String {
    format!("lora-server {}", env!("CARGO_PKG_VERSION"))
}
