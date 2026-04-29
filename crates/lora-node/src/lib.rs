#![deny(clippy::all)]

//! Node.js N-API bindings for the Lora graph database.
//!
//! Query execution runs on the libuv threadpool via [`napi::Task`] so the
//! JS main thread (event loop) stays responsive for the duration of a
//! query. The JS `execute()` method returns a real Promise backed by an
//! `AsyncTask`; parameter parsing, query planning, execution and result
//! serialisation all happen on a worker thread.
//!
//! `clear()`, `nodeCount()`, `relationshipCount()` stay synchronous —
//! they are constant-time lock-and-read operations and the cost of a
//! thread hop would dominate the useful work.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use napi::bindgen_prelude::*;
use napi::{Env, Error as NapiError, JsUnknown, Status, Task};
use napi_derive::napi;

use lora_database::{
    snapshot_credentials_from_json, snapshot_options_from_json, Database as InnerDatabase,
    DatabaseName, DatabaseOpenOptions, ExecuteOptions, InMemoryGraph, LoraValue, QueryResult,
    ResultFormat, Row, SnapshotConfig, SnapshotCredentials, SnapshotOptions, SyncMode,
    TransactionMode, WalConfig,
};
use lora_store::{
    LoraBinary, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint,
    LoraTime, LoraVector, RawCoordinate, VectorCoordinateType,
};

const LORA_ERROR_CODE: &str = "LORA_ERROR";
const INVALID_PARAMS_CODE: &str = "INVALID_PARAMS";
static PERSISTENT_DATABASES: OnceLock<Mutex<BTreeMap<PathBuf, PersistentDatabaseEntry>>> =
    OnceLock::new();

struct PersistentDatabaseEntry {
    db: Weak<InnerDatabase<InMemoryGraph>>,
    options: PersistentOpenOptions,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct PersistentOpenOptions {
    sync_mode: SyncMode,
    segment_target_bytes: u64,
    max_database_bytes: u64,
}

/// Lora graph database handle exposed to Node.
///
/// Wraps an `Arc<Database<InMemoryGraph>>`; the same handle is cloned
/// onto the libuv threadpool for each `execute()` call. Multiple
/// concurrent queries against the same `Database` can share read-only
/// work; writes serialize on the inner store's write lock without
/// blocking the JS event loop.
///
/// With no constructor arg the database is purely in-memory. Passing a
/// database name enables archive-backed persistence: the binding opens or
/// creates the serialized `.loradb` path under `database_dir` when supplied,
/// or the current directory otherwise. It replays committed writes on boot
/// and then serves queries against the recovered graph.
#[napi]
pub struct Database {
    db: Mutex<Option<Arc<InnerDatabase<InMemoryGraph>>>>,
    streams: Mutex<BTreeMap<u32, NativeQueryStream>>,
    next_stream_id: AtomicU32,
}

#[napi]
impl Database {
    /// Construct a database.
    ///
    /// - no args => fresh in-memory graph.
    /// - `database_name` => archive-backed graph rooted at the serialized
    ///   `.loradb` path under `database_dir`, or the current directory when no
    ///   directory is provided.
    #[napi(constructor)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        #[napi(ts_arg_type = "string | null | undefined")] database_name: Option<String>,
        #[napi(ts_arg_type = "string | null | undefined")] database_dir: Option<String>,
        #[napi(ts_arg_type = "\"group\" | \"perCommit\" | \"per_commit\" | null | undefined")]
        sync_mode: Option<String>,
        #[napi(ts_arg_type = "number | null | undefined")] group_sync_interval_ms: Option<u32>,
        #[napi(ts_arg_type = "string | null | undefined")] wal_dir: Option<String>,
        #[napi(ts_arg_type = "string | null | undefined")] snapshot_dir: Option<String>,
        #[napi(ts_arg_type = "number | null | undefined")] snapshot_every_commits: Option<u32>,
        #[napi(ts_arg_type = "number | null | undefined")] snapshot_keep_old: Option<u32>,
        #[napi(ts_arg_type = "Record<string, any> | null | undefined")] snapshot_options: Option<
            serde_json::Value,
        >,
    ) -> Result<Self> {
        let explicit_wal = wal_dir.is_some()
            || snapshot_dir.is_some()
            || snapshot_every_commits.is_some()
            || snapshot_keep_old.is_some()
            || snapshot_options.is_some();
        let db = if explicit_wal {
            if database_name.is_some() || database_dir.is_some() {
                return Err(NapiError::new(
                    Status::InvalidArg,
                    format!(
                        "{INVALID_PARAMS_CODE}: walDir/snapshotDir cannot be combined with databaseName/databaseDir"
                    ),
                ));
            }
            open_explicit_wal_database(
                wal_dir,
                snapshot_dir,
                sync_mode,
                group_sync_interval_ms,
                snapshot_every_commits,
                snapshot_keep_old,
                snapshot_options,
            )?
        } else {
            match database_name {
                None => Arc::new(InnerDatabase::in_memory()),
                Some(name) => {
                    open_persistent_database(name, database_dir, sync_mode, group_sync_interval_ms)?
                }
            }
        };
        Ok(Self {
            db: Mutex::new(Some(db)),
            streams: Mutex::new(BTreeMap::new()),
            next_stream_id: AtomicU32::new(1),
        })
    }

