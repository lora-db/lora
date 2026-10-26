use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use lora_database::QueryRunner;

mod admin;
mod errors;
mod routes;
mod types;

pub use admin::{AdminConfig, SnapshotAdminConfig};
pub use errors::ErrorResponse;
pub use types::{
    HealthResponse, QueryFormat, QueryRequest, SnapshotRequest, SnapshotResponse,
    WalStatusResponse, WalTruncateRequest,
};

/// Default per-request query budget (`--query-timeout-ms`).
pub const DEFAULT_QUERY_TIMEOUT_MS: u64 = 30_000;

/// Default request body cap (`--max-body-bytes`); matches axum's own
/// default so the limit is unchanged unless an operator raises it.
pub const DEFAULT_MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Default number of engine calls that run at once: twice the CPU count,
/// so reads keep every core busy while a few requests wait on I/O or the
/// writer lock.
pub fn default_max_concurrent_queries() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get() * 2)
        .unwrap_or(8)
}

/// Resource limits applied to the query routes (`/query`, `/explain`,
/// `/profile`).
///
/// Every engine call runs on tokio's blocking pool, so a slow query never
/// stalls the async workers that accept connections and answer `/health`.
#[derive(Debug, Clone)]
pub struct QueryLimits {
    /// Budget for one request, including the wait for a free slot. When it
    /// runs out the query is cancelled and the request fails with
    /// `LORA_TIMEOUT`. `None` disables the timeout. A query whose client
    /// disconnects is cancelled either way.
    pub timeout: Option<Duration>,
    /// How many engine calls run at once; further requests wait for a
    /// slot. `None` means unlimited.
    pub max_concurrent: Option<usize>,
    /// Largest accepted request body, in bytes.
    pub max_body_bytes: usize,
}

impl Default for QueryLimits {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_millis(DEFAULT_QUERY_TIMEOUT_MS)),
            max_concurrent: Some(default_max_concurrent_queries()),
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        }
    }
}

/// [`build_app_with`] using [`QueryLimits::default`] and no admin routes.
pub fn build_app<R>(db: Arc<R>) -> Router
where
    R: QueryRunner,
{
    build_app_with(db, QueryLimits::default(), None)
}

pub async fn serve<R>(listener: tokio::net::TcpListener, db: Arc<R>) -> Result<()>
where
    R: QueryRunner,
{
    serve_with(listener, db, QueryLimits::default(), None).await
}

/// Same as [`build_app`] but additionally mounts the admin routes when
/// `admin_config` is `Some`.
pub fn build_app_with_admin<R>(db: Arc<R>, admin_config: Option<AdminConfig>) -> Router
where
    R: QueryRunner,
{
    build_app_with(db, QueryLimits::default(), admin_config)
}

/// Build the router with explicit [`QueryLimits`] and optional admin
/// routes.
pub fn build_app_with<R>(
    db: Arc<R>,
    limits: QueryLimits,
    admin_config: Option<AdminConfig>,
) -> Router
where
    R: QueryRunner,
{
    let body_limit = limits.max_body_bytes;
    let router = Router::new()
        .route("/health", get(routes::health))
        .route("/query", post(routes::query::<R>))
        .route("/explain", post(routes::explain::<R>))
        .route("/profile", post(routes::profile::<R>))
        .with_state(routes::QueryState::new(db, &limits));
    let router = match admin_config {
        Some(cfg) => router.merge(admin::build_admin_router(cfg)),
        None => router,
    };
    router.layer(DefaultBodyLimit::max(body_limit))
}

pub async fn serve_with_admin<R>(
    listener: tokio::net::TcpListener,
    db: Arc<R>,
    admin_config: Option<AdminConfig>,
) -> Result<()>
where
    R: QueryRunner,
{
    serve_with(listener, db, QueryLimits::default(), admin_config).await
}

pub async fn serve_with<R>(
    listener: tokio::net::TcpListener,
    db: Arc<R>,
    limits: QueryLimits,
    admin_config: Option<AdminConfig>,
) -> Result<()>
where
    R: QueryRunner,
{
    let app = build_app_with(db, limits, admin_config);
    axum::serve(listener, app).await?;
    Ok(())
}
