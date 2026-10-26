//! Static `--help` and `--version` text.

use super::{
    ADMIN_PATH_ROOT_ENV, DEFAULT_HEADER_READ_TIMEOUT_MS, DEFAULT_HOST, DEFAULT_MAX_BODY_BYTES,
    DEFAULT_MAX_RESPONSE_BYTES, DEFAULT_MAX_RESULT_ROWS, DEFAULT_PORT, DEFAULT_QUERY_TIMEOUT_MS,
    HEADER_READ_TIMEOUT_MS_ENV, HOST_ENV, MAX_BODY_BYTES_ENV, MAX_CONCURRENT_QUERIES_ENV,
    MAX_RESPONSE_BYTES_ENV, MAX_RESULT_ROWS_ENV, PORT_ENV, QUERY_TIMEOUT_MS_ENV, SNAPSHOT_PATH_ENV,
    WAL_DIR_ENV, WAL_SYNC_MODE_ENV, WAL_TRUNCATE_CORRUPT_TAIL_ENV,
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
        --wal-truncate-corrupt-tail
                                   Incident recovery only. When the WAL is
                                   corrupt mid-log (LORA_WAL_CORRUPTION),
                                   cut it at the first corrupt record and
                                   start with the writes before it instead
                                   of refusing to start. The cut bytes and
                                   any later segments are kept in the WAL
                                   directory; writes after the cut are not
                                   recovered. Off by default. Also read from
                                   ${WAL_TRUNCATE_CORRUPT_TAIL_ENV} (true/false).
        --query-timeout-ms <MS>    Time budget for each /query and /profile
                                   request, including any wait for a free
                                   query slot. A query that runs out of time,
                                   or whose client disconnects, is cancelled
                                   and answered with LORA_TIMEOUT (HTTP 408).
                                   0 disables the timeout.
                                   Default: {DEFAULT_QUERY_TIMEOUT_MS}. Also read from ${QUERY_TIMEOUT_MS_ENV}.
        --max-concurrent-queries <N>
                                   How many /query, /explain and /profile
                                   requests run at once; the rest wait for a
                                   slot. 0 means unlimited.
                                   Default: twice the CPU count.
                                   Also read from ${MAX_CONCURRENT_QUERIES_ENV}.
        --max-body-bytes <BYTES>   Largest accepted request body.
                                   Default: {DEFAULT_MAX_BODY_BYTES}. Also read from ${MAX_BODY_BYTES_ENV}.
        --max-result-rows <N>      Most rows a /query result may have. A
                                   query that goes over stops there, any
                                   write it made is rolled back, and it is
                                   answered with LORA_RESULT_TOO_LARGE
                                   (HTTP 422). 0 means unlimited.
                                   Default: {DEFAULT_MAX_RESULT_ROWS}. Also read from ${MAX_RESULT_ROWS_ENV}.
        --max-response-bytes <BYTES>
                                   Largest /query response body. A larger
                                   response is answered with
                                   LORA_RESULT_TOO_LARGE (HTTP 422); a write
                                   behind it stays committed. 0 means
                                   unlimited.
                                   Default: {DEFAULT_MAX_RESPONSE_BYTES}. Also read from ${MAX_RESPONSE_BYTES_ENV}.
        --header-read-timeout-ms <MS>
                                   Time a connection gets to send a
                                   request's headers before it is closed,
                                   so slow or stalled clients cannot hold
                                   connections open. 0 disables it.
                                   Default: {DEFAULT_HEADER_READ_TIMEOUT_MS}. Also read from ${HEADER_READ_TIMEOUT_MS_ENV}.
        --admin-path-root <DIR>    Allow admin requests to send a `path`
                                   override, as long as it stays inside DIR.
                                   Without it, overrides are rejected and the
                                   admin routes only use --snapshot-path.
                                   Also read from ${ADMIN_PATH_ROOT_ENV}.
        --help                     Print this help and exit.
        --version                  Print version and exit.

ENVIRONMENT:
    {HOST_ENV}            Bind address (overridden by --host).
    {PORT_ENV}            TCP port      (overridden by --port).
    {SNAPSHOT_PATH_ENV}   Path used by --snapshot-path.
    {WAL_DIR_ENV}         Directory used by --wal-dir.
    {WAL_SYNC_MODE_ENV}   Mode used by --wal-sync-mode.
    {QUERY_TIMEOUT_MS_ENV}         Value used by --query-timeout-ms.
    {MAX_CONCURRENT_QUERIES_ENV}   Value used by --max-concurrent-queries.
    {MAX_BODY_BYTES_ENV}           Value used by --max-body-bytes.
    {MAX_RESULT_ROWS_ENV}          Value used by --max-result-rows.
    {MAX_RESPONSE_BYTES_ENV}       Value used by --max-response-bytes.
    {ADMIN_PATH_ROOT_ENV}          Directory used by --admin-path-root.
    {HEADER_READ_TIMEOUT_MS_ENV}   Value used by --header-read-timeout-ms.
    {WAL_TRUNCATE_CORRUPT_TAIL_ENV} Value used by --wal-truncate-corrupt-tail.

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
