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
use napi::{Env, Error as NapiError, JsObject, JsUnknown, Status};
use napi_derive::napi;

use lora_database::{
    snapshot_credentials_from_json, snapshot_options_from_json, CancellableDeadline,
    Database as InnerDatabase, DatabaseName, DatabaseOpenOptions, InMemoryGraph, LoraError,
    LoraErrorCode, SnapshotConfig, SnapshotCredentials, SnapshotOptions, SyncMode, WalConfig,
};

mod actor;
mod changes;
mod encode;
mod errors;
mod interactive;
mod json;
mod stream;
mod tasks;
mod to_napi;

use errors::{closed_error_message, format_lora_error, INVALID_PARAMS_CODE, LORA_ERROR_CODE};
use json::json_value_to_params;
use tasks::{ClearTask, ExecuteTask, ExplainTask, ProfileTask, SyncTask, TransactionTask};
use to_napi::row_to_napi;

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
/// database name enables container-backed persistence: the binding opens or
/// creates the serialized `.loradb` path under `database_dir` when supplied,
/// or the current directory otherwise. It replays committed writes on boot
/// and then serves queries against the recovered graph.
#[napi]
pub struct Database {
    db: Mutex<Option<Arc<InnerDatabase<InMemoryGraph>>>>,
    streams: Mutex<BTreeMap<u32, NativeQueryStream>>,
    next_stream_id: AtomicU32,
    /// Database-wide timeout applied when a call passes none.
    default_timeout_ms: Option<u32>,
    /// Cancellation handles for in-flight queries started with an
    /// `AbortSignal`, keyed by the token handed to JS.
    cancels: Mutex<BTreeMap<u32, SharedCancel>>,
    next_cancel_id: AtomicU32,
    /// Open interactive transactions (see [`interactive`]).
    txs: TxRegistry,
    next_tx_id: AtomicU32,
    /// Open change feeds (see [`changes`]).
    feeds: changes::FeedRegistry,
    next_feed_id: AtomicU32,
}

pub(crate) type TxRegistry = Arc<Mutex<BTreeMap<u32, Arc<interactive::TxActor>>>>;

#[napi]
impl Database {
    /// Construct a database.
    ///
    /// - no args => fresh in-memory graph.
    /// - `database_name` => container-backed graph rooted at the serialized
    ///   `.loradb` path under `database_dir`, or the current directory when no
    ///   directory is provided.
    #[napi(constructor)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        #[napi(ts_arg_type = "string | null | undefined")] database_name: Option<String>,
        #[napi(ts_arg_type = "string | null | undefined")] database_dir: Option<String>,
        #[napi(ts_arg_type = "\"groupSync\" | null | undefined")] sync_mode: Option<String>,
        #[napi(ts_arg_type = "number | null | undefined")] group_sync_interval_ms: Option<u32>,
        #[napi(ts_arg_type = "string | null | undefined")] wal_dir: Option<String>,
        #[napi(ts_arg_type = "string | null | undefined")] snapshot_dir: Option<String>,
        #[napi(ts_arg_type = "number | null | undefined")] snapshot_every_commits: Option<u32>,
        #[napi(ts_arg_type = "number | null | undefined")] snapshot_keep_old: Option<u32>,
        #[napi(ts_arg_type = "Record<string, any> | null | undefined")] snapshot_options: Option<
            serde_json::Value,
        >,
        #[napi(ts_arg_type = "number | null | undefined")] query_timeout_ms: Option<u32>,
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
            default_timeout_ms: query_timeout_ms.filter(|ms| *ms > 0),
            cancels: Mutex::new(BTreeMap::new()),
            next_cancel_id: AtomicU32::new(1),
            txs: Arc::new(Mutex::new(BTreeMap::new())),
            next_tx_id: AtomicU32::new(1),
            feeds: Arc::new(Mutex::new(changes::FeedSet::default())),
            next_feed_id: AtomicU32::new(1),
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
    /// Execute a Lora query and return the encoded result buffer.
    ///
    /// The TS wrapper decodes the buffer into the canonical
    /// `{ columns, rows }` shape. Encoding the result on the libuv
    /// worker and transferring it to JS as a single Buffer avoids the
    /// per-cell napi syscalls that otherwise dominate wall-clock cost
    /// on bulk reads. See `crates/bindings/lora-node/src/encode.rs`
    /// for the wire format.
    ///
    /// `timeout_ms` bounds the query (falling back to the database-wide
    /// `queryTimeoutMs`); `cancel_token` from [`Self::create_cancel_token`]
    /// lets JS cancel it early. Either way an expired query rejects with
    /// `LORA_TIMEOUT`, aborts its WAL transaction and releases its locks.
    #[napi(ts_return_type = "Promise<Buffer>")]
    pub fn execute(
        &self,
        query: String,
        #[napi(ts_arg_type = "Record<string, any> | null | undefined")] params: Option<
            serde_json::Value,
        >,
        #[napi(ts_arg_type = "number | null | undefined")] timeout_ms: Option<u32>,
        #[napi(ts_arg_type = "number | null | undefined")] cancel_token: Option<u32>,
    ) -> Result<AsyncTask<ExecuteTask>> {
        Ok(AsyncTask::new(ExecuteTask {
            db: self.inner()?,
            query,
            params,
            limit: self.limit(timeout_ms, cancel_token)?,
        }))
    }

