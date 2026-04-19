use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use lora_database::{ExecuteOptions, QueryRunner, ResultFormat};
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
