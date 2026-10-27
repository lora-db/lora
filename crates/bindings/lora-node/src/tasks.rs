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
use napi::{Env, Error as NapiError, JsDeferred, JsObject, JsUnknown, Status, Task};

use lora_database::{
    Database as InnerDatabase, DirectorySnapshot, ExecuteOptions, InMemoryGraph, LoraValue,
    QueryPlan, QueryProfile, QueryResult, ResultFormat, SnapshotCredentials, SnapshotMeta,
    TransactionMode,
};

use crate::encode::{encode_query_rows, encode_rows};
use crate::errors::{format_lora_error, INVALID_PARAMS_CODE, LORA_ERROR_CODE};
use crate::guard::{guard, lock, Guarded};
use crate::json::json_value_to_params;
use crate::to_napi::{plan_to_napi, profile_to_napi};
use crate::write_queue::{self, Expiry, WriteQueue};
use crate::QueryLimit;

/// Start `task` and return its promise.
///
/// It normally runs on the libuv pool. Work that may have to wait for the
/// writer lock while a JS-bound writer (an interactive transaction or a
/// mutating stream, see [`crate::stream`]) holds it is passed `off_pool`:
/// it then waits on one of lora's own actor threads instead. That wait
/// lasts as long as the JS code keeps the transaction open, and on the
/// libuv pool a few such waits would take every worker, stalling `fs`,
/// `dns`, `zlib` and `crypto` for the whole process (and making
/// `process.exit()` hang, as libuv joins its workers on exit).
///
/// Auto-commit writes that may wait go to [`spawn_queued`] instead, so
/// that many of them do not each take a thread. What remains here
/// (`sync()`, `clear()`, snapshot loads, opening a change feed) is
/// started by one admin call at a time and never waits behind that queue.
pub(crate) fn spawn<T>(env: &Env, task: T, off_pool: bool) -> Result<JsObject>
where
    T: Task + 'static,
{
    if !off_pool {
        return Ok(env.spawn(Guarded(task))?.promise_object());
    }
    let (deferred, promise) = env.create_deferred::<T::JsValue, crate::Resolver<T::JsValue>>()?;
    // Shared with the job so a thread that fails to start still settles
    // the promise: a deferred dropped unsettled keeps the process alive.
    let slot = Arc::new(std::sync::Mutex::new(Some((task, deferred))));
    let job_slot = slot.clone();
    let started = lora_binding_buffer::stream::run_on_actor_thread(move || {
        if let Some((task, deferred)) = lock(&job_slot).take() {
            run_and_settle(task, deferred);
        }
    });
    if let Err(e) = started {
        if let Some((_, deferred)) = lock(&slot).take() {
            deferred.reject(NapiError::new(
                Status::GenericFailure,
                format!("{LORA_ERROR_CODE}: could not start a query thread: {e}"),
            ));
        }
    }
    Ok(promise)
}

/// Start `task`, an auto-commit write that may wait for a writer lock JS
/// code holds, in the handle's write queue (see [`crate::write_queue`]),
/// and return its promise. It rejects with `LORA_TIMEOUT` if `expiry`
/// passes while it is still queued.
pub(crate) fn spawn_queued<T>(
    env: &Env,
    task: T,
    queue: &WriteQueue,
    expiry: Expiry,
) -> Result<JsObject>
where
    T: Task + 'static,
{
    let (deferred, promise) = env.create_deferred::<T::JsValue, crate::Resolver<T::JsValue>>()?;
    let mut slot = Some((task, deferred));
    queue.push(
        expiry,
        write_queue::job(move |outcome| {
            let Some((task, deferred)) = slot.take() else {
                return;
            };
            match outcome {
                Ok(()) => run_and_settle(task, deferred),
                Err(message) => deferred.reject(NapiError::new(Status::GenericFailure, message)),
            }
        }),
    );
    Ok(promise)
}

/// Run `task` on this (non-libuv) thread and settle its promise.
fn run_and_settle<T>(mut task: T, deferred: JsDeferred<T::JsValue, crate::Resolver<T::JsValue>>)
where
    T: Task + 'static,
{
    let output = guard(|| task.compute());
    deferred.resolve(Box::new(move |env| {
        guard(|| match output {
            Ok(output) => task.resolve(env, output),
            Err(err) => task.reject(env, err),
        })
    }));
}