    /// Execute a Lora query on the libuv threadpool.
    ///
    /// The returned JS Promise resolves with `{ columns, rows }`. Values
    /// follow the shared `LoraValue` TypeScript union: primitives pass
    /// through, nodes / relationships / paths carry a `kind` discriminator,
    /// and temporal / spatial values are tagged objects.
    ///
    /// Errors surface as `LoraError` in the TS wrapper with a narrowed
    /// `code` (`LORA_ERROR`, `INVALID_PARAMS`).
    #[napi(ts_return_type = "Promise<{ columns: string[]; rows: Array<Record<string, any>> }>")]
    pub fn execute(
        &self,
        query: String,
        #[napi(ts_arg_type = "Record<string, any> | null | undefined")] params: Option<
            serde_json::Value,
        >,
    ) -> Result<AsyncTask<ExecuteTask>> {
        Ok(AsyncTask::new(ExecuteTask {
            db: self.inner()?,
            query,
            params,
        }))
    }

    /// Open a true native row stream.
    ///
    /// The returned handle owns the Rust `QueryStream`, so rows are pulled
    /// from the executor one `next()` call at a time instead of materializing
    /// the whole result up front.
    #[napi(ts_return_type = "number")]
    pub fn open_stream(
        &self,
        query: String,
        #[napi(ts_arg_type = "Record<string, any> | null | undefined")] params: Option<
            serde_json::Value,
        >,
    ) -> Result<u32> {
        let params_map = match params {
            None | Some(serde_json::Value::Null) => BTreeMap::new(),
            Some(other) => json_value_to_params(other)?,
        };
        let db = self.inner()?;
        let stream = unsafe { db.stream_with_params_owned(&query, params_map) }
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;
        let stream_id = self.next_stream_id.fetch_add(1, Ordering::Relaxed);
        let mut streams = self
            .streams
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "stream registry poisoned"))?;
        streams.insert(stream_id, NativeQueryStream { _db: db, stream });
        Ok(stream_id)
    }

    #[napi(ts_return_type = "string[]")]
    pub fn stream_columns(&self, stream_id: u32) -> Result<Vec<String>> {
        let streams = self
            .streams
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "stream registry poisoned"))?;
        let stream = streams
            .get(&stream_id)
            .ok_or_else(|| NapiError::new(Status::GenericFailure, "query stream is closed"))?;
        Ok(stream.stream.columns().to_vec())
    }

    #[napi(ts_return_type = "Record<string, any> | null")]
    pub fn stream_next(&self, stream_id: u32) -> Result<Option<serde_json::Value>> {
        let mut streams = self
            .streams
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "stream registry poisoned"))?;
        let stream = streams
            .get_mut(&stream_id)
            .ok_or_else(|| NapiError::new(Status::GenericFailure, "query stream is closed"))?;
        match stream.stream.next_row() {
            Ok(Some(row)) => Ok(Some(row_to_json(&row))),
            Ok(None) => {
                streams.remove(&stream_id);
                Ok(None)
            }
            Err(e) => {
                streams.remove(&stream_id);
                Err(NapiError::new(
                    Status::GenericFailure,
                    format!("{LORA_ERROR_CODE}: {e}"),
                ))
            }
        }
    }

    #[napi]
    pub fn stream_close(&self, stream_id: u32) -> Result<()> {
        let mut streams = self
            .streams
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "stream registry poisoned"))?;
        streams.remove(&stream_id);
        Ok(())
    }

    /// Execute multiple statements inside one core transaction.
    ///
    /// `statements` is an array of `{ query, params? }` objects. Results are
    /// returned in statement order. If any statement fails, the transaction is
    /// rolled back by dropping the native transaction before commit.
    #[napi(
        ts_return_type = "Promise<Array<{ columns: string[]; rows: Array<Record<string, any>> }>>"
    )]
    pub fn transaction(
        &self,
        #[napi(ts_arg_type = "Array<{ query: string; params?: Record<string, any> | null }>")]
        statements: serde_json::Value,
        #[napi(
            ts_arg_type = "\"read_write\" | \"read_only\" | \"readwrite\" | \"readonly\" | null | undefined"
        )]
        mode: Option<String>,
    ) -> Result<AsyncTask<TransactionTask>> {
        Ok(AsyncTask::new(TransactionTask {
            db: self.inner()?,
            statements,
            mode,
        }))
    }

    /// Force pending WAL bytes and the portable archive mirror to disk.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn sync(&self) -> Result<AsyncTask<SyncTask>> {
        Ok(AsyncTask::new(SyncTask { db: self.inner()? }))
    }

    /// Drop every node and relationship, returning the database to an empty
    /// state.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn clear(&self) -> Result<AsyncTask<ClearTask>> {
        Ok(AsyncTask::new(ClearTask { db: self.inner()? }))
    }

    /// Number of nodes in the graph. Synchronous.
    #[napi]
    pub fn node_count(&self) -> Result<u32> {
        Ok(self.inner()?.node_count() as u32)
    }

    /// Number of relationships in the graph. Synchronous.
    #[napi]
    pub fn relationship_count(&self) -> Result<u32> {
        Ok(self.inner()?.relationship_count() as u32)
    }

    /// Release the native database handle. Idempotent.
    ///
    /// Any query already dispatched to the libuv threadpool keeps its cloned
    /// handle until it finishes; new operations fail with `database is closed`.
    #[napi]
    pub fn dispose(&self) -> Result<()> {
        self.streams
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "stream registry poisoned"))?
            .clear();
        let mut slot = self
            .db
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, closed_error_message()))?;
        slot.take();
        Ok(())
    }

    /// Save the graph to a snapshot file. Atomic: the target is only
    /// replaced once the whole payload has been written + fsync'd.
    /// Synchronous — snapshots are usually infrequent and running on the
    /// event loop dodges the cost of a thread hop for small graphs.
    #[napi(
        ts_return_type = "{ formatVersion: number; nodeCount: number; relationshipCount: number; walLsn: number | null }"
    )]
    pub fn save_snapshot(
        &self,
        path: String,
        #[napi(ts_arg_type = "Record<string, any> | null | undefined")] options: Option<
            serde_json::Value,
        >,
    ) -> Result<serde_json::Value> {
        let options = parse_snapshot_options_for_napi(options)?;
        let meta = self
            .inner()?
            .save_snapshot_to_with_options(&path, &options)
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;
        Ok(snapshot_meta_to_json(meta))
    }

    /// Serialize the current graph into snapshot bytes.
    #[napi(ts_return_type = "Buffer")]
    pub fn save_snapshot_buffer(
        &self,
        #[napi(ts_arg_type = "Record<string, any> | null | undefined")] options: Option<
            serde_json::Value,
        >,
    ) -> Result<Buffer> {
        let options = parse_snapshot_options_for_napi(options)?;
        let (bytes, _) = self
            .inner()?
            .save_snapshot_to_bytes_with_options(&options)
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;
        Ok(Buffer::from(bytes))
    }

    /// Replace the current graph state with a snapshot loaded from disk.
    #[napi(
        ts_return_type = "{ formatVersion: number; nodeCount: number; relationshipCount: number; walLsn: number | null }"
    )]
    pub fn load_snapshot(
        &self,
        path: String,
        #[napi(ts_arg_type = "Record<string, any> | null | undefined")] options: Option<
            serde_json::Value,
        >,
    ) -> Result<serde_json::Value> {
        let credentials = parse_snapshot_credentials_for_napi(options)?;
        let meta = self
            .inner()?
            .load_snapshot_from_with_credentials(&path, credentials.as_ref())
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;
        Ok(snapshot_meta_to_json(meta))
    }

    /// Replace the current graph state with a snapshot loaded from bytes.
    #[napi(
        ts_return_type = "{ formatVersion: number; nodeCount: number; relationshipCount: number; walLsn: number | null }"
    )]
    pub fn load_snapshot_buffer(
        &self,
        #[napi(ts_arg_type = "Uint8Array | Buffer")] bytes: Buffer,
        #[napi(ts_arg_type = "Record<string, any> | null | undefined")] options: Option<
            serde_json::Value,
        >,
    ) -> Result<serde_json::Value> {
        let credentials = parse_snapshot_credentials_for_napi(options)?;
        let meta = self
            .inner()?
            .load_snapshot_from_bytes_with_credentials(bytes.as_ref(), credentials.as_ref())
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;
        Ok(snapshot_meta_to_json(meta))
    }

    fn inner(&self) -> Result<Arc<InnerDatabase<InMemoryGraph>>> {
        let slot = self
            .db
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, closed_error_message()))?;
        slot.as_ref()
            .cloned()
            .ok_or_else(|| NapiError::new(Status::GenericFailure, closed_error_message()))
    }
}