    /// Create a cancellation token for one query. `timeout_ms` (or the
    /// database default) is folded into it, so pass the token instead of
    /// a timeout. Release it with [`Self::release_cancel_token`] once the
    /// query settles.
    #[napi]
    pub fn create_cancel_token(
        &self,
        #[napi(ts_arg_type = "number | null | undefined")] timeout_ms: Option<u32>,
    ) -> Result<u32> {
        let timeout = self.effective_timeout(timeout_ms);
        let handle = Arc::new(Mutex::new(CancellableDeadline::new(timeout)));
        let id = self.next_cancel_id.fetch_add(1, Ordering::Relaxed);
        self.cancels
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "cancel registry poisoned"))?
            .insert(id, handle);
        Ok(id)
    }

    /// Cancel the query holding `token`. It stops at its next check point
    /// and rejects with `LORA_TIMEOUT`. Unknown tokens are ignored.
    #[napi]
    pub fn cancel_query(&self, token: u32) -> Result<()> {
        let handle = self
            .cancels
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "cancel registry poisoned"))?
            .get(&token)
            .cloned();
        if let Some(handle) = handle {
            handle.lock().unwrap_or_else(|p| p.into_inner()).cancel();
        }
        Ok(())
    }

    #[napi]
    pub fn release_cancel_token(&self, token: u32) -> Result<()> {
        self.cancels
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "cancel registry poisoned"))?
            .remove(&token);
        Ok(())
    }

    /// Compile a query and return its execution plan without running it.
    ///
    /// Unlike [`Self::execute`], this never invokes the executor — even
    /// for mutating queries (`CREATE`, `MERGE`, `SET`, `DELETE`,
    /// `REMOVE`) `explain()` produces no side effects. The returned
    /// object describes the operator tree the executor *would* run.
    #[napi(ts_return_type = "Promise<LoraQueryPlan>")]
    pub fn explain(
        &self,
        query: String,
        #[napi(ts_arg_type = "Record<string, any> | null | undefined")] params: Option<
            serde_json::Value,
        >,
    ) -> Result<AsyncTask<ExplainTask>> {
        Ok(AsyncTask::new(ExplainTask {
            db: self.inner()?,
            query,
            params,
        }))
    }

    /// Execute a query and return runtime metrics alongside the plan.
    ///
    /// **`profile()` runs the query for real.** Mutating queries
    /// produce the same side effects as `execute()`. Use `explain()`
    /// to inspect a mutating plan without running it.
    #[napi(ts_return_type = "Promise<LoraQueryProfile>")]
    pub fn profile(
        &self,
        query: String,
        #[napi(ts_arg_type = "Record<string, any> | null | undefined")] params: Option<
            serde_json::Value,
        >,
    ) -> Result<AsyncTask<ProfileTask>> {
        Ok(AsyncTask::new(ProfileTask {
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
        #[napi(ts_arg_type = "number | null | undefined")] timeout_ms: Option<u32>,
        #[napi(ts_arg_type = "number | null | undefined")] cancel_token: Option<u32>,
    ) -> Result<u32> {
        let limit = self.limit(timeout_ms, cancel_token)?;
        let deadline = limit.deadline();
        let params_map = match params {
            None | Some(serde_json::Value::Null) => BTreeMap::new(),
            Some(other) => json_value_to_params(other)?,
        };
        let db = self.inner()?;
        let lora_err = |e: LoraError| NapiError::new(Status::GenericFailure, format_lora_error(&e));
        // A mutating stream takes the writer lock when it opens. Waiting for
        // it here, on the JS thread, deadlocks against an interactive
        // transaction holding the lock (it needs the JS thread for its next
        // command), so such a stream opens on its own thread and is pulled
        // asynchronously (see [`stream`]).
        let plan = db.explain(&query, None).map_err(lora_err)?;
        let (kind, columns) = if plan.shape.is_mutating() {
            let actor = stream::StreamActor::spawn(db, query, params_map, deadline)
                .map_err(interactive::napi_err)?;
            (StreamKind::Actor(actor), plan.result_columns)
        } else {
            let local = stream::LocalStream::open(db, &query, params_map).map_err(lora_err)?;
            let columns = local.stream.columns().to_vec();
            (StreamKind::Local(local), columns)
        };
        let stream_id = self.next_stream_id.fetch_add(1, Ordering::Relaxed);
        let mut streams = self
            .streams
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "stream registry poisoned"))?;
        streams.insert(
            stream_id,
            NativeQueryStream {
                kind,
                columns,
                deadline,
                _limit: limit,
            },
        );
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
        Ok(stream.columns.clone())
    }

    /// Whether stream `stream_id` is pulled with [`Self::stream_next_async`]
    /// (a mutating stream) rather than [`Self::stream_next`].
    #[napi]
    pub fn stream_is_async(&self, stream_id: u32) -> Result<bool> {
        let streams = self
            .streams
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "stream registry poisoned"))?;
        let stream = streams
            .get(&stream_id)
            .ok_or_else(|| NapiError::new(Status::GenericFailure, stream::closed()))?;
        Ok(matches!(stream.kind, StreamKind::Actor(_)))
    }

    /// Pull the next row of a mutating stream. Resolves with the row, or
    /// `null` at the end (the stream's writes are then committed); a failure
    /// rolls them back. The pull runs on the stream's own thread, which
    /// first waits for the writer lock. After the end or a failure, the
    /// caller closes the stream with [`Self::stream_close`].
    #[napi(ts_return_type = "Promise<Record<string, any> | null>")]
    pub fn stream_next_async(&self, env: Env, stream_id: u32) -> Result<JsObject> {
        let streams = self
            .streams
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "stream registry poisoned"))?;
        let stream = streams
            .get(&stream_id)
            .ok_or_else(|| NapiError::new(Status::GenericFailure, stream::closed()))?;
        let StreamKind::Actor(actor) = &stream.kind else {
            return Err(NapiError::new(
                Status::GenericFailure,
                "LORA_INTERNAL: a read-only stream is pulled with streamNext",
            ));
        };
        let (deferred, promise) =
            env.create_deferred::<Option<JsObject>, Resolver<Option<JsObject>>>()?;
        actor.pull(stream::pull_done(move |pulled| {
            deferred.resolve(Box::new(move |env| match pulled {
                Ok(Some(row)) => Ok(Some(row_to_napi(&env, &row)?)),
                Ok(None) => Ok(None),
                Err(message) => Err(NapiError::new(Status::GenericFailure, message)),
            }))
        }));
        Ok(promise)
    }

    #[napi(ts_return_type = "Record<string, any> | null")]
    pub fn stream_next(&self, env: Env, stream_id: u32) -> Result<Option<JsUnknown>> {
        let mut streams = self
            .streams
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "stream registry poisoned"))?;
        let stream = streams
            .get_mut(&stream_id)
            .ok_or_else(|| NapiError::new(Status::GenericFailure, stream::closed()))?;
        let StreamKind::Local(local) = &mut stream.kind else {
            return Err(NapiError::new(
                Status::GenericFailure,
                "LORA_INTERNAL: a mutating stream is pulled with streamNextAsync",
            ));
        };
        // Rows are pulled one call at a time, so the stream's deadline (or
        // cancellation) is enforced between rows; closing the stream drops
        // its read snapshot.
        if stream.deadline.is_some_and(lora_executor_deadline_reached) {
            streams.remove(&stream_id);
            return Err(NapiError::new(Status::GenericFailure, stream::timed_out()));
        }
        match local.stream.next_row() {
            Ok(Some(row)) => Ok(Some(row_to_napi(&env, &row)?.into_unknown())),
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
    #[napi(ts_return_type = "Promise<Buffer[]>")]
    pub fn transaction(
        &self,
        #[napi(ts_arg_type = "Array<{ query: string; params?: Record<string, any> | null }>")]
        statements: serde_json::Value,
        #[napi(
            ts_arg_type = "\"read_write\" | \"read_only\" | \"readwrite\" | \"readonly\" | null | undefined"
        )]
        mode: Option<String>,
        #[napi(ts_arg_type = "number | null | undefined")] timeout_ms: Option<u32>,
        #[napi(ts_arg_type = "number | null | undefined")] cancel_token: Option<u32>,
    ) -> Result<AsyncTask<TransactionTask>> {
        Ok(AsyncTask::new(TransactionTask {
            db: self.inner()?,
            statements,
            mode,
            limit: self.limit(timeout_ms, cancel_token)?,
        }))
    }

    /// Begin an interactive transaction. Resolves with a transaction id
    /// once it is open; a read-write transaction holds the writer lock
    /// (other writers wait) until it commits or rolls back.
    ///
    /// Waiting for the writer lock happens on the transaction's own thread
    /// and never occupies a libuv worker (see [`interactive`]).
    #[napi(ts_return_type = "Promise<number>")]
    pub fn begin_transaction(
        &self,
        env: Env,
        #[napi(
            ts_arg_type = "\"read_write\" | \"read_only\" | \"readwrite\" | \"readonly\" | null | undefined"
        )]
        mode: Option<String>,
    ) -> Result<JsObject> {
        // Everything that can fail runs before the deferred exists: a
        // deferred dropped unsettled keeps the process alive.
        let mode = tasks::parse_transaction_mode(mode.as_deref())?;
        let db = self.inner()?;
        let mut txs = self
            .txs
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "transaction registry poisoned"))?;
        let id = self.next_tx_id.fetch_add(1, Ordering::Relaxed);
        let (deferred, promise) = env.create_deferred::<u32, Resolver<u32>>()?;
        let registry = self.txs.clone();
        let actor = interactive::TxActor::spawn(
            db,
            mode,
            interactive::done(move |opened| {
                // Settled on the JS thread, after this call has registered
                // the actor: a failed open unregisters it there. A
                // transaction that opened just as `dispose()` dropped it is
                // closed, not handed out.
                deferred.resolve(Box::new(move |_env| match opened {
                    Ok(()) if tx_registered(&registry, id) => Ok(id),
                    Ok(()) => Err(interactive::napi_err(interactive::closed())),
                    Err(message) => {
                        forget_tx(&registry, id);
                        Err(interactive::napi_err(message))
                    }
                }))
            }),
        );
        txs.insert(id, Arc::new(actor));
        Ok(promise)
    }

    /// Run one statement inside interactive transaction `tx_id`. A failed
    /// statement rolls the transaction back.
    #[napi(ts_return_type = "Promise<Buffer>")]
    pub fn tx_execute(
        &self,
        env: Env,
        tx_id: u32,
        query: String,
        #[napi(ts_arg_type = "Record<string, any> | null | undefined")] params: Option<
            serde_json::Value,
        >,
        #[napi(ts_arg_type = "number | null | undefined")] timeout_ms: Option<u32>,
        #[napi(ts_arg_type = "number | null | undefined")] cancel_token: Option<u32>,
    ) -> Result<JsObject> {
        let actor = self.tx_actor(tx_id)?;
        let limit = self.limit(timeout_ms, cancel_token)?;
        let params = match params {
            None | Some(serde_json::Value::Null) => BTreeMap::new(),
            Some(other) => match json_value_to_params(other) {
                Ok(params) => params,
                Err(err) => {
                    // The JS side treats any rejection as closing the
                    // transaction; roll it back so the writer lock is freed.
                    // Dropping the last handle never waits for a running
                    // statement (see `actor`).
                    forget_tx(&self.txs, tx_id);
                    actor.rollback(interactive::done(|_| {}));
                    return Err(err);
                }
            },
        };
        let (deferred, promise) = env.create_deferred::<Buffer, Resolver<Buffer>>()?;
        let registry = self.txs.clone();
        actor.execute(
            query,
            params,
            limit,
            interactive::done(move |result| {
                let encoded = result.map(tasks::encode_query_result_rowarrays);
                deferred.resolve(Box::new(move |_env| match encoded {
                    Ok(bytes) => Ok(Buffer::from(bytes?)),
                    Err(message) => {
                        // The actor rolled back; forget the transaction.
                        forget_tx(&registry, tx_id);
                        Err(interactive::napi_err(message))
                    }
                }))
            }),
        );
        Ok(promise)
    }

    /// Run several statements inside interactive transaction `tx_id` in one
    /// call. Results come back in statement order. The first failing
    /// statement stops the batch and rolls the transaction back; one
    /// timeout bounds the whole batch.
    #[napi(ts_return_type = "Promise<Buffer[]>")]
    pub fn tx_execute_many(
        &self,
        env: Env,
        tx_id: u32,
        #[napi(ts_arg_type = "Array<{ query: string; params?: Record<string, any> | null }>")]
        statements: serde_json::Value,
        #[napi(ts_arg_type = "number | null | undefined")] timeout_ms: Option<u32>,
        #[napi(ts_arg_type = "number | null | undefined")] cancel_token: Option<u32>,
    ) -> Result<JsObject> {
        let actor = self.tx_actor(tx_id)?;
        let limit = self.limit(timeout_ms, cancel_token)?;
        let statements = match tasks::parse_transaction_statements(statements) {
            Ok(statements) => statements
                .into_iter()
                .map(|st| (st.query, st.params))
                .collect(),
            Err(err) => {
                // A rejected call closes the transaction on the JS side, so
                // roll it back here too rather than leave the writer lock
                // held by a handle nothing can reach.
                forget_tx(&self.txs, tx_id);
                actor.rollback(interactive::done(|_| {}));
                return Err(err);
            }
        };
        let (deferred, promise) = env.create_deferred::<Vec<Buffer>, Resolver<Vec<Buffer>>>()?;
        let registry = self.txs.clone();
        actor.execute_many(
            statements,
            limit,
            interactive::done::<Vec<lora_database::QueryResult>>(move |result| {
                let encoded = result.map(|results| {
                    results
                        .into_iter()
                        .map(tasks::encode_query_result_rowarrays)
                        .collect::<Result<Vec<_>>>()
                });
                deferred.resolve(Box::new(move |_env| match encoded {
                    Ok(buffers) => Ok(buffers?.into_iter().map(Buffer::from).collect()),
                    Err(message) => {
                        // The actor rolled back; forget the transaction.
                        forget_tx(&registry, tx_id);
                        Err(interactive::napi_err(message))
                    }
                }))
            }),
        );
        Ok(promise)
    }

    /// Commit (`commit = true`) or roll back interactive transaction `tx_id`.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn tx_finish(&self, env: Env, tx_id: u32, commit: bool) -> Result<JsObject> {
        let actor = self
            .txs
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "transaction registry poisoned"))?
            .remove(&tx_id)
            .ok_or_else(|| interactive::napi_err(tx_closed_message()))?;
        let (deferred, promise) = env.create_deferred::<(), Resolver<()>>()?;
        // The handle rides along to the JS thread and is dropped there, after
        // the actor has finished: its thread is joined off the actor itself.
        let keep = actor.clone();
        let done = interactive::done(move |result| {
            deferred.resolve(Box::new(move |_env| {
                drop(keep);
                result.map_err(interactive::napi_err)
            }))
        });
        if commit {
            actor.commit(done);
        } else {
            actor.rollback(done);
        }
        Ok(promise)
    }

    /// Open a committed-change feed. Resolves with a feed id once the feed
    /// is registered. `on_wake` fires (on the JS thread) whenever the feed
    /// may have something new: call [`Self::changes_poll`] then.
    #[napi(ts_return_type = "Promise<number>")]
    pub fn open_changes(
        &self,
        env: Env,
        #[napi(ts_arg_type = "number | null | undefined")] from_lsn: Option<f64>,
        #[napi(ts_arg_type = "number | null | undefined")] buffer_size: Option<u32>,
        #[napi(ts_arg_type = "() => void")] on_wake: JsFunction,
    ) -> Result<AsyncTask<changes::OpenChangesTask>> {
        let from_lsn = match from_lsn {
            None => None,
            Some(lsn) if lsn.is_finite() && lsn >= 0.0 && lsn.fract() == 0.0 => Some(lsn as u64),
            Some(_) => {
                return Err(NapiError::new(
                    Status::InvalidArg,
                    format!("{INVALID_PARAMS_CODE}: `fromLsn` must be a non-negative integer"),
                ))
            }
        };
        let buffer_size = match buffer_size {
            None => lora_database::DEFAULT_FEED_BUFFER,
            Some(0) => {
                return Err(NapiError::new(
                    Status::InvalidArg,
                    format!("{INVALID_PARAMS_CODE}: `bufferSize` must be greater than 0"),
                ))
            }
            Some(n) => n as usize,
        };
        let mut waker: changes::Waker =
            on_wake.create_threadsafe_function(0, |_ctx| Ok(Vec::<u32>::new()))?;
        waker.unref(&env)?;
        let db = self.inner()?;
        let options = lora_database::ChangeFeedOptions {
            from_lsn,
            buffer_size,
        };
        let opened = db.try_changes(options);
        Ok(AsyncTask::new(changes::OpenChangesTask {
            db,
            options,
            opened,
            waker: Some(waker),
            registry: self.feeds.clone(),
            id: self.next_feed_id.fetch_add(1, Ordering::Relaxed),
        }))
    }

    /// Drain up to `max` batches from feed `feed_id`. Resolves with
    /// `{ batches, closed }`; rejects with `LORA_CHANGES_LAGGED` (or another
    /// coded error) once the feed fails. Never waits for new commits.
    #[napi(
        ts_return_type = "Promise<{ batches: Array<{ lsn: number; changes: Array<Record<string, any>> }>; closed: boolean }>"
    )]
    pub fn changes_poll(
        &self,
        feed_id: u32,
        #[napi(ts_arg_type = "number | null | undefined")] max: Option<u32>,
    ) -> Result<AsyncTask<changes::PollChangesTask>> {
        let feed = self
            .feeds
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "change feed registry poisoned"))?
            .feeds
            .get(&feed_id)
            .cloned()
            .ok_or_else(|| {
                NapiError::new(
                    Status::GenericFailure,
                    "LORA_INTERNAL: change feed is closed",
                )
            })?;
        Ok(AsyncTask::new(changes::PollChangesTask {
            feed,
            max: max.unwrap_or(256).max(1) as usize,
        }))
    }

    /// Close feed `feed_id`. Idempotent.
    #[napi]
    pub fn changes_close(&self, feed_id: u32) -> Result<()> {
        let feed = self
            .feeds
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "change feed registry poisoned"))?
            .feeds
            .remove(&feed_id);
        if let Some(feed) = feed {
            feed.close();
        }
        Ok(())
    }

    /// Force pending WAL bytes and the portable container mirror to disk.
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
        // Dropping an open transaction's actor rolls it back and releases
        // the writer lock. Close every actor before dropping any. Only idle
        // actors are joined: one still waiting for the writer lock (which a
        // transaction on another handle to the same directory may hold) or
        // running a statement is left to finish by itself, and its pending
        // `begin()` rejects as closed.
        let actors = self
            .txs
            .lock()
            .map(|mut txs| std::mem::take(&mut *txs))
            .unwrap_or_default();
        for actor in actors.values() {
            actor.close();
        }
        drop(actors);
        // End every change feed opened through this handle.
        if let Ok(mut feeds) = self.feeds.lock() {
            feeds.close_all();
        }
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

    match sync_mode.as_deref().unwrap_or("groupSync") {
        "groupSync" => Ok(SyncMode::GroupSync { interval_ms }),
        other => Err(NapiError::new(
            Status::InvalidArg,
            format!("{INVALID_PARAMS_CODE}: invalid syncMode '{other}'; expected 'groupSync'"),
        )),
    }
}

