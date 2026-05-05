//! Threadpool tasks backing the async `#[napi]` methods.
//!
//! Each `Task` impl owns its inputs (query string, params JSON, etc.) so
//! it can be moved onto the libuv worker pool by `napi`'s `AsyncTask`
//! plumbing and run without touching the JS main thread until it
//! resolves the Promise. Errors flow back as `NapiError`s carrying the
//! stable `LORA_*:` code prefixes from [`crate::errors`] (mirroring
//! `lora_database::LoraErrorCode::as_str`).
//!
//! `execute()` and the per-statement results inside `transaction()` are
//! encoded into a single binary buffer on the worker thread (see
//! [`crate::encode`]) and handed to JS as a `Buffer` in one napi call.
//! This bypasses the per-row / per-cell napi syscalls that otherwise
//! dominate the wall-clock cost. The TS wrapper decodes the buffer
//! into the same `{ columns, rows }` shape the engine has always
//! produced, so the user-facing API is unchanged.
//!
//! `explain()` and `profile()` produce small, tree-shaped results;
//! they stay on the napi-direct path (`crate::to_napi`) because the
//! decode/encode overhead would dominate.

use std::collections::BTreeMap;
use std::sync::Arc;

use napi::bindgen_prelude::{Buffer, Result};
use napi::{Env, Error as NapiError, JsUnknown, Status, Task};

use lora_database::{
    Database as InnerDatabase, ExecuteOptions, InMemoryGraph, LoraValue, QueryPlan, QueryProfile,
    QueryResult, ResultFormat, TransactionMode,
};

use crate::encode::{encode_query_rows, encode_rows};
use crate::errors::{format_lora_error, INVALID_PARAMS_CODE};
use crate::json::json_value_to_params;
use crate::to_napi::{plan_to_napi, profile_to_napi};

fn encode_query_result_rowarrays(result: QueryResult) -> Result<Vec<u8>> {
    let QueryResult::RowArrays(row_arrays) = result else {
        return Err(NapiError::new(
            Status::GenericFailure,
            "expected RowArrays result".to_string(),
        ));
    };
    Ok(encode_rows(&row_arrays.columns, &row_arrays.rows))
}

fn encode_query_result_rows(result: QueryResult) -> Result<Vec<u8>> {
    let QueryResult::Rows(rows_result) = result else {
        return Err(NapiError::new(
            Status::GenericFailure,
            "expected Rows result".to_string(),
        ));
    };
    Ok(encode_query_rows(&rows_result.rows))
}

/// Work unit for `Database.execute`. Owns its inputs so it can move onto the
/// libuv worker pool and run without touching the JS main thread until it
/// resolves the Promise with the encoded result buffer.
pub struct ExecuteTask {
    pub(crate) db: Arc<InnerDatabase<InMemoryGraph>>,
    pub(crate) query: String,
    pub(crate) params: Option<serde_json::Value>,
}

impl Task for ExecuteTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        // Parse params here (on the worker thread) so param-validation errors
        // surface as Promise rejections, not synchronous throws.
        let params_map = match self.params.take() {
            None | Some(serde_json::Value::Null) => BTreeMap::new(),
            Some(other) => json_value_to_params(other)?,
        };

        // ResultFormat::Rows lets us encode straight from the engine's
        // native `Vec<Row>` and skip the RowArrays projection, which
        // otherwise clones every cell and allocates a `Vec<LoraValue>`
        // per row.
        let options = ExecuteOptions {
            format: ResultFormat::Rows,
        };

        let result = self
            .db
            .execute_with_params(&self.query, Some(options), params_map)
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;

        encode_query_result_rows(result)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        // Owned-Vec → JS Buffer: napi takes the allocation and finalizes
        // it when GC'd, so this is a single napi call with no copy.
        Ok(Buffer::from(output))
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
    type Output = QueryPlan;
    type JsValue = JsUnknown;

    fn compute(&mut self) -> Result<Self::Output> {
        let params_map = match self.params.take() {
            None | Some(serde_json::Value::Null) => None,
            Some(other) => Some(json_value_to_params(other)?),
        };
        self.db
            .explain(&self.query, params_map)
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))
    }

    fn resolve(&mut self, env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(plan_to_napi(&env, &output)?.into_unknown())
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
    type Output = QueryProfile;
    type JsValue = JsUnknown;

    fn compute(&mut self) -> Result<Self::Output> {
        let params_map = match self.params.take() {
            None | Some(serde_json::Value::Null) => None,
            Some(other) => Some(json_value_to_params(other)?),
        };
        self.db
            .profile(&self.query, params_map)
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))
    }

    fn resolve(&mut self, env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(profile_to_napi(&env, &output)?.into_unknown())
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
    type Output = Vec<Vec<u8>>;
    type JsValue = Vec<Buffer>;

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
            results.push(encode_query_result_rowarrays(result)?);
        }

        tx.commit()
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;

        Ok(results)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output.into_iter().map(Buffer::from).collect())
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