fn snapshot_meta_to_json(meta: lora_database::SnapshotMeta) -> serde_json::Value {
    serde_json::json!({
        "formatVersion": meta.format_version,
        "nodeCount": meta.node_count as u64,
        "relationshipCount": meta.relationship_count as u64,
        "walLsn": meta.wal_lsn,
    })
}

fn parse_snapshot_options_for_napi(options: Option<serde_json::Value>) -> Result<SnapshotOptions> {
    snapshot_options_from_json(options).map_err(|e| {
        NapiError::new(
            Status::InvalidArg,
            format!("{INVALID_PARAMS_CODE}: invalid snapshot options: {e}"),
        )
    })
}

fn parse_snapshot_credentials_for_napi(
    options: Option<serde_json::Value>,
) -> Result<Option<SnapshotCredentials>> {
    snapshot_credentials_from_json(options).map_err(|e| {
        NapiError::new(
            Status::InvalidArg,
            format!("{INVALID_PARAMS_CODE}: invalid snapshot credentials: {e}"),
        )
    })
}

fn persistent_database_registry() -> &'static Mutex<BTreeMap<PathBuf, PersistentDatabaseEntry>> {
    PERSISTENT_DATABASES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn open_explicit_wal_database(
    wal_dir: Option<String>,
    snapshot_dir: Option<String>,
    sync_mode: Option<String>,
    group_sync_interval_ms: Option<u32>,
    snapshot_every_commits: Option<u32>,
    snapshot_keep_old: Option<u32>,
    snapshot_options: Option<serde_json::Value>,
) -> Result<Arc<InnerDatabase<InMemoryGraph>>> {
    let has_snapshot_tuning = snapshot_every_commits.is_some()
        || snapshot_keep_old.is_some()
        || snapshot_options.is_some();
    let wal_dir = wal_dir.ok_or_else(|| {
        NapiError::new(
            Status::InvalidArg,
            format!("{INVALID_PARAMS_CODE}: managed snapshot options require walDir"),
        )
    })?;
    if snapshot_dir.is_none() && has_snapshot_tuning {
        return Err(NapiError::new(
            Status::InvalidArg,
            format!(
                "{INVALID_PARAMS_CODE}: snapshotDir is required when managed snapshot options are provided"
            ),
        ));
    }
    let sync_mode = parse_sync_mode(sync_mode, group_sync_interval_ms)?;
    let wal_config = WalConfig::Enabled {
        dir: PathBuf::from(wal_dir),
        sync_mode,
        segment_target_bytes: 8 * 1024 * 1024,
    };
    let db = if let Some(snapshot_dir) = snapshot_dir {
        let mut snapshots = SnapshotConfig::enabled(snapshot_dir)
            .keep_old(snapshot_keep_old.unwrap_or(1) as usize)
            .codec(parse_snapshot_options_for_napi(snapshot_options)?);
        if let Some(every) = snapshot_every_commits {
            if every != 0 {
                snapshots = snapshots.every_commits(every as u64);
            }
        }
        InnerDatabase::open_with_wal_snapshots(wal_config, snapshots)
    } else {
        InnerDatabase::open_with_wal(wal_config)
    }
    .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;
    Ok(Arc::new(db))
}

