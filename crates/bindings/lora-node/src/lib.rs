#![deny(clippy::all)]

//! Node.js N-API bindings for the Lora graph database.
//!
//! Query execution runs on the libuv threadpool via [`Task`] so the
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
use napi::{Error as NapiError, Status};
use napi_derive::napi;

use lora_database::{
    snapshot_credentials_from_json, snapshot_options_from_json, Database as InnerDatabase,
    DatabaseName, DatabaseOpenOptions, InMemoryGraph, LoraError, LoraErrorCode, SnapshotConfig,
    SnapshotCredentials, SnapshotOptions, SyncMode, WalConfig,
};

mod errors;
mod json;
mod tasks;

use errors::{closed_error_message, format_lora_error, INVALID_PARAMS_CODE, LORA_ERROR_CODE};
use json::{json_value_to_params, row_to_json};
use tasks::{ClearTask, ExecuteTask, SyncTask, TransactionTask};

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
    /// `code` from the `LoraErrorCode` union (e.g. `LORA_PARSE`,
    /// `LORA_INVALID_PARAMS`, `LORA_INTERNAL`).
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
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;
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
                    format_lora_error(&LoraError::from_anyhow(e)),
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
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;
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
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;
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
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;
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
            .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;
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
    .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;
    Ok(Arc::new(db))
}

fn open_persistent_database(
    database_name: String,
    database_dir: Option<String>,
    sync_mode: Option<String>,
    group_sync_interval_ms: Option<u32>,
) -> Result<Arc<InnerDatabase<InMemoryGraph>>> {
    let name = DatabaseName::parse(&database_name).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format_lora_error(&LoraError::from(e)),
        )
    })?;
    let sync_mode = parse_sync_mode(sync_mode, group_sync_interval_ms)?;
    let mut options = DatabaseOpenOptions::default();
    if let Some(database_dir) = database_dir {
        options.database_dir = PathBuf::from(database_dir);
    }
    options.sync_mode = sync_mode;

    std::fs::create_dir_all(&options.database_dir).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format_lora_error(&LoraError::with_source(
                LoraErrorCode::Io,
                format!("failed to create database directory: {e}"),
                e,
            )),
        )
    })?;
    options.database_dir = std::fs::canonicalize(&options.database_dir).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format_lora_error(&LoraError::with_source(
                LoraErrorCode::Io,
                format!("failed to canonicalize database directory: {e}"),
                e,
            )),
        )
    })?;
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
                    Status::InvalidArg,
                    format!(
                        "{INVALID_PARAMS_CODE}: database '{}' is already open with different persistence options",
                        key.display()
                    ),
                ));
            }
            return Ok(existing);
        }
    }
    registry.retain(|_, entry| entry.db.strong_count() > 0);

    let db = InnerDatabase::open_named(name.as_str(), options)
        .map_err(|e| NapiError::new(Status::GenericFailure, format_lora_error(&e)))?;
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