pub(crate) fn encode_query_result_rowarrays(result: QueryResult) -> Result<Vec<u8>> {
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
    pub(crate) limit: QueryLimit,
}

impl Task for ExecuteTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        // May hold or wait for the writer lock (see `crate::stream`).
        let _work = crate::stream::PoolWork::enter();
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

        let result = match self.limit.deadline() {
            Some(deadline) => self.db.execute_with_params_deadline(
                &self.query,
                Some(options),
                params_map,
                deadline,
            ),
            None => self
                .db
                .execute_with_params(&self.query, Some(options), params_map),
        }
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
        // May hold or wait for the writer lock (see `crate::stream`).
        let _work = crate::stream::PoolWork::enter();
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

/// Work unit for `Database.flush()`: an `fsync` of the WAL, no snapshot.
pub struct FlushTask {
    pub(crate) db: Arc<InnerDatabase<InMemoryGraph>>,
}

impl Task for FlushTask {
    type Output = Option<u64>;
    type JsValue = JsUnknown;

    fn compute(&mut self) -> Result<Self::Output> {
        self.db
            .flush()
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))
    }

    fn resolve(&mut self, env: Env, lsn: Self::Output) -> Result<Self::JsValue> {
        env.to_js_value(&serde_json::json!({ "lsn": lsn }))
    }
}

/// Work unit for `Database::snapshot`: an online copy of a named
/// database into an empty directory.
pub struct SnapshotDirTask {
    pub(crate) db: Arc<InnerDatabase<InMemoryGraph>>,
    pub(crate) target_dir: String,
}

impl Task for SnapshotDirTask {
    type Output = DirectorySnapshot;
    type JsValue = JsUnknown;

    fn compute(&mut self) -> Result<Self::Output> {
        // Waits for the writer lock while it takes the graph.
        let _work = crate::stream::PoolWork::enter();
        self.db
            .snapshot_to_dir(&self.target_dir)
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))
    }

    fn resolve(&mut self, env: Env, snapshot: Self::Output) -> Result<Self::JsValue> {
        env.to_js_value(&serde_json::json!({
            "path": snapshot.path.to_string_lossy(),
            "bytes": snapshot.bytes,
            "lsn": snapshot.lsn,
            "nodeCount": snapshot.node_count,
            "relationshipCount": snapshot.relationship_count,
        }))
    }
}

/// Work unit for `Database::checkpoint`.
pub struct CheckpointTask {
    pub(crate) db: Arc<InnerDatabase<InMemoryGraph>>,
}

impl Task for CheckpointTask {
    type Output = SnapshotMeta;
    type JsValue = JsUnknown;

    fn compute(&mut self) -> Result<Self::Output> {
        // Waits for the writer lock while it takes the graph.
        let _work = crate::stream::PoolWork::enter();
        self.db
            .checkpoint()
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))
    }

    fn resolve(&mut self, env: Env, meta: Self::Output) -> Result<Self::JsValue> {
        env.to_js_value(&crate::snapshot_meta_to_json(meta))
    }
}

pub struct ClearTask {
    pub(crate) db: Arc<InnerDatabase<InMemoryGraph>>,
}

impl Task for ClearTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> Result<Self::Output> {
        // May hold or wait for the writer lock (see `crate::stream`).
        let _work = crate::stream::PoolWork::enter();
        self.db
            .try_clear()
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

/// What `close()` waits for once its handle is detached.
pub(crate) enum CloseWait {
    /// Already disposed, or another handle keeps the engine open.
    Nothing,
    /// An engine no other handle shares: until the operations still
    /// holding it finish.
    Engine(std::sync::Weak<InnerDatabase<InMemoryGraph>>),
    /// The last handle of a named database: until its directory is
    /// unlocked, which happens once the operations still holding the
    /// engine finish and it closes its WAL and container.
    Directory(std::path::PathBuf),
}

/// Work unit for `Database.close()`.
pub struct CloseTask {
    pub(crate) wait: CloseWait,
    pub(crate) timeout: std::time::Duration,
}

impl Task for CloseTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> Result<Self::Output> {
        let closed = match &self.wait {
            CloseWait::Nothing => true,
            CloseWait::Engine(engine) => {
                let deadline = std::time::Instant::now() + self.timeout;
                let mut backoff = std::time::Duration::from_micros(100);
                loop {
                    if engine.strong_count() == 0 {
                        break true;
                    }
                    if std::time::Instant::now() >= deadline {
                        break false;
                    }
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(std::time::Duration::from_millis(10));
                }
            }
            CloseWait::Directory(path) => lora_database::wait_for_release(path, self.timeout),
        };
        if closed {
            Ok(())
        } else {
            Err(NapiError::new(
                Status::GenericFailure,
                format!(
                    "LORA_TIMEOUT: close() waited {} s for operations still running on the database",
                    self.timeout.as_secs()
                ),
            ))
        }
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

/// Work unit for `Database.loadSnapshot` / `loadSnapshotBuffer`. Takes
/// the writer lock, so it never runs on the JS thread: there it froze the
/// process while an interactive transaction or a mutating stream held the
/// lock, as releasing it needs the JS thread.
pub struct LoadSnapshotTask {
    pub(crate) db: Arc<InnerDatabase<InMemoryGraph>>,
    pub(crate) source: SnapshotSource,
    pub(crate) credentials: Option<SnapshotCredentials>,
}

pub(crate) enum SnapshotSource {
    Path(String),
    /// A buffer the TS wrapper copied for this call, so nothing on the JS
    /// side writes to it while the worker reads it.
    Bytes(Buffer),
}

impl Task for LoadSnapshotTask {
    type Output = SnapshotMeta;
    type JsValue = JsUnknown;