impl Default for Database {
    fn default() -> Self {
        Self::new(None, None, None, None, None, None, None, None, None, None)
            .expect("in-memory Database::default should not fail")
    }
}

pub struct NativeQueryStream {
    // Declared first, so dropped first: the stream borrows from a database
    // `Arc` it holds and must go before anything it depends on.
    kind: StreamKind,
    columns: Vec<String>,
    deadline: Option<std::time::Instant>,
    /// Keeps a cancellation handle alive for the stream's lifetime.
    _limit: QueryLimit,
}

enum StreamKind {
    /// Read-only: pulled synchronously on the JS thread.
    Local(stream::LocalStream),
    /// Mutating: opened and pulled on its own thread.
    Actor(stream::StreamActor),
}

type SharedCancel = Arc<Mutex<CancellableDeadline>>;

fn lora_executor_deadline_reached(deadline: std::time::Instant) -> bool {
    lora_database::deadline_reached(deadline)
}

/// How long a query may run: nothing, a plain timeout, or a cancellable
/// deadline shared with JS through a token.
pub(crate) enum QueryLimit {
    None,
    Timeout(std::time::Duration),
    Cancellable(SharedCancel),
}

impl QueryLimit {
    /// Absolute deadline. Call it when the query actually starts (on the
    /// libuv worker, or on the transaction's actor for a statement in an
    /// interactive transaction) so time spent queued does not count
    /// against a plain timeout.
    pub(crate) fn deadline(&self) -> Option<std::time::Instant> {
        match self {
            QueryLimit::None => None,
            QueryLimit::Timeout(d) => Some(
                std::time::Instant::now()
                    .checked_add(*d)
                    .unwrap_or_else(std::time::Instant::now),
            ),
            QueryLimit::Cancellable(handle) => {
                Some(handle.lock().unwrap_or_else(|p| p.into_inner()).deadline())
            }
        }
    }
}