fn open_persistent_database(
    database_name: String,
    database_dir: Option<String>,
    sync_mode: Option<String>,
    group_sync_interval_ms: Option<u32>,
) -> Result<Arc<InnerDatabase<InMemoryGraph>>> {
    let name = DatabaseName::parse(&database_name)
        .map_err(|e| NapiError::new(Status::GenericFailure, format!("{LORA_ERROR_CODE}: {e}")))?;
    let sync_mode = parse_sync_mode(sync_mode, group_sync_interval_ms)?;
    let mut options = DatabaseOpenOptions::default();
    if let Some(database_dir) = database_dir {
        options.database_dir = PathBuf::from(database_dir);
    }
    options.sync_mode = sync_mode;

    std::fs::create_dir_all(&options.database_dir)
        .map_err(|e| NapiError::new(Status::GenericFailure, format!("{LORA_ERROR_CODE}: {e}")))?;
    options.database_dir = std::fs::canonicalize(&options.database_dir)
        .map_err(|e| NapiError::new(Status::GenericFailure, format!("{LORA_ERROR_CODE}: {e}")))?;
    let key = options.database_path_for(&name);
    let open_options = PersistentOpenOptions {
        sync_mode: options.sync_mode,
        segment_target_bytes: options.segment_target_bytes,
        max_database_bytes: options.max_database_bytes,
    };

    let registry = persistent_database_registry();
    let mut registry = registry.lock().map_err(|_| {
        NapiError::new(
            Status::GenericFailure,
            format!("{LORA_ERROR_CODE}: persistent database registry poisoned"),
        )
    })?;
    if let Some(entry) = registry.get(&key) {
        if let Some(existing) = entry.db.upgrade() {
            if entry.options != open_options {
                return Err(NapiError::new(
                    Status::GenericFailure,
                    format!(
                        "{LORA_ERROR_CODE}: database '{}' is already open with different persistence options",
                        key.display()
                    ),
                ));
            }
            return Ok(existing);
        }
    }
    registry.retain(|_, entry| entry.db.strong_count() > 0);

    let db = InnerDatabase::open_named(name.as_str(), options)
        .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;
    let db = Arc::new(db);
    registry.insert(
        key,
        PersistentDatabaseEntry {
            db: Arc::downgrade(&db),
            options: open_options,
        },
    );
    Ok(db)
}

