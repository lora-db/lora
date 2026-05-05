use std::sync::Arc;

use anyhow::Result;
use axum::{
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

pub fn build_app<R>(db: Arc<R>) -> Router
where
    R: QueryRunner,
{
    Router::new()
        .route("/health", get(routes::health))
        .route("/query", post(routes::query::<R>))
        .route("/explain", post(routes::explain::<R>))
        .route("/profile", post(routes::profile::<R>))
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

/// Same as [`build_app`] but additionally mounts the admin routes when
/// `admin_config` is `Some`.
pub fn build_app_with_admin<R>(db: Arc<R>, admin_config: Option<AdminConfig>) -> Router
where
    R: QueryRunner,
{
    let router = build_app(db);
    match admin_config {
        Some(cfg) => router.merge(admin::build_admin_router(cfg)),
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
