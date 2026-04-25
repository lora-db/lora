//! HTTP transport layer for the Lora database.
//!
//! This crate is intentionally thin: it wraps anything that implements
//! [`lora_database::QueryRunner`] (typically a [`lora_database::Database`])
//! with an Axum router exposing `/health` and `/query`.

pub mod app;
pub mod config;

pub use app::{
    build_app, build_app_with_admin, serve, serve_with_admin, AdminConfig, ErrorResponse,
    HealthResponse, QueryFormat, QueryRequest, SnapshotAdminConfig, SnapshotRequest,
    SnapshotResponse, WalStatusResponse, WalTruncateRequest,
};
pub use config::{ConfigError, ConfigOutcome, ServerConfig};
