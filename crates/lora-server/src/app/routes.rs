use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    extract::{rejection::JsonRejection, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use lora_database::{
    CancellableDeadline, ExecuteOptions, LoraError, LoraErrorCode, LoraValue, PlanShape,
    PlanTreeNode, QueryPlan, QueryProfile, QueryRunner,
};
use tokio::sync::Semaphore;
use web_time::Instant;

use super::errors::{json_rejection_response, lora_error_response};
use super::types::{HealthResponse, PlanRequest, QueryRequest};
use super::QueryLimits;

/// State shared by the query routes: the runner plus the limits every
/// engine call goes through (see [`run_engine`]).
pub(crate) struct QueryState<R> {
    db: Arc<R>,
    timeout: Option<Duration>,
    slots: Option<Arc<Semaphore>>,
    max_result_rows: Option<usize>,
    max_response_bytes: Option<usize>,
}

impl<R> Clone for QueryState<R> {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
            timeout: self.timeout,
            slots: self.slots.clone(),
            max_result_rows: self.max_result_rows,
            max_response_bytes: self.max_response_bytes,
        }
    }
}

impl<R> QueryState<R> {
    pub(crate) fn new(db: Arc<R>, limits: &QueryLimits) -> Self {
        Self {
            db,
            timeout: limits.timeout,
            slots: limits
                .max_concurrent
                .filter(|n| *n > 0)
                .map(|n| Arc::new(Semaphore::new(n))),
            max_result_rows: limits.max_result_rows,
            max_response_bytes: limits.max_response_bytes,
        }
    }
}

/// Cancels the query when the handler future is dropped before the engine
/// call finished, which is what happens when the client disconnects.
struct CancelOnDrop(Option<Arc<Mutex<CancellableDeadline>>>);

impl CancelOnDrop {
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.lock().unwrap_or_else(|p| p.into_inner()).cancel();
        }
    }
}

/// Run one engine call off the async workers, bounded by the configured
/// timeout and concurrency limit.
///
/// The call gets a cancellable deadline: it fails with `LORA_TIMEOUT` once
/// the timeout passes, or as soon as the request future is dropped (client
/// disconnect). The slot and the deadline registration live inside the
/// blocking task, so a cancelled query keeps its slot until the engine has
/// actually stopped.
async fn run_engine<R, T, F>(state: &QueryState<R>, call: F) -> Result<T, LoraError>
where
    R: QueryRunner,
    T: Send + 'static,
    F: FnOnce(&R, Instant) -> Result<T, LoraError> + Send + 'static,
{
    let handle = Arc::new(Mutex::new(CancellableDeadline::new(state.timeout)));
    let deadline = handle.lock().unwrap_or_else(|p| p.into_inner()).deadline();
    let guard = CancelOnDrop(Some(Arc::clone(&handle)));

    let slot = match &state.slots {
        None => None,
        Some(slots) => {
            let acquire = Arc::clone(slots).acquire_owned();
            let acquired = match state.timeout {
                None => Ok(acquire.await),
                Some(_) => {
                    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), acquire).await
                }
            };
            match acquired {
                Ok(Ok(permit)) => Some(permit),
                Ok(Err(_)) => {
                    return Err(LoraError::new(
                        LoraErrorCode::Internal,
                        "query slots closed",
                    ))
                }
                Err(_) => {
                    guard.disarm();
                    return Err(LoraError::new(
                        LoraErrorCode::Timeout,
                        "query timed out waiting for a free query slot",
                    ));
                }
            }
        }
    };

    let db = Arc::clone(&state.db);
    let task = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        let _registration = handle;
        call(&db, deadline)
    });
    let outcome = task.await;
    guard.disarm();
    match outcome {
        Ok(result) => result,
        Err(join) => Err(LoraError::new(
            LoraErrorCode::Internal,
            format!("query task failed: {join}"),
        )),
    }
}