fn parse_sync_mode(
    sync_mode: Option<String>,
    group_sync_interval_ms: Option<u32>,
) -> Result<SyncMode> {
    let interval_ms = group_sync_interval_ms.unwrap_or(1_000);
    if interval_ms == 0 {
        return Err(NapiError::new(
            Status::InvalidArg,
            format!("{INVALID_PARAMS_CODE}: groupSyncIntervalMs must be greater than 0"),
        ));
    }

    match sync_mode.as_deref().unwrap_or("group") {
        "group" => Ok(SyncMode::Group { interval_ms }),
        "perCommit" | "per_commit" => Ok(SyncMode::PerCommit),
        other => Err(NapiError::new(
            Status::InvalidArg,
            format!(
                "{INVALID_PARAMS_CODE}: invalid syncMode '{other}'; expected 'group' or 'perCommit'"
            ),
        )),
    }
}

impl Default for Database {
    fn default() -> Self {
        Self::new(None, None, None, None, None, None, None, None, None)
            .expect("in-memory Database::default should not fail")
    }
}

pub struct NativeQueryStream {
    _db: Arc<InnerDatabase<InMemoryGraph>>,
    stream: lora_database::QueryStream<'static>,
}

// ============================================================================
// Threadpool task
// ============================================================================

/// Work unit for `Database.execute`. Owns its inputs so it can move onto the
/// libuv worker pool and run without touching the JS main thread until it
/// resolves the Promise with the serialised `{columns, rows}` payload.
pub struct ExecuteTask {
    db: Arc<InnerDatabase<InMemoryGraph>>,
    query: String,
    params: Option<serde_json::Value>,
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
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;

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

pub struct SyncTask {
    db: Arc<InnerDatabase<InMemoryGraph>>,
}

impl Task for SyncTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> Result<Self::Output> {
        self.db
            .sync()
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct ClearTask {
    db: Arc<InnerDatabase<InMemoryGraph>>,
}

impl Task for ClearTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> Result<Self::Output> {
        self.db
            .try_clear()
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct TransactionTask {
    db: Arc<InnerDatabase<InMemoryGraph>>,
    statements: serde_json::Value,
    mode: Option<String>,
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
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;

        let mut results = Vec::with_capacity(statements.len());
        for statement in statements {
            let result = tx
                .execute_with_params(&statement.query, Some(options), statement.params)
                .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;
            let QueryResult::RowArrays(row_arrays) = result else {
                return Err(NapiError::new(
                    Status::GenericFailure,
                    "expected RowArrays result".to_string(),
                ));
            };
            results.push(serialize_rows(&row_arrays.columns, &row_arrays.rows));
        }

        tx.commit()
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;

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

fn serialize_rows(columns: &[String], rows: &[Vec<LoraValue>]) -> serde_json::Value {
    let columns_json: Vec<serde_json::Value> = columns
        .iter()
        .map(|c| serde_json::Value::String(c.clone()))
        .collect();

    let rows_json: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let mut obj = serde_json::Map::with_capacity(columns.len());
            for (col, val) in columns.iter().zip(row.iter()) {
                obj.insert(col.clone(), lora_value_to_json(val));
            }
            serde_json::Value::Object(obj)
        })
        .collect();

