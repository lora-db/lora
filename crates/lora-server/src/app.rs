use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use lora_database::{ExecuteOptions, QueryRunner, ResultFormat, SnapshotAdmin, WalAdmin};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    pub query: String,
    #[serde(default)]
    pub format: Option<QueryFormat>,
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
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

pub fn build_app<R>(db: Arc<R>) -> Router
where
    R: QueryRunner,
{
    Router::new()
        .route("/health", get(health))
        .route("/query", post(query::<R>))
        .with_state(db)
}

pub async fn serve<R>(listener: tokio::net::TcpListener, db: Arc<R>) -> Result<()>
where
    R: QueryRunner,
{
    let app = build_app(db);
    axum::serve(listener, app).await?;
    Ok(())
}

/// Snapshot admin surface. Mounted as a unit so that
/// `/admin/snapshot/{save,load}` always have a configured default
/// path: an operator who set `--snapshot-path` is the one paying the
/// cost of the route's existence, and they reasonably expect the
/// path to be resolved automatically when no `path` field is sent in
/// the request body.
#[derive(Clone)]
pub struct SnapshotAdminConfig {
    pub path: PathBuf,
    pub admin: Arc<dyn SnapshotAdmin>,
}

/// Configuration for the admin surface. Snapshot and WAL admin are
/// independent: each set of routes mounts only when its corresponding
/// field is `Some`.
///
/// - `snapshot.is_some()` mounts `POST /admin/snapshot/save` and
///   `POST /admin/snapshot/load` against the configured path
///   (the body's optional `path` field overrides per request).
/// - `wal.is_some()` mounts `POST /admin/wal/status` and
///   `POST /admin/wal/truncate` unconditionally, plus
///   `POST /admin/checkpoint` (which uses `snapshot.path` as a default
///   when present, and otherwise requires `path` in the request body).
///
/// The endpoints are intentionally opt-in: exposing them without
/// authentication on a network-reachable interface is a footgun, so
/// the caller must explicitly construct an `AdminConfig` and pass it
/// to the server — there is no implicit default path.
#[derive(Clone, Default)]
pub struct AdminConfig {
    /// Snapshot save/load admin. `None` to disable
    /// `/admin/snapshot/{save,load}`.
    pub snapshot: Option<SnapshotAdminConfig>,
    /// WAL admin. `None` to disable `/admin/wal/*` and
    /// `/admin/checkpoint`.
    pub wal: Option<Arc<dyn WalAdmin>>,
}

impl AdminConfig {
    /// Construct a snapshot-only admin config (no WAL endpoints).
    pub fn snapshot_only(snapshot_path: PathBuf, admin: Arc<dyn SnapshotAdmin>) -> Self {
        Self {
            snapshot: Some(SnapshotAdminConfig {
                path: snapshot_path,
                admin,
            }),
            wal: None,
        }
    }

    /// Construct a WAL-only admin config (no snapshot endpoints). The
    /// `/admin/checkpoint` route still mounts but every call needs a
    /// `path` in the request body since there is no configured
    /// default.
    pub fn wal_only(wal: Arc<dyn WalAdmin>) -> Self {
        Self {
            snapshot: None,
            wal: Some(wal),
        }
    }

    /// True when neither admin surface is configured. The router
    /// merge then becomes a no-op and the admin routes don't exist.
    pub fn is_empty(&self) -> bool {
        self.snapshot.is_none() && self.wal.is_none()
    }
}

/// Same as [`build_app`] but additionally mounts the admin routes when
/// `admin_config` is `Some`.
pub fn build_app_with_admin<R>(db: Arc<R>, admin_config: Option<AdminConfig>) -> Router
where
    R: QueryRunner,
{
    let router = build_app(db);
    match admin_config {
        Some(cfg) => router.merge(build_admin_router(cfg)),
        None => router,
    }
}

pub async fn serve_with_admin<R>(
    listener: tokio::net::TcpListener,
    db: Arc<R>,
    admin_config: Option<AdminConfig>,
) -> Result<()>
where
    R: QueryRunner,
{
    let app = build_app_with_admin(db, admin_config);
    axum::serve(listener, app).await?;
    Ok(())
}

fn build_admin_router(cfg: AdminConfig) -> Router {
    let mut router = Router::new();

    if let Some(snap) = cfg.snapshot.clone() {
        let snapshot_router: Router = Router::new()
            .route("/admin/snapshot/save", post(admin_snapshot_save))
            .route("/admin/snapshot/load", post(admin_snapshot_load))
            .with_state(snap);
        router = router.merge(snapshot_router);
    }

    if let Some(wal) = cfg.wal.clone() {
        let wal_state = WalAdminState {
            // Reuse the snapshot path as the default checkpoint
            // target when present so a body-less
            // `POST /admin/checkpoint` writes to the same file the
            // snapshot endpoints use. When no snapshot path is
            // configured, the handler requires `path` in the body.
            default_checkpoint_path: cfg.snapshot.as_ref().map(|s| s.path.clone()),
            wal,
        };
        let wal_router: Router = Router::new()
            .route("/admin/checkpoint", post(admin_checkpoint))
            .route("/admin/wal/status", post(admin_wal_status))
            .route("/admin/wal/truncate", post(admin_wal_truncate))
            .with_state(wal_state);
        router = router.merge(wal_router);
    }

    router
}