    fn compute(&mut self) -> Result<Self::Output> {
        // Holds or waits for the writer lock (see `crate::stream`).
        let _work = crate::stream::PoolWork::enter();
        match &self.source {
            SnapshotSource::Path(path) => self
                .db
                .load_snapshot_from_with_credentials(path, self.credentials.as_ref()),
            SnapshotSource::Bytes(bytes) => self.db.load_snapshot_from_bytes_with_credentials(
                bytes.as_ref(),
                self.credentials.as_ref(),
            ),
        }
        .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))
    }

    fn resolve(&mut self, env: Env, meta: Self::Output) -> Result<Self::JsValue> {
        env.to_js_value(&crate::snapshot_meta_to_json(meta))
    }
}

/// Panics in `compute`: see `Database::panic_in_task_for_testing`.
pub struct PanicTask;

impl Task for PanicTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> Result<Self::Output> {
        panic!("panic requested by __panicInTaskForTesting");
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct TransactionTask {
    pub(crate) db: Arc<InnerDatabase<InMemoryGraph>>,
    pub(crate) statements: serde_json::Value,
    pub(crate) mode: Option<String>,
    pub(crate) limit: QueryLimit,
}

impl Task for TransactionTask {
    type Output = Vec<Vec<u8>>;
    type JsValue = Vec<Buffer>;

    fn compute(&mut self) -> Result<Self::Output> {
        // May hold or wait for the writer lock (see `crate::stream`).
        let _work = crate::stream::PoolWork::enter();
        let mode = parse_transaction_mode(self.mode.as_deref())?;
        let statements = parse_transaction_statements(std::mem::take(&mut self.statements))?;
        let options = ExecuteOptions {
            format: ResultFormat::RowArrays,
        };
        let mut tx = self
            .db
            .begin_transaction(mode)
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;

        // One deadline bounds the whole batch; a statement that hits it
        // fails, and dropping `tx` rolls back everything before it.
        let deadline = self.limit.deadline();
        let mut results = Vec::with_capacity(statements.len());
        for statement in statements {
            let result = match deadline {
                Some(deadline) => tx.execute_with_params_deadline(
                    &statement.query,
                    Some(options),
                    statement.params,
                    deadline,
                ),
                None => tx.execute_with_params(&statement.query, Some(options), statement.params),
            }
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

pub(crate) struct TransactionStatement {
    pub(crate) query: String,
    pub(crate) params: BTreeMap<String, LoraValue>,
}

pub(crate) fn parse_transaction_mode(mode: Option<&str>) -> Result<TransactionMode> {
    match mode.unwrap_or("read_write") {
        "read_write" | "readwrite" | "rw" => Ok(TransactionMode::ReadWrite),
        "read_only" | "readonly" | "ro" => Ok(TransactionMode::ReadOnly),
        other => Err(NapiError::new(
            Status::InvalidArg,
            format!("{INVALID_PARAMS_CODE}: unknown transaction mode '{other}'"),
        )),
    }
}

pub(crate) fn parse_transaction_statements(
    value: serde_json::Value,
) -> Result<Vec<TransactionStatement>> {
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