    serde_json::json!({
        "columns": serde_json::Value::Array(columns_json),
        "rows": serde_json::Value::Array(rows_json),
    })
}

fn row_to_json(row: &Row) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (_, name, value) in row.iter_named() {
        obj.insert(name.into_owned(), lora_value_to_json(value));
    }
    serde_json::Value::Object(obj)
}

// ============================================================================
// LoraValue <-> JSON conversion
// ============================================================================

fn lora_value_to_json(value: &LoraValue) -> serde_json::Value {
    use serde_json::Value as J;
    match value {
        LoraValue::Null => J::Null,
        LoraValue::Bool(b) => J::Bool(*b),
        LoraValue::Int(i) => J::Number((*i).into()),
        LoraValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .unwrap_or(J::Null),
        LoraValue::String(s) => J::String(s.clone()),
        LoraValue::Binary(b) => binary_to_json(b),
        LoraValue::List(items) => J::Array(items.iter().map(lora_value_to_json).collect()),
        LoraValue::Map(m) => {
            let obj = m
                .iter()
                .map(|(k, v)| (k.clone(), lora_value_to_json(v)))
                .collect::<serde_json::Map<_, _>>();
            J::Object(obj)
        }
        LoraValue::Node(id) => serde_json::json!({
            "kind": "node",
            "id": *id as i64,
            "labels": serde_json::Value::Array(vec![]),
            "properties": serde_json::Value::Object(Default::default()),
        }),
        LoraValue::Relationship(id) => serde_json::json!({
            "kind": "relationship",
            "id": *id as i64,
        }),
        LoraValue::Path(p) => serde_json::json!({
            "kind": "path",
            "nodes": p.nodes.iter().map(|n| *n as i64).collect::<Vec<_>>(),
            "rels": p.rels.iter().map(|n| *n as i64).collect::<Vec<_>>(),
        }),
        LoraValue::Date(d) => serde_json::json!({ "kind": "date", "iso": d.to_string() }),
        LoraValue::Time(t) => serde_json::json!({ "kind": "time", "iso": t.to_string() }),
        LoraValue::LocalTime(t) => {
            serde_json::json!({ "kind": "localtime", "iso": t.to_string() })
        }
        LoraValue::DateTime(dt) => {
            serde_json::json!({ "kind": "datetime", "iso": dt.to_string() })
        }
        LoraValue::LocalDateTime(dt) => {
            serde_json::json!({ "kind": "localdatetime", "iso": dt.to_string() })
        }
        LoraValue::Duration(d) => {
            serde_json::json!({ "kind": "duration", "iso": d.to_string() })
        }
        LoraValue::Point(p) => point_to_json(p),
        LoraValue::Vector(v) => vector_to_json(v),
    }
}

fn binary_to_json(b: &LoraBinary) -> serde_json::Value {
    serde_json::json!({
        "kind": "binary",
        "length": b.len(),
        "segments": b.segments(),
    })
}

/// Render a `LoraVector` into the canonical external tagged shape.
fn vector_to_json(v: &LoraVector) -> serde_json::Value {
    let values: serde_json::Value = match &v.values {
        lora_store::VectorValues::Float64(vs) => serde_json::Value::Array(
            vs.iter()
                .map(|x| {
                    serde_json::Number::from_f64(*x)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null)
                })
                .collect(),
        ),
        lora_store::VectorValues::Float32(vs) => serde_json::Value::Array(
            vs.iter()
                .map(|x| {
                    serde_json::Number::from_f64(*x as f64)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null)
                })
                .collect(),
        ),
        lora_store::VectorValues::Integer64(vs) => {
            serde_json::Value::Array(vs.iter().map(|x| serde_json::json!(*x)).collect())
        }
        lora_store::VectorValues::Integer32(vs) => {
            serde_json::Value::Array(vs.iter().map(|x| serde_json::json!(*x as i64)).collect())
        }
        lora_store::VectorValues::Integer16(vs) => {
            serde_json::Value::Array(vs.iter().map(|x| serde_json::json!(*x as i64)).collect())
        }
        lora_store::VectorValues::Integer8(vs) => {
            serde_json::Value::Array(vs.iter().map(|x| serde_json::json!(*x as i64)).collect())
        }
    };
    serde_json::json!({
        "kind": "vector",
        "dimension": v.dimension,
        "coordinateType": v.coordinate_type().as_str(),
        "values": values,
    })
}