pub(crate) async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub(crate) async fn query<R>(
    State(state): State<QueryState<R>>,
    body: Result<Json<QueryRequest>, JsonRejection>,
) -> impl IntoResponse
where
    R: QueryRunner,
{
    let req = match body {
        Ok(Json(req)) => req,
        Err(rejection) => return json_rejection_response(rejection),
    };
    let params = match parse_params(req.params) {
        Ok(p) => p,
        Err(err) => return lora_error_response(err),
    };
    let options = req.format.map(|format| ExecuteOptions {
        format: format.into(),
    });

    let query = req.query;
    let max_rows = state.max_result_rows;
    let max_bytes = state.max_response_bytes;
    // The response is encoded on the blocking pool too, against its byte
    // cap, so a large one neither holds up the async workers nor grows
    // past the cap.
    let outcome = run_engine(&state, move |db, deadline| {
        let params = params.unwrap_or_default();
        let result = match max_rows {
            Some(max_rows) => db
                .execute_with_params_deadline_max_rows(&query, options, params, deadline, max_rows),
            None => db.execute_with_params_deadline(&query, options, params, deadline),
        }?;
        encode_json(&result, max_bytes)
    })
    .await;
    match outcome {
        Ok(body) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            body,
        )
            .into_response(),
        Err(err) => lora_error_response(err),
    }
}

/// Encode `value` as JSON, failing with `LORA_RESULT_TOO_LARGE` as soon as
/// the encoding passes `max_bytes` (`None`: no cap).
fn encode_json<T: serde::Serialize>(
    value: &T,
    max_bytes: Option<usize>,
) -> Result<Vec<u8>, LoraError> {
    let mut out = CappedWriter {
        buf: Vec::new(),
        max: max_bytes.unwrap_or(usize::MAX),
        over: false,
    };
    match serde_json::to_writer(&mut out, value) {
        Ok(()) => Ok(out.buf),
        Err(_) if out.over => Err(LoraError::new(
            LoraErrorCode::ResultTooLarge,
            format!(
                "query response is larger than {} bytes; return fewer or smaller values, or page through the result with SKIP and LIMIT",
                out.max
            ),
        )),
        Err(err) => Err(LoraError::new(
            LoraErrorCode::Internal,
            format!("failed to encode the query result: {err}"),
        )),
    }
}

/// A `Vec<u8>` writer that refuses to grow past `max` bytes.
struct CappedWriter {
    buf: Vec<u8>,
    max: usize,
    over: bool,
}

