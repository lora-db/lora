//! Threadpool tasks backing the async `#[napi]` methods.
//!
//! Each `Task` impl owns its inputs (query string, params JSON, etc.) so
//! it can be moved onto the libuv worker pool by `napi`'s `AsyncTask`
//! plumbing and run without touching the JS main thread until it
//! resolves the Promise. Errors flow back as `NapiError`s carrying the
//! stable `LORA_*:` code prefixes from [`crate::errors`] (mirroring
//! `lora_database::LoraErrorCode::as_str`).

use std::collections::BTreeMap;
use std::sync::Arc;

use napi::bindgen_prelude::Result;
use napi::{Env, Error as NapiError, JsUnknown, Status, Task};

use lora_database::{
    Database as InnerDatabase, ExecuteOptions, InMemoryGraph, LoraValue, QueryResult, ResultFormat,
    TransactionMode,
};

use crate::errors::{format_lora_error, INVALID_PARAMS_CODE};
use crate::json::{json_value_to_params, plan_to_json, profile_to_json, serialize_rows};

/// Work unit for `Database.execute`. Owns its inputs so it can move onto the
/// libuv worker pool and run without touching the JS main thread until it
/// resolves the Promise with the serialised `{columns, rows}` payload.
pub struct ExecuteTask {
    pub(crate) db: Arc<InnerDatabase<InMemoryGraph>>,
    pub(crate) query: String,
    pub(crate) params: Option<serde_json::Value>,
}

impl Task for ExecuteTask {
    type Output = serde_json::Value;
    type JsValue = JsUnknown;

    fn compute(&mut self) -> Result<Self::Output> {
        // Parse params here (on the worker thread) so param-validation errors
        // surface as Promise rejections, not synchronous throws. Matches the
        // lora-wasm semantics.
        let params_map = match self.params.take() {
            None | Some(serde_json::Value::Null) => BTreeMap::new(),
            Some(other) => json_value_to_params(other)?,
        };

        let options = ExecuteOptions {
            format: ResultFormat::RowArrays,
        };

        let result = self
            .db
            .execute_with_params(&self.query, Some(options), params_map)
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;

        let QueryResult::RowArrays(row_arrays) = result else {
            return Err(NapiError::new(
                Status::GenericFailure,
                "expected RowArrays result".to_string(),
            ));
        };

        Ok(serialize_rows(&row_arrays.columns, &row_arrays.rows))
    }

    fn resolve(&mut self, env: Env, output: Self::Output) -> Result<Self::JsValue> {
        // `serde-json` feature on napi bridges serde_json::Value → JS objects.
        env.to_js_value(&output)
    }
}

/// Work unit for `Database.explain`. Compiles the query and serializes
/// the resulting plan; the executor is never invoked, so this stays
/// safe to run on any query including mutating ones.
pub struct ExplainTask {
    pub(crate) db: Arc<InnerDatabase<InMemoryGraph>>,
    pub(crate) query: String,
    pub(crate) params: Option<serde_json::Value>,
}

impl Task for ExplainTask {
    type Output = serde_json::Value;
    type JsValue = JsUnknown;

    fn compute(&mut self) -> Result<Self::Output> {
        let params_map = match self.params.take() {
            None | Some(serde_json::Value::Null) => None,
            Some(other) => Some(json_value_to_params(other)?),
        };
        let plan = self
            .db
            .explain(&self.query, params_map)
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;
        Ok(plan_to_json(&plan))
    }

    fn resolve(&mut self, env: Env, output: Self::Output) -> Result<Self::JsValue> {
        env.to_js_value(&output)
    }
}

/// Work unit for `Database.profile`. PROFILE runs the query for real,
/// including any mutations; the runtime metrics are returned alongside
/// the plan tree.
pub struct ProfileTask {
    pub(crate) db: Arc<InnerDatabase<InMemoryGraph>>,
    pub(crate) query: String,
    pub(crate) params: Option<serde_json::Value>,
}

impl Task for ProfileTask {
    type Output = serde_json::Value;
    type JsValue = JsUnknown;