impl Database {
    fn effective_timeout(&self, timeout_ms: Option<u32>) -> Option<std::time::Duration> {
        timeout_ms
            .or(self.default_timeout_ms)
            .filter(|ms| *ms > 0)
            .map(|ms| std::time::Duration::from_millis(u64::from(ms)))
    }

    pub(crate) fn limit(
        &self,
        timeout_ms: Option<u32>,
        cancel_token: Option<u32>,
    ) -> Result<QueryLimit> {
        if let Some(token) = cancel_token {
            let handle = self
                .cancels
                .lock()
                .map_err(|_| NapiError::new(Status::GenericFailure, "cancel registry poisoned"))?
                .get(&token)
                .cloned();
            if let Some(handle) = handle {
                return Ok(QueryLimit::Cancellable(handle));
            }
        }
        Ok(match self.effective_timeout(timeout_ms) {
            Some(d) => QueryLimit::Timeout(d),
            None => QueryLimit::None,
        })
    }
}

/// How a transaction call's promise is settled on the JS thread.
type Resolver<T> = Box<dyn FnOnce(Env) -> Result<T> + Send>;

/// Whether transaction `id` is still registered (on the JS thread).
fn tx_registered(registry: &TxRegistry, id: u32) -> bool {
    registry.lock().is_ok_and(|txs| txs.contains_key(&id))
}

/// Drop transaction `id` from the registry (on the JS thread).
fn forget_tx(registry: &TxRegistry, id: u32) {
    if let Ok(mut txs) = registry.lock() {
        txs.remove(&id);
    }
}

fn tx_closed_message() -> String {
    interactive::closed()
}

impl Database {
    fn tx_actor(&self, tx_id: u32) -> Result<Arc<interactive::TxActor>> {
        self.txs
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "transaction registry poisoned"))?
            .get(&tx_id)
            .cloned()
            .ok_or_else(|| interactive::napi_err(tx_closed_message()))
    }
}
