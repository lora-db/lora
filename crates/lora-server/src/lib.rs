//! HTTP transport layer for the Lora database.
//!
//! This crate is intentionally thin: it wraps anything that implements
//! [`lora_database::QueryRunner`] (typically a [`lora_database::Database`])
//! with an Axum router exposing `/health` and `/query`.

pub mod app;
pub mod config;

pub use app::{build_app, serve, ErrorResponse, HealthResponse, QueryFormat, QueryRequest};
pub use config::{ConfigError, ConfigOutcome, ServerConfig};
