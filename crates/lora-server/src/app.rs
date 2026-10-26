use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder as ConnectionBuilder;
use hyper_util::service::TowerToHyperService;
use lora_database::QueryRunner;
use tokio::net::{TcpListener, TcpStream};

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

/// Default cap on the rows of one `/query` result (`--max-result-rows`).
pub const DEFAULT_MAX_RESULT_ROWS: usize = 1_000_000;

/// Default cap on one `/query` response body (`--max-response-bytes`).
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 256 * 1024 * 1024;

/// Default time a client gets to send a request's headers
/// (`--header-read-timeout-ms`).
pub const DEFAULT_HEADER_READ_TIMEOUT_MS: u64 = 10_000;

/// Default number of engine calls that run at once: twice the CPU count,
/// so reads keep every core busy while a few requests wait on I/O or the
/// writer lock.
pub fn default_max_concurrent_queries() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get() * 2)
        .unwrap_or(8)
}

/// Resource limits applied to the query routes (`/query`, `/explain`,
/// `/profile`), plus the per-connection header read timeout.
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
    /// Most rows a `/query` result may have. The engine checks the cap as
    /// it builds the result and stops there, so a huge result is never
    /// built; the request fails with `LORA_RESULT_TOO_LARGE` and a write
    /// rolls back. `None` means unlimited.
    pub max_result_rows: Option<usize>,
    /// Largest `/query` response body, in bytes. The response is encoded
    /// against the cap and dropped once it goes over, failing with
    /// `LORA_RESULT_TOO_LARGE`. The query has finished by then, so a write
    /// behind it stays committed. `None` means unlimited.
    pub max_response_bytes: Option<usize>,
    /// How long a connection may take to send a request's headers before
    /// it is closed, so clients that open connections and trickle bytes
    /// (slowloris) cannot hold them. Used by [`serve_with`]; `None`
    /// disables it.
    pub header_read_timeout: Option<Duration>,
}

impl Default for QueryLimits {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_millis(DEFAULT_QUERY_TIMEOUT_MS)),
            max_concurrent: Some(default_max_concurrent_queries()),
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            max_result_rows: Some(DEFAULT_MAX_RESULT_ROWS),
            max_response_bytes: Some(DEFAULT_MAX_RESPONSE_BYTES),
            header_read_timeout: Some(Duration::from_millis(DEFAULT_HEADER_READ_TIMEOUT_MS)),
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

/// Serve the router on `listener` until the process exits.
///
/// This is `axum::serve` with hyper's header read timeout turned on
/// (`limits.header_read_timeout`), which `axum::serve` does not expose:
/// each accepted connection is served by its own task through hyper-util,
/// with the same HTTP/1 settings and upgrade support otherwise.
pub async fn serve_with<R>(
    listener: TcpListener,
    db: Arc<R>,
    limits: QueryLimits,
    admin_config: Option<AdminConfig>,
) -> Result<()>
where
    R: QueryRunner,
{
    let header_read_timeout = limits.header_read_timeout;
    let app = build_app_with(db, limits, admin_config);
    loop {
        let Some(stream) = accept(&listener).await else {
            continue;
        };
        let service = TowerToHyperService::new(app.clone());
        tokio::spawn(async move {
            let mut builder = ConnectionBuilder::new(TokioExecutor::new());
            builder
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(header_read_timeout);
            // An error here means the client went away or broke the
            // protocol (including running out of header read time); the
            // connection is closed either way.
            let _ = builder
                .serve_connection_with_upgrades(TokioIo::new(stream), service)
                .await;
        });
    }
}

/// Accept the next connection. A failure tied to one connection is
/// skipped; any other failure (such as running out of file descriptors)
/// is logged and retried after a second, as `axum::serve` does.
async fn accept(listener: &TcpListener) -> Option<TcpStream> {
    match listener.accept().await {
        Ok((stream, _)) => Some(stream),
        Err(err) => {
            if !matches!(
                err.kind(),
                std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::ConnectionReset
            ) {
                tracing::error!("accept error: {err}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            None
        }
    }
}
