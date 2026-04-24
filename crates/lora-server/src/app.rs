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
use lora_database::{ExecuteOptions, QueryRunner, ResultFormat, SnapshotAdmin};
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

/// Configuration for the admin surface. When present, `build_app_with_admin`
/// (or `serve_with_admin`) mounts `POST /admin/snapshot/save` and
/// `POST /admin/snapshot/load` that drive the configured admin handle.
///
/// The endpoints are intentionally opt-in: exposing them without
/// authentication on a network-reachable interface is a footgun, so the
/// caller must explicitly construct an `AdminConfig` and pass it to the
/// server — there is no implicit default path.
#[derive(Clone)]
pub struct AdminConfig {
    pub snapshot_path: PathBuf,
    pub admin: Arc<dyn SnapshotAdmin>,
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
    Router::new()
        .route("/admin/snapshot/save", post(admin_snapshot_save))
        .route("/admin/snapshot/load", post(admin_snapshot_load))
        .with_state(cfg)
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
fn resolve_snapshot_path(cfg: &AdminConfig, req: Option<&SnapshotRequest>) -> PathBuf {
    match req.and_then(|r| r.path.as_deref()) {
        Some(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => cfg.snapshot_path.clone(),
    }
}

async fn admin_snapshot_save(
    State(cfg): State<AdminConfig>,
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
    State(cfg): State<AdminConfig>,
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
