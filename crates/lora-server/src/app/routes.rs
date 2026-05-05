use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use lora_database::{
    ExecuteOptions, LoraError, LoraErrorCode, LoraValue, PlanShape, PlanTreeNode, QueryPlan,
    QueryProfile, QueryRunner,
};

use super::errors::lora_error_response;
use super::types::{HealthResponse, PlanRequest, QueryRequest};

pub(crate) async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub(crate) async fn query<R>(
    State(db): State<Arc<R>>,
    Json(req): Json<QueryRequest>,
) -> impl IntoResponse
where
    R: QueryRunner,
{
    let options = req.format.map(|format| ExecuteOptions {
        format: format.into(),
    });

    match db.execute(&req.query, options) {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(err) => lora_error_response(err),
    }
}

/// `POST /explain` — compile a query and return its plan as JSON
/// without invoking the executor. Mutating queries leave the graph
/// untouched.
pub(crate) async fn explain<R>(
    State(db): State<Arc<R>>,
    Json(req): Json<PlanRequest>,
) -> impl IntoResponse
where
    R: QueryRunner,
{
    let params = match parse_plan_params(req.params) {
        Ok(p) => p,
        Err(err) => return lora_error_response(err),
    };
    match db.explain(&req.query, params) {
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
    State(db): State<Arc<R>>,
    Json(req): Json<PlanRequest>,
) -> impl IntoResponse
where
    R: QueryRunner,
{
    let params = match parse_plan_params(req.params) {
        Ok(p) => p,
        Err(err) => return lora_error_response(err),
    };
    match db.profile(&req.query, params) {
        Ok(prof) => (StatusCode::OK, Json(profile_to_json(&prof))).into_response(),
        Err(err) => lora_error_response(err),
    }
}

fn parse_plan_params(
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