/// State plumbed into the WAL admin handlers.
#[derive(Clone)]
struct WalAdminState {
    /// Default target for `POST /admin/checkpoint` when the body
    /// omits `path`. `None` when the operator did not pass
    /// `--snapshot-path`; in that case the handler returns 400 with
    /// a hint.
    default_checkpoint_path: Option<PathBuf>,
    wal: Arc<dyn WalAdmin>,
}

/// Request body for `POST /admin/snapshot/{save,load}`. The body is
/// optional; when it is absent (or an empty JSON object) the server uses
/// the path configured in `AdminConfig`.
///
/// Supplying a `path` override lets an operator snapshot to / restore from
/// an arbitrary filesystem location in a single request. **Any client that
/// can reach the admin surface can write to any path the server process
/// can write to — deploy the admin surface behind authenticated transport
/// only.** We deliberately do not sandbox the path here; a well-meaning
/// whitelist would give a false sense of safety without auth.
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

/// Extract the target path for a snapshot operation: the request-body
/// override if present, else the configured default.
fn resolve_snapshot_path(cfg: &SnapshotAdminConfig, req: Option<&SnapshotRequest>) -> PathBuf {
    match req.and_then(|r| r.path.as_deref()) {
        Some(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => cfg.path.clone(),
    }
}

async fn admin_snapshot_save(
    State(cfg): State<SnapshotAdminConfig>,
    body: Option<Json<SnapshotRequest>>,
) -> impl IntoResponse {
    let req = body.map(|Json(r)| r);
    let path = resolve_snapshot_path(&cfg, req.as_ref());

    match cfg.admin.save_snapshot(&path) {
        Ok(meta) => (
            StatusCode::OK,
            Json(SnapshotResponse {
                format_version: meta.format_version,
                node_count: meta.node_count as u64,
                relationship_count: meta.relationship_count as u64,
                wal_lsn: meta.wal_lsn,
                path: path.display().to_string(),
            }),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn admin_snapshot_load(
    State(cfg): State<SnapshotAdminConfig>,
    body: Option<Json<SnapshotRequest>>,
) -> impl IntoResponse {
    let req = body.map(|Json(r)| r);
    let path = resolve_snapshot_path(&cfg, req.as_ref());

    match cfg.admin.load_snapshot(&path) {
        Ok(meta) => (
            StatusCode::OK,
            Json(SnapshotResponse {
                format_version: meta.format_version,
                node_count: meta.node_count as u64,
                relationship_count: meta.relationship_count as u64,
                wal_lsn: meta.wal_lsn,
                path: path.display().to_string(),
            }),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// WAL admin endpoints (mounted only when `AdminConfig.wal` is `Some`).
// ---------------------------------------------------------------------------

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
    /// `SyncMode::Group`). `None` when healthy.
    #[serde(rename = "bgFailure")]
    pub bg_failure: Option<String>,
}

fn resolve_checkpoint_path(
    state: &WalAdminState,
    req: Option<&SnapshotRequest>,
) -> Result<PathBuf, &'static str> {
    match req.and_then(|r| r.path.as_deref()) {
        Some(p) if !p.trim().is_empty() => Ok(PathBuf::from(p)),
        _ => state
            .default_checkpoint_path
            .clone()
            .ok_or("no checkpoint path: pass `path` in the request body or start the server with --snapshot-path"),
    }
}

async fn admin_checkpoint(
    State(state): State<WalAdminState>,
    body: Option<Json<SnapshotRequest>>,
) -> impl IntoResponse {
    let req = body.map(|Json(r)| r);
    let path = match resolve_checkpoint_path(&state, req.as_ref()) {
        Ok(p) => p,
        Err(msg) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: msg.to_string(),
                }),
            )
                .into_response()
        }
    };

    match state.wal.checkpoint(&path) {
        Ok(meta) => (
            StatusCode::OK,
            Json(SnapshotResponse {
                format_version: meta.format_version,
                node_count: meta.node_count as u64,
                relationship_count: meta.relationship_count as u64,
                wal_lsn: meta.wal_lsn,
                path: path.display().to_string(),
            }),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn admin_wal_status(State(state): State<WalAdminState>) -> impl IntoResponse {
    match state.wal.wal_status() {
        Ok(s) => (
            StatusCode::OK,
            Json(WalStatusResponse {
                durable_lsn: s.durable_lsn,
                next_lsn: s.next_lsn,
                active_segment_id: s.active_segment_id,
                oldest_segment_id: s.oldest_segment_id,
                bg_failure: s.bg_failure,
            }),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn admin_wal_truncate(
    State(state): State<WalAdminState>,
    body: Option<Json<WalTruncateRequest>>,
) -> impl IntoResponse {
    // No body / no fence => truncate up to the WAL's current durable
    // LSN. That's the natural "drop everything safe to drop" default.
    let fence = match body.and_then(|Json(r)| r.fence_lsn) {
        Some(lsn) => lsn,
        None => match state.wal.wal_status() {
            Ok(s) => s.durable_lsn,
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: err.to_string(),
                    }),
                )
                    .into_response()
            }
        },
    };

    match state.wal.wal_truncate(fence) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn query<R>(State(db): State<Arc<R>>, Json(req): Json<QueryRequest>) -> impl IntoResponse
where
    R: QueryRunner,
{
    let options = req.format.map(|format| ExecuteOptions {
        format: format.into(),
    });

    match db.execute(&req.query, options) {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}
