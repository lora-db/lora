use lora_database::ResultFormat;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    pub query: String,
    #[serde(default)]
    pub format: Option<QueryFormat>,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// Request body for `POST /explain` and `POST /profile`. Mirrors
/// [`QueryRequest`] without a `format` field — plan and profile
/// payloads have a fixed shape.
#[derive(Debug, Deserialize)]
pub struct PlanRequest {
    pub query: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QueryFormat {
    Rows,
    RowArrays,
    Graph,
    Combined,
}

impl From<QueryFormat> for ResultFormat {
    fn from(value: QueryFormat) -> Self {
        match value {
            QueryFormat::Rows => ResultFormat::Rows,
            QueryFormat::RowArrays => ResultFormat::RowArrays,
            QueryFormat::Graph => ResultFormat::Graph,
            QueryFormat::Combined => ResultFormat::Combined,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

/// Request body for `POST /admin/snapshot/{save,load}`. The body is
/// optional; when it is absent (or an empty JSON object) the server uses
/// the path configured in `AdminConfig`.
///
/// A `path` override is only accepted when the server was started with an
/// admin path root (`--admin-path-root`), and must stay inside it. **The
/// admin surface has no authentication: any client that can reach it can
/// overwrite the snapshot, replace the live graph, and truncate the WAL.
/// Deploy it behind authenticated transport only.**
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SnapshotRequest {
    /// Override the configured snapshot path for this request only.
    pub path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SnapshotResponse {
    #[serde(rename = "formatVersion")]
    pub format_version: u32,
    #[serde(rename = "nodeCount")]
    pub node_count: u64,
    #[serde(rename = "relationshipCount")]
    pub relationship_count: u64,
    #[serde(rename = "walLsn")]
    pub wal_lsn: Option<u64>,
    pub path: String,
}

/// Body for `POST /admin/wal/truncate`. Operators supply the LSN past
/// which sealed segments may be deleted; the WAL truncates everything
/// at or below that point. Active and tombstone segments are always
/// retained.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct WalTruncateRequest {
    #[serde(rename = "fenceLsn")]
    pub fence_lsn: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct WalStatusResponse {
    #[serde(rename = "durableLsn")]
    pub durable_lsn: u64,
    #[serde(rename = "nextLsn")]
    pub next_lsn: u64,
    #[serde(rename = "activeSegmentId")]
    pub active_segment_id: u64,
    #[serde(rename = "oldestSegmentId")]
    pub oldest_segment_id: u64,
    /// Latched fsync error from the bg flusher (only populated under
    /// `SyncMode::GroupSync`). `None` when healthy.
    #[serde(rename = "bgFailure")]
    pub bg_failure: Option<String>,
}