impl std::io::Write for CappedWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if data.len() > self.max - self.buf.len() {
            self.over = true;
            return Err(std::io::Error::other("response byte cap reached"));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `POST /explain` — compile a query and return its plan as JSON
/// without invoking the executor. Mutating queries leave the graph
/// untouched.
pub(crate) async fn explain<R>(
    State(state): State<QueryState<R>>,
    body: Result<Json<PlanRequest>, JsonRejection>,
) -> impl IntoResponse
where
    R: QueryRunner,
{
    let req = match body {
        Ok(Json(req)) => req,
        Err(rejection) => return json_rejection_response(rejection),
    };
    let params = match parse_params(req.params) {
        Ok(p) => p,
        Err(err) => return lora_error_response(err),
    };
    let query = req.query;
    match run_engine(&state, move |db, _| db.explain(&query, params)).await {
        Ok(plan) => (StatusCode::OK, Json(plan_to_json(&plan))).into_response(),
        Err(err) => lora_error_response(err),
    }
}

/// `POST /profile` — execute a query and return the plan plus runtime
/// metrics.
///
/// **PROFILE EXECUTES THE QUERY FOR REAL.** Mutating queries are
/// persisted exactly as in `POST /query`. Use `POST /explain` to
/// inspect a mutating plan without running it.
pub(crate) async fn profile<R>(
    State(state): State<QueryState<R>>,
    body: Result<Json<PlanRequest>, JsonRejection>,
) -> impl IntoResponse
where
    R: QueryRunner,
{
    let req = match body {
        Ok(Json(req)) => req,
        Err(rejection) => return json_rejection_response(rejection),
    };
    let params = match parse_params(req.params) {
        Ok(p) => p,
        Err(err) => return lora_error_response(err),
    };
    let query = req.query;
    let outcome = run_engine(&state, move |db, deadline| {
        db.profile_deadline(&query, params, deadline)
    })
    .await;
    match outcome {
        Ok(prof) => (StatusCode::OK, Json(profile_to_json(&prof))).into_response(),
        Err(err) => lora_error_response(err),
    }
}

fn parse_params(
    raw: Option<serde_json::Value>,
) -> Result<Option<BTreeMap<String, LoraValue>>, LoraError> {
    let Some(value) = raw else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let serde_json::Value::Object(obj) = value else {
        return Err(LoraError::new(
            LoraErrorCode::InvalidParams,
            "params must be an object keyed by parameter name",
        ));
    };
    let mut map = BTreeMap::new();
    for (k, v) in obj {
        map.insert(k, json_value_to_lora(v)?);
    }
    Ok(Some(map))
}

fn json_value_to_lora(value: serde_json::Value) -> Result<LoraValue, LoraError> {
    use serde_json::Value as J;
    match value {
        J::Null => Ok(LoraValue::Null),
        J::Bool(b) => Ok(LoraValue::Bool(b)),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(LoraValue::Int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(LoraValue::Float(f))
            } else {
                Err(LoraError::new(
                    LoraErrorCode::InvalidParams,
                    "unsupported numeric value",
                ))
            }
        }
        J::String(s) => Ok(LoraValue::String(s)),
        J::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(json_value_to_lora(item)?);
            }
            Ok(LoraValue::List(out))
        }
        J::Object(obj) => {
            let mut map = BTreeMap::new();
            for (k, v) in obj {
                map.insert(k, json_value_to_lora(v)?);
            }
            Ok(LoraValue::Map(map))
        }
    }
}

fn plan_to_json(plan: &QueryPlan) -> serde_json::Value {
    serde_json::json!({
        "query": &plan.query,
        "shape": plan_shape_str(plan.shape),
        "resultColumns": serde_json::Value::Array(
            plan.result_columns
                .iter()
                .map(|c| serde_json::Value::String(c.clone()))
                .collect(),
        ),
        "tree": plan_tree_node_to_json(&plan.tree.root),
    })
}

fn profile_to_json(prof: &QueryProfile) -> serde_json::Value {
    serde_json::json!({
        "plan": plan_to_json(&prof.plan),
        "metrics": {
            "totalElapsedNs": prof.metrics.total_elapsed_ns as f64,
            "totalRows": prof.metrics.total_rows as f64,
            "mutated": prof.metrics.mutated,
            "perOperator": serde_json::Value::Object(
                prof.metrics
                    .per_operator
                    .iter()
                    .map(|(id, m)| (
                        id.to_string(),
                        serde_json::json!({
                            "rows": m.rows as f64,
                            "dbHits": m.db_hits as f64,
                            "elapsedNs": m.elapsed_ns as f64,
                            "nextCalls": m.next_calls as f64,
                        }),
                    ))
                    .collect(),
            ),
        },
    })
}

fn plan_tree_node_to_json(node: &PlanTreeNode) -> serde_json::Value {
    let details = serde_json::Value::Object(
        node.details
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect(),
    );
    serde_json::json!({
        "id": node.id as f64,
        "operator": &node.operator,
        "details": details,
        "estimatedRows": node.estimated_rows.map(|r| r as f64),
        "children": serde_json::Value::Array(
            node.children.iter().map(plan_tree_node_to_json).collect(),
        ),
    })
}

fn plan_shape_str(s: PlanShape) -> serde_json::Value {
    serde_json::Value::String(s.as_str().to_string())
}