/// Render a `LoraPoint` into the canonical external point shape consumed
/// by the shared TS `LoraPoint` union. Cartesian points carry `x`/`y`
/// (and `z` for 3D); WGS-84 points additionally expose the geographic
/// aliases `longitude`/`latitude` (and `height` for 3D).
fn point_to_json(p: &LoraPoint) -> serde_json::Value {
    let mut obj = serde_json::Map::with_capacity(7);
    obj.insert("kind".into(), serde_json::Value::String("point".into()));
    obj.insert("srid".into(), serde_json::json!(p.srid));
    obj.insert("crs".into(), serde_json::Value::String(p.crs_name().into()));
    obj.insert("x".into(), serde_json::json!(p.x));
    obj.insert("y".into(), serde_json::json!(p.y));
    if let Some(z) = p.z {
        obj.insert("z".into(), serde_json::json!(z));
    }
    if p.is_geographic() {
        obj.insert("longitude".into(), serde_json::json!(p.longitude()));
        obj.insert("latitude".into(), serde_json::json!(p.latitude()));
        if let Some(h) = p.height() {
            obj.insert("height".into(), serde_json::json!(h));
        }
    }
    serde_json::Value::Object(obj)
}

fn json_value_to_params(value: serde_json::Value) -> Result<BTreeMap<String, LoraValue>> {
    match value {
        serde_json::Value::Object(obj) => {
            let mut map = BTreeMap::new();
            for (k, v) in obj {
                map.insert(k, json_value_to_cypher(v)?);
            }
            Ok(map)
        }
        _ => Err(NapiError::new(
            Status::InvalidArg,
            format!("{INVALID_PARAMS_CODE}: params must be an object keyed by parameter name"),
        )),
    }
}

fn json_value_to_cypher(value: serde_json::Value) -> Result<LoraValue> {
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
                Err(NapiError::new(
                    Status::InvalidArg,
                    format!("{INVALID_PARAMS_CODE}: unsupported numeric value"),
                ))
            }
        }
        J::String(s) => Ok(LoraValue::String(s)),
        J::Array(items) => {
            let list = items
                .into_iter()
                .map(json_value_to_cypher)
                .collect::<Result<Vec<_>>>()?;
            Ok(LoraValue::List(list))
        }
        J::Object(obj) => {
            if let Some(serde_json::Value::String(kind)) = obj.get("kind") {
                match kind.as_str() {
                    "date" => {
                        let iso = require_iso(&obj, "date")?;
                        let d = LoraDate::parse(iso).map_err(invalid_param)?;
                        return Ok(LoraValue::Date(d));
                    }
                    "time" => {
                        let iso = require_iso(&obj, "time")?;
                        let t = LoraTime::parse(iso).map_err(invalid_param)?;
                        return Ok(LoraValue::Time(t));
                    }
                    "localtime" => {
                        let iso = require_iso(&obj, "localtime")?;
                        let t = LoraLocalTime::parse(iso).map_err(invalid_param)?;
                        return Ok(LoraValue::LocalTime(t));
                    }
                    "datetime" => {
                        let iso = require_iso(&obj, "datetime")?;
                        let dt = LoraDateTime::parse(iso).map_err(invalid_param)?;
                        return Ok(LoraValue::DateTime(dt));
                    }
                    "localdatetime" => {
                        let iso = require_iso(&obj, "localdatetime")?;
                        let dt = LoraLocalDateTime::parse(iso).map_err(invalid_param)?;
                        return Ok(LoraValue::LocalDateTime(dt));
                    }
                    "duration" => {
                        let iso = require_iso(&obj, "duration")?;
                        let d = LoraDuration::parse(iso).map_err(invalid_param)?;
                        return Ok(LoraValue::Duration(d));
                    }
                    "point" => {
                        let srid = obj.get("srid").and_then(|v| v.as_u64()).unwrap_or(7203) as u32;
                        let x = obj
                            .get("x")
                            .and_then(|v| v.as_f64())
                            .ok_or_else(|| invalid_param("point.x must be a number"))?;
                        let y = obj
                            .get("y")
                            .and_then(|v| v.as_f64())
                            .ok_or_else(|| invalid_param("point.y must be a number"))?;
                        let z = obj.get("z").and_then(|v| v.as_f64());
                        return Ok(LoraValue::Point(LoraPoint { x, y, z, srid }));
                    }
                    "vector" => {
                        let v = vector_from_json_map(&obj).map_err(invalid_param)?;
                        return Ok(LoraValue::Vector(v));
                    }
                    "binary" | "blob" => {
                        return Ok(LoraValue::Binary(binary_from_json_map(&obj)?));
                    }
                    _ => {}
                }
            }
            let mut map = BTreeMap::new();
            for (k, v) in obj {
                map.insert(k, json_value_to_cypher(v)?);
            }
            Ok(LoraValue::Map(map))
        }
    }
}