    fn compute(&mut self) -> Result<Self::Output> {
        let params_map = match self.params.take() {
            None | Some(serde_json::Value::Null) => None,
            Some(other) => Some(json_value_to_params(other)?),
        };
        let profile = self
            .db
            .profile(&self.query, params_map)
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;
        Ok(profile_to_json(&profile))
    }

    fn resolve(&mut self, env: Env, output: Self::Output) -> Result<Self::JsValue> {
        env.to_js_value(&output)
    }
}

pub struct SyncTask {
    pub(crate) db: Arc<InnerDatabase<InMemoryGraph>>,
}

impl Task for SyncTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> Result<Self::Output> {
        self.db
            .sync()
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct ClearTask {
    pub(crate) db: Arc<InnerDatabase<InMemoryGraph>>,
}

impl Task for ClearTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> Result<Self::Output> {
        self.db
            .try_clear()
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct TransactionTask {
    pub(crate) db: Arc<InnerDatabase<InMemoryGraph>>,
    pub(crate) statements: serde_json::Value,
    pub(crate) mode: Option<String>,
}

impl Task for TransactionTask {
    type Output = serde_json::Value;
    type JsValue = JsUnknown;

    fn compute(&mut self) -> Result<Self::Output> {
        let mode = parse_transaction_mode(self.mode.as_deref())?;
        let statements = parse_transaction_statements(std::mem::take(&mut self.statements))?;
        let options = ExecuteOptions {
            format: ResultFormat::RowArrays,
        };
        let mut tx = self
            .db
            .begin_transaction(mode)
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;

        let mut results = Vec::with_capacity(statements.len());
        for statement in statements {
            let result = tx
                .execute_with_params(&statement.query, Some(options), statement.params)
                .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;
            let QueryResult::RowArrays(row_arrays) = result else {
                return Err(NapiError::new(
                    Status::GenericFailure,
                    "expected RowArrays result".to_string(),
                ));
            };
            results.push(serialize_rows(&row_arrays.columns, &row_arrays.rows));
        }

        tx.commit()
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;

        Ok(serde_json::Value::Array(results))
    }

    fn resolve(&mut self, env: Env, output: Self::Output) -> Result<Self::JsValue> {
        env.to_js_value(&output)
    }
}

struct TransactionStatement {
    query: String,
    params: BTreeMap<String, LoraValue>,
}

fn parse_transaction_mode(mode: Option<&str>) -> Result<TransactionMode> {
    match mode.unwrap_or("read_write") {
        "read_write" | "readwrite" | "rw" => Ok(TransactionMode::ReadWrite),
        "read_only" | "readonly" | "ro" => Ok(TransactionMode::ReadOnly),
        other => Err(NapiError::new(
            Status::InvalidArg,
            format!("{INVALID_PARAMS_CODE}: unknown transaction mode '{other}'"),
        )),
    }
}

fn parse_transaction_statements(value: serde_json::Value) -> Result<Vec<TransactionStatement>> {
    let serde_json::Value::Array(items) = value else {
        return Err(NapiError::new(
            Status::InvalidArg,
            format!("{INVALID_PARAMS_CODE}: transaction statements must be an array"),
        ));
    };

    items
        .into_iter()
        .map(|item| {
            let serde_json::Value::Object(mut obj) = item else {
                return Err(NapiError::new(
                    Status::InvalidArg,
                    format!("{INVALID_PARAMS_CODE}: transaction statement must be an object"),
                ));
            };
            let query = match obj.remove("query") {
                Some(serde_json::Value::String(query)) => query,
                _ => {
                    return Err(NapiError::new(
                        Status::InvalidArg,
                        format!(
                            "{INVALID_PARAMS_CODE}: transaction statement requires query: string"
                        ),
                    ));
                }
            };
            let params = match obj.remove("params") {
                None | Some(serde_json::Value::Null) => BTreeMap::new(),
                Some(other) => json_value_to_params(other)?,
            };
            Ok(TransactionStatement { query, params })
        })
        .collect()
}