fn binary_from_json_map(obj: &serde_json::Map<String, serde_json::Value>) -> Result<LoraBinary> {
    let segments = obj
        .get("segments")
        .and_then(|v| v.as_array())
        .ok_or_else(|| invalid_param("binary.segments must be an array of byte arrays"))?;
    let mut out = Vec::with_capacity(segments.len());
    for segment in segments {
        let bytes = segment
            .as_array()
            .ok_or_else(|| invalid_param("binary segment must be an array of bytes"))?;
        let mut chunk = Vec::with_capacity(bytes.len());
        for byte in bytes {
            let value = byte
                .as_u64()
                .ok_or_else(|| invalid_param("binary byte must be an integer 0..255"))?;
            let value = u8::try_from(value)
                .map_err(|_| invalid_param("binary byte must be an integer 0..255"))?;
            chunk.push(value);
        }
        out.push(chunk);
    }
    Ok(LoraBinary::from_segments(out))
}

fn require_iso<'a>(
    obj: &'a serde_json::Map<String, serde_json::Value>,
    tag: &str,
) -> Result<&'a str> {
    match obj.get("iso").and_then(|v| v.as_str()) {
        Some(s) => Ok(s),
        None => Err(invalid_param(format!("{tag} value requires iso: string"))),
    }
}

fn invalid_param(msg: impl Into<String>) -> NapiError {
    NapiError::new(
        Status::InvalidArg,
        format!("{INVALID_PARAMS_CODE}: {}", msg.into()),
    )
}

/// Parse a tagged `{kind: "vector", dimension, coordinateType, values}`
/// map into a `LoraVector`. Used by every binding that accepts a vector
/// parameter — the validation rules are identical across bindings.
pub(crate) fn vector_from_json_map(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> std::result::Result<LoraVector, String> {
    let dimension = obj
        .get("dimension")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "vector.dimension must be an integer".to_string())?;
    let coordinate_type_name = obj
        .get("coordinateType")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "vector.coordinateType must be a string".to_string())?;
    let coordinate_type = VectorCoordinateType::parse(coordinate_type_name)
        .ok_or_else(|| format!("unknown vector coordinate type '{coordinate_type_name}'"))?;
    let values = obj
        .get("values")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "vector.values must be an array of numbers".to_string())?;

    let mut raw = Vec::with_capacity(values.len());
    for v in values {
        match v {
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    raw.push(RawCoordinate::Int(i));
                } else if let Some(f) = n.as_f64() {
                    raw.push(RawCoordinate::Float(f));
                } else {
                    return Err("vector.values entries must be finite numbers".to_string());
                }
            }
            _ => return Err("vector.values entries must be numbers".to_string()),
        }
    }

    LoraVector::try_new(raw, dimension, coordinate_type).map_err(|e| e.to_string())
}

fn format_error(err: &anyhow::Error) -> String {
    format!("{LORA_ERROR_CODE}: {err}")
}

fn closed_error_message() -> String {
    format!("{LORA_ERROR_CODE}: database is closed")
}
