use std::any::Any;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use lora_analyzer::Analyzer;
use lora_ast::{Direction, Document};
use lora_compiler::{CompiledQuery, Compiler};
use lora_executor::{
    classify_stream, compiled_result_columns, lora_value_to_property, project_rows, ExecuteOptions,
    LoraValue, MutableExecutionContext, MutableExecutor, QueryResult, Row, StreamShape,
};
use lora_parser::parse_query;
use lora_snapshot::{
    decode_snapshot as decode_database_snapshot, write_snapshot as write_database_snapshot,
    Compression, EncryptionKey, PasswordKdfParams, SnapshotCredentials, SnapshotEncryption,
    SnapshotInfo, SnapshotOptions, SnapshotPassword, DATABASE_SNAPSHOT_MAGIC,
};
use lora_store::{
    GraphStorage, GraphStorageMut, InMemoryGraph, MutationEvent, MutationRecorder, NodeId,
    NodeRecord, Properties, RelationshipId, RelationshipRecord, SnapshotMeta, Snapshotable,
};
use lora_wal::{replay_dir, Lsn, Wal, WalConfig, WalMirror, WalRecorder, WroteCommit};

use crate::archive::WalArchive;
use crate::named::{DatabaseName, DatabaseOpenOptions};
use crate::snapshot_store::{ManagedSnapshotStore, SnapshotConfig};
use crate::stream::{AutoCommitGuard, LiveCursor, QueryStream};
use crate::transaction::{LiveStoreGuard, Transaction, TransactionMode};

/// Minimal abstraction any transport can depend on to run Lora queries.
pub trait QueryRunner: Send + Sync + 'static {
    fn execute(&self, query: &str, options: Option<ExecuteOptions>) -> Result<QueryResult>;
}

/// Owns the graph store and orchestrates parse → analyze → compile → execute.
///
/// Optionally drives a write-ahead log: when constructed via
/// [`Database::open_with_wal`] or [`Database::recover`] the database
/// holds an [`Arc<WalRecorder>`] that brackets every query with
/// `begin → mutations → commit/abort → flush` while the store write
/// lock is held, so the WAL order is exactly the in-memory commit order.
/// When constructed via [`Database::in_memory`] / [`Database::from_graph`]
/// the WAL handle is `None` and the engine pays only the existing
/// `MutationRecorder::record` null-pointer check per mutation.
pub struct Database<S> {
    pub(crate) store: Arc<RwLock<S>>,
    pub(crate) wal: Option<Arc<WalRecorder>>,
    pub(crate) snapshots: Option<Arc<ManagedSnapshotStore>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphDirection {
    Outgoing,
    Incoming,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotByteFormat {
    Database,
    LegacyStore,
}

impl SnapshotByteFormat {
    pub fn detect(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(DATABASE_SNAPSHOT_MAGIC) {
            Some(Self::Database)
        } else if bytes.starts_with(lora_store::SNAPSHOT_MAGIC) {
            Some(Self::LegacyStore)
        } else {
            None
        }
    }
}

impl GraphDirection {
    fn as_store_direction(self) -> Direction {
        match self {
            Self::Outgoing => Direction::Right,
            Self::Incoming => Direction::Left,
            Self::Both => Direction::Undirected,
        }
    }
}

fn values_to_properties(values: BTreeMap<String, LoraValue>) -> Result<Properties> {
    values
        .into_iter()
        .map(|(key, value)| {
            let value = lora_value_to_property(value).map_err(|e| anyhow!(e))?;
            Ok((key, value))
        })
        .collect()
}

const DEFAULT_SNAPSHOT_KEY_ID: &str = "default";

/// Build snapshot save options from the JSON shape used by the language
/// bindings.
///
/// Supported shape:
///
/// `{ compression?: "none" | "gzip" | { format: "gzip", level?: number },
///    encryption?: { type: "password", keyId?: string, password: string,
///                   params?: { memoryCostKib?: number, timeCost?: number,
///                              parallelism?: number } } }`
///
/// `encryption` may also be a raw 32-byte key object with
/// `{ type: "key", keyId?: string, key: number[] }`.
pub fn snapshot_options_from_json(value: Option<serde_json::Value>) -> Result<SnapshotOptions> {
    let Some(value) = value else {
        return Ok(SnapshotOptions {
            compression: Compression::None,
            encryption: None,
        });
    };
    if value.is_null() {
        return Ok(SnapshotOptions {
            compression: Compression::None,
            encryption: None,
        });
    }

    let compression = match value.get("compression") {
        Some(value) => parse_snapshot_compression_json(value)?,
        None => Compression::None,
    };
    let encryption = parse_snapshot_credentials_json(Some(value))?;

    Ok(SnapshotOptions {
        compression,
        encryption,
    })
}

/// Build snapshot load credentials from the JSON shape used by the language
/// bindings. The credential object may be supplied directly, or under
/// `credentials` / `encryption` so the same options object can be reused for
/// save and load.
pub fn snapshot_credentials_from_json(
    value: Option<serde_json::Value>,
) -> Result<Option<SnapshotCredentials>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    parse_snapshot_credentials_json(Some(value))
}

fn parse_snapshot_compression_json(value: &serde_json::Value) -> Result<Compression> {
    match value {
        serde_json::Value::Null => Ok(Compression::None),
        serde_json::Value::String(format) => snapshot_compression_from_parts(format, None),
        serde_json::Value::Object(obj) => {
            let format =
                string_field(obj, &["format", "type"])?.unwrap_or_else(|| "none".to_string());
            let level = u32_field(obj, &["level"])?;
            snapshot_compression_from_parts(&format, level)
        }
        _ => Err(anyhow!(
            "snapshot compression must be a string or object with a format field"
        )),
    }
}

fn snapshot_compression_from_parts(format: &str, level: Option<u32>) -> Result<Compression> {
    match format {
        "none" | "identity" | "uncompressed" => Ok(Compression::None),
        "gzip" => {
            let level = level.unwrap_or(1);
            if level > 9 {
                return Err(anyhow!(
                    "gzip snapshot compression level must be between 0 and 9"
                ));
            }
            Ok(Compression::Gzip { level })
        }
        other => Err(anyhow!("unknown snapshot compression '{other}'")),
    }
}

fn parse_snapshot_credentials_json(
    value: Option<serde_json::Value>,
) -> Result<Option<SnapshotCredentials>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }

    let credential_value = if let Some(credentials) = value.get("credentials") {
        credentials
    } else if let Some(encryption) = value.get("encryption") {
        encryption
    } else if looks_like_snapshot_encryption(&value) {
        &value
    } else {
        return Ok(None);
    };

    if credential_value.is_null() {
        return Ok(None);
    }

    Ok(Some(parse_snapshot_encryption_json(credential_value)?))
}

fn looks_like_snapshot_encryption(value: &serde_json::Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    obj.contains_key("password")
        || obj.contains_key("key")
        || obj.contains_key("keyBytes")
        || obj.contains_key("key_bytes")
}

fn parse_snapshot_encryption_json(value: &serde_json::Value) -> Result<SnapshotEncryption> {
    let serde_json::Value::Object(obj) = value else {
        return Err(anyhow!("snapshot encryption must be an object"));
    };

    let kind = string_field(obj, &["type", "kind"])?.unwrap_or_else(|| {
        if obj.contains_key("key") || obj.contains_key("keyBytes") || obj.contains_key("key_bytes")
        {
            "key".to_string()
        } else {
            "password".to_string()
        }
    });

    match kind.as_str() {
        "password" | "passphrase" => {
            let key_id = string_field(obj, &["keyId", "key_id"])?
                .unwrap_or_else(|| DEFAULT_SNAPSHOT_KEY_ID.to_string());
            let password = required_string_field(obj, &["password"])?;
            let params = parse_password_kdf_params(
                obj.get("params")
                    .or_else(|| obj.get("kdfParams"))
                    .or_else(|| obj.get("kdf_params")),
            )?;
            Ok(SnapshotEncryption::Password(SnapshotPassword::with_params(
                key_id, password, params,
            )))
        }
        "key" | "raw_key" | "rawKey" => {
            let key_id = string_field(obj, &["keyId", "key_id"])?
                .unwrap_or_else(|| DEFAULT_SNAPSHOT_KEY_ID.to_string());
            let key = required_key_field(obj, &["key", "keyBytes", "key_bytes"])?;
            Ok(SnapshotEncryption::Key(EncryptionKey::new(key_id, key)))
        }
        other => Err(anyhow!("unknown snapshot encryption type '{other}'")),
    }
}

fn parse_password_kdf_params(value: Option<&serde_json::Value>) -> Result<PasswordKdfParams> {
    let Some(value) = value else {
        return Ok(PasswordKdfParams::interactive());
    };
    if value.is_null() {
        return Ok(PasswordKdfParams::interactive());
    }
    let serde_json::Value::Object(obj) = value else {
        return Err(anyhow!("snapshot password params must be an object"));
    };

    let defaults = PasswordKdfParams::interactive();
    let memory_cost_kib =
        u32_field(obj, &["memoryCostKib", "memory_cost_kib"])?.unwrap_or(defaults.memory_cost_kib);
    let time_cost = u32_field(obj, &["timeCost", "time_cost"])?.unwrap_or(defaults.time_cost);
    let parallelism = u32_field(obj, &["parallelism"])?.unwrap_or(defaults.parallelism);

    if memory_cost_kib == 0 || time_cost == 0 || parallelism == 0 {
        return Err(anyhow!(
            "snapshot password params memoryCostKib, timeCost, and parallelism must be greater than zero"
        ));
    }

    Ok(PasswordKdfParams {
        memory_cost_kib,
        time_cost,
        parallelism,
    })
}

fn string_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    names: &[&str],
) -> Result<Option<String>> {
    for name in names {
        if let Some(value) = obj.get(*name) {
            return match value {
                serde_json::Value::Null => Ok(None),
                serde_json::Value::String(value) => Ok(Some(value.clone())),
                _ => Err(anyhow!("snapshot field '{name}' must be a string")),
            };
        }
    }
    Ok(None)
}

fn required_string_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    names: &[&str],
) -> Result<String> {
    string_field(obj, names)?.ok_or_else(|| anyhow!("snapshot field '{}' is required", names[0]))
}

fn u32_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    names: &[&str],
) -> Result<Option<u32>> {
    for name in names {
        if let Some(value) = obj.get(*name) {
            return match value {
                serde_json::Value::Null => Ok(None),
                serde_json::Value::Number(number) => {
                    let Some(value) = number.as_u64() else {
                        return Err(anyhow!(
                            "snapshot field '{name}' must be a non-negative integer"
                        ));
                    };
                    if value > u32::MAX as u64 {
                        return Err(anyhow!("snapshot field '{name}' is too large"));
                    }
                    Ok(Some(value as u32))
                }
                _ => Err(anyhow!("snapshot field '{name}' must be an integer")),
            };
        }
    }
    Ok(None)
}

fn required_key_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    names: &[&str],
) -> Result<[u8; 32]> {
    for name in names {
        if let Some(value) = obj.get(*name) {
            return parse_key_bytes(value, name);
        }
    }
    Err(anyhow!("snapshot field '{}' is required", names[0]))
}

fn parse_key_bytes(value: &serde_json::Value, field_name: &str) -> Result<[u8; 32]> {
    let serde_json::Value::Array(values) = value else {
        return Err(anyhow!(
            "snapshot field '{field_name}' must be an array of 32 byte values"
        ));
    };
    if values.len() != 32 {
        return Err(anyhow!(
            "snapshot field '{field_name}' must contain exactly 32 byte values"
        ));
    }

    let mut out = [0u8; 32];
    for (idx, value) in values.iter().enumerate() {
        let Some(byte) = value.as_u64() else {
            return Err(anyhow!(
                "snapshot field '{field_name}' item {idx} must be a byte integer"
            ));
        };
        if byte > u8::MAX as u64 {
            return Err(anyhow!(
                "snapshot field '{field_name}' item {idx} must be between 0 and 255"
            ));
        }
        out[idx] = byte as u8;
    }
    Ok(out)
}

fn snapshot_info_to_meta(info: SnapshotInfo) -> SnapshotMeta {
    SnapshotMeta {
        format_version: info.format_version,
        node_count: info.node_count,
        relationship_count: info.relationship_count,
        wal_lsn: info.wal_lsn,
    }
}

impl Database<InMemoryGraph> {
    /// Convenience constructor: a fresh, empty in-memory graph database.
    pub fn in_memory() -> Self {
        Self::from_graph(InMemoryGraph::new())
    }

    /// Open or create a WAL-enabled in-memory database from a fresh
    /// graph.
    ///
    /// `WalConfig::Disabled` falls back to [`Database::in_memory`].
    /// Otherwise, opens the WAL directory, replays any committed
    /// events into a fresh graph, installs a [`WalRecorder`] on the
    /// graph, and returns a database ready to serve queries.
    ///
    /// To restore from a snapshot in addition to the WAL, use
    /// [`Database::recover`] instead.
    pub fn open_with_wal(wal_config: WalConfig) -> Result<Self> {
        match wal_config {
            WalConfig::Disabled => Ok(Self::in_memory()),
            WalConfig::Enabled {
                dir,
                sync_mode,
                segment_target_bytes,
            } => {
                let mut graph = InMemoryGraph::new();
                let (wal, events) = Wal::open(dir, sync_mode, segment_target_bytes, Lsn::ZERO)?;
                replay_into(&mut graph, events)?;
                let recorder = Arc::new(WalRecorder::new(wal));
                graph.set_mutation_recorder(Some(recorder.clone() as Arc<dyn MutationRecorder>));
                Ok(Self {
                    store: Arc::new(RwLock::new(graph)),
                    wal: Some(recorder),
                    snapshots: None,
                })
            }
        }
    }

    /// Open or create a WAL-backed database with managed snapshots beside it.
    ///
    /// Recovery loads the newest managed snapshot first, then replays WAL
    /// records above the snapshot's LSN fence. Checkpoints are written through
    /// [`Self::checkpoint_managed`] / [`Self::sync`], or automatically when
    /// `snapshot_config.checkpoint_every_commits` is set.
    pub fn open_with_wal_snapshots(
        wal_config: WalConfig,
        snapshot_config: SnapshotConfig,
    ) -> Result<Self> {
        let snapshot_store = Arc::new(ManagedSnapshotStore::open(snapshot_config)?);
        let mut graph = InMemoryGraph::new();

        match wal_config {
            WalConfig::Disabled => Err(anyhow!("managed snapshots require WAL enabled")),
            WalConfig::Enabled {
                dir,
                sync_mode,
                segment_target_bytes,
            } => {
                let snapshot_lsn = snapshot_store.load_latest(&mut graph)?;
                let (wal, events) = Wal::open(dir, sync_mode, segment_target_bytes, snapshot_lsn)?;
                replay_into(&mut graph, events)?;
                let recorder = Arc::new(WalRecorder::new(wal));
                graph.set_mutation_recorder(Some(recorder.clone() as Arc<dyn MutationRecorder>));
                Ok(Self {
                    store: Arc::new(RwLock::new(graph)),
                    wal: Some(recorder),
                    snapshots: Some(snapshot_store),
                })
            }
        }
    }

    /// Open or create a named portable database rooted under
    /// `options.database_dir`.
    ///
    /// The database name may be either a portable basename (`app` or
    /// `app.loradb`) or a safe relative path (`tenant/app`). It is resolved
    /// under `options.database_dir` before the WAL archive backend opens.
    pub fn open_named(
        database_name: impl AsRef<str>,
        options: DatabaseOpenOptions,
    ) -> Result<Self> {
        let name = DatabaseName::parse(database_name.as_ref())?;
        let archive = Arc::new(WalArchive::open(
            options.database_path_for(&name),
            options.max_database_bytes,
        )?);
        let mut graph = InMemoryGraph::new();
        let (wal, events) = Wal::open(
            archive.work_dir(),
            options.sync_mode,
            options.segment_target_bytes,
            Lsn::ZERO,
        )?;
        replay_into(&mut graph, events)?;
        let mirror: Arc<dyn WalMirror> = archive;
        let recorder = Arc::new(WalRecorder::new_with_mirror(wal, Some(mirror)));
        graph.set_mutation_recorder(Some(recorder.clone() as Arc<dyn MutationRecorder>));
        // Mark the archive dirty so a fresh named database is materialized as
        // a portable ZIP. The archive writer coalesces this with any immediate
        // follow-up writes and flushes it in the background, with a final flush
        // on database drop.
        recorder
            .flush()
            .map_err(|e| anyhow!("initial database archive persist failed: {e}"))?;
        Ok(Self {
            store: Arc::new(RwLock::new(graph)),
            wal: Some(recorder),
            snapshots: None,
        })
    }

    /// Start an explicit transaction.
    ///
    /// Read-only transactions hold a shared read lock for their
    /// lifetime; read-write transactions hold the write lock. The
    /// staging clone is **lazy** — it only happens when a
    /// [`TransactionMode::ReadWrite`] transaction sees its first
    /// mutating statement. Materialized read-only statements run
    /// straight against the live graph; tx-bound streams may still
    /// clone so their cursors can own a stable view. ReadWrite
    /// transactions that perform only materialized reads (or commit
    /// empty) pay nothing for staging.
    pub fn begin_transaction(&self, mode: TransactionMode) -> Result<Transaction<'_>> {
        let live = match mode {
            TransactionMode::ReadOnly => LiveStoreGuard::Read(self.read_store()),
            TransactionMode::ReadWrite => LiveStoreGuard::Write(self.write_store()),
        };
        Ok(Transaction::new(
            live,
            self.wal.clone(),
            self.snapshots.clone(),
            mode,
        ))
    }

    /// Force any pending WAL bytes to durable storage and, for archive-backed
    /// databases, refresh the portable `.loradb` file before returning.
    ///
    /// Managed snapshot checkpoints are explicit via
    /// [`Self::checkpoint_managed`] or threshold-driven via
    /// [`SnapshotConfig::checkpoint_every_commits`]; `sync()` remains a
    /// durability operation rather than an O(graph) checkpoint.
    pub fn sync(&self) -> Result<()> {
        if let Some(wal) = &self.wal {
            wal.force_fsync()?;
        }
        Ok(())
    }

    /// Restore from a snapshot file then replay any WAL records past
    /// it.
    ///
    /// The snapshot's `wal_lsn` (when set) becomes the replay fence —
    /// events at or below that LSN are already represented in the
    /// loaded snapshot and are skipped. A missing snapshot file is
    /// treated as "fresh start" so operators can pass the same path
    /// on every boot.
    ///
    /// If the WAL contains a checkpoint marker newer than the
    /// snapshot's `wal_lsn`, a one-line warning is printed to stderr
    /// — the snapshot is stale relative to a more recent checkpoint
    /// the operator is presumably aware of. Recovery still proceeds
    /// from the snapshot's fence (replay re-applies every record
    /// above it, which is conservative-correct); a tighter contract
    /// is deferred to v2 because verifying that the marker's
    /// snapshot file actually exists and is loadable is a separate
    /// observability concern.
    pub fn recover(snapshot_path: impl AsRef<Path>, wal_config: WalConfig) -> Result<Self> {
        let snapshot_path = snapshot_path.as_ref();
        let mut graph = InMemoryGraph::new();
        let snapshot_lsn = match File::open(snapshot_path) {
            Ok(f) => {
                let reader = BufReader::new(f);
                let meta = graph.load_snapshot(reader)?;
                meta.wal_lsn.map(Lsn::new).unwrap_or(Lsn::ZERO)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Lsn::ZERO,
            Err(e) => return Err(e.into()),
        };

        match wal_config {
            WalConfig::Disabled => Ok(Self::from_graph(graph)),
            WalConfig::Enabled {
                dir,
                sync_mode,
                segment_target_bytes,
            } => {
                // Diagnostic peek at the WAL's newest checkpoint
                // marker so we can warn the operator about a stale
                // snapshot before we start replaying. Treat any error
                // as "no marker" — the subsequent `Wal::open` will
                // surface the real failure if there is one.
                if dir.exists() {
                    if let Ok(outcome) = replay_dir(&dir, Lsn::ZERO) {
                        if let Some(marker) = outcome.checkpoint_lsn_observed {
                            if marker > snapshot_lsn {
                                eprintln!(
                                    "lora-wal: snapshot at LSN {} is older than the newest \
                                     checkpoint marker on disk (LSN {}). Replaying every WAL \
                                     record above LSN {}; consider passing the more recent \
                                     snapshot to --restore-from.",
                                    snapshot_lsn.raw(),
                                    marker.raw(),
                                    snapshot_lsn.raw()
                                );
                            }
                        }
                    }
                }

                let (wal, events) = Wal::open(dir, sync_mode, segment_target_bytes, snapshot_lsn)?;
                replay_into(&mut graph, events)?;
                let recorder = Arc::new(WalRecorder::new(wal));
                graph.set_mutation_recorder(Some(recorder.clone() as Arc<dyn MutationRecorder>));
                Ok(Self {
                    store: Arc::new(RwLock::new(graph)),
                    wal: Some(recorder),
                    snapshots: None,
                })
            }
        }
    }

    /// Execute a query and return an owning row stream.
    pub fn stream(&self, query: &str) -> Result<QueryStream<'_>> {
        self.stream_with_params(query, BTreeMap::new())
    }

    /// Execute a parameterised query and return an owning row stream.
    ///
    /// The compiled plan is classified at open time. Read-only
    /// queries run directly off the live store and yield a
    /// buffered cursor with plan-derived columns. Mutating queries
    /// are routed through a hidden read-write [`Transaction`]:
    /// full cursor exhaustion calls `tx.commit` (publishing staged
    /// changes and replaying the tx-local WAL buffer); a premature
    /// drop or any error from `next_row` calls `tx.rollback` so
    /// the live store and the WAL stay untouched.
    pub fn stream_with_params(
        &self,
        query: &str,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<QueryStream<'_>> {
        // Classify by compiling once against the live store. The
        // mutating branch then re-compiles inside the hidden
        // transaction (against a staged graph that's identical to
        // live at clone time, so the second plan matches the
        // first); the cost is one extra parse+analyze+compile per
        // mutating stream, paid in exchange for a tiny
        // classify-stream surface.
        let document = parse_query(query)?;
        let store_guard = self.read_store();
        let resolved = {
            let mut analyzer = Analyzer::new(&*store_guard);
            analyzer.analyze(&document)?
        };
        let compiled = Compiler::compile(&resolved);
        let columns = compiled_result_columns(&compiled);
        let shape = classify_stream(&compiled);
        // Release the analyzer's lock before either branch
        // re-acquires (read-only path keeps it; mutating path
        // delegates to begin_transaction which takes its own).
        drop(store_guard);

        match shape {
            StreamShape::ReadOnly => {
                // True pull-shaped streaming. `LiveCursor` holds
                // the live store lock and the cursor that
                // borrows from it; its `Drop` releases them in
                // the right order so the caller observes pure
                // pull semantics with no intermediate
                // materialization.
                let live = LiveCursor::open(self.store.clone(), compiled, params)?;
                Ok(QueryStream::live(live, columns))
            }
            StreamShape::Mutating => {
                // Hidden auto-commit transaction. The transaction
                // owns staging, the buffering recorder, savepoint
                // management, and the WAL replay-on-commit logic;
                // we just pick commit-on-exhaustion vs
                // rollback-on-drop based on cursor state.
                //
                // The cursor returned by `open_streaming_compiled_autocommit`
                // may be a real per-row `StreamingWriteCursor`,
                // a mutable UNION cursor, or a buffered leaf for
                // operators that still need full materialization.
                // Either way the AutoCommit guard's
                // drop/exhaustion semantics are identical. The
                // compiled plan is wrapped in an `Arc` so the
                // cursor's `'static` borrows into it remain valid
                // for the cursor's lifetime.
                let mut tx = self.begin_transaction(TransactionMode::ReadWrite)?;
                let compiled_arc = Arc::new(compiled);
                let cursor =
                    match tx.open_streaming_compiled_autocommit(compiled_arc.clone(), params) {
                        Ok(c) => c,
                        Err(err) => {
                            // Tx rolls back implicitly on drop here.
                            return Err(err);
                        }
                    };
                let guard = AutoCommitGuard {
                    tx: Some(tx),
                    finalized: false,
                };
                Ok(QueryStream::auto_commit(cursor, columns, guard))
            }
        }
    }

    /// Open a stream whose lifetime can be carried by an outer owner that
    /// also retains an `Arc<Database>`.
    ///
    /// # Safety
    ///
    /// The returned stream may contain lock guards that borrow from the
    /// database's internal `RwLock`. The caller must keep this exact `Arc`
    /// alive until the stream is dropped. This is intended for language
    /// bindings that store both the `Arc<Database>` and the `QueryStream` in
    /// the same opaque stream handle.
    pub unsafe fn stream_with_params_owned(
        self: &Arc<Self>,
        query: &str,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<QueryStream<'static>> {
        let stream = self.stream_with_params(query, params)?;
        Ok(std::mem::transmute::<QueryStream<'_>, QueryStream<'static>>(stream))
    }
}

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any,
{
    /// Build a database from a pre-wrapped, shared store.
    pub fn new(store: Arc<RwLock<S>>) -> Self {
        Self {
            store,
            wal: None,
            snapshots: None,
        }
    }

    /// Build a database by taking ownership of a bare graph store.
    pub fn from_graph(graph: S) -> Self {
        Self::new(Arc::new(RwLock::new(graph)))
    }

    /// Handle to the installed WAL recorder, if any. Exposed for
    /// admin paths (checkpoint, truncate, observability) that need
    /// to drive the WAL outside the standard query lifecycle.
    pub fn wal(&self) -> Option<&Arc<WalRecorder>> {
        self.wal.as_ref()
    }

    fn observe_snapshot_commit_if_needed(&self, store: &S, recorder: &WalRecorder) -> Result<()> {
        let Some(snapshots) = &self.snapshots else {
            return Ok(());
        };
        let graph = (store as &dyn Any)
            .downcast_ref::<InMemoryGraph>()
            .ok_or_else(|| anyhow!("managed snapshots require InMemoryGraph storage"))?;
        snapshots.observe_commit(graph, recorder)?;
        Ok(())
    }

    /// Handle to the underlying shared store — useful for callers that need
    /// to snapshot or share the graph across multiple databases.
    pub fn store(&self) -> &Arc<RwLock<S>> {
        &self.store
    }

    /// Parse a query string into an AST without executing it.
    pub fn parse(&self, query: &str) -> Result<Document> {
        Ok(parse_query(query)?)
    }

    pub(crate) fn read_store(&self) -> RwLockReadGuard<'_, S> {
        self.store
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn write_store(&self) -> RwLockWriteGuard<'_, S> {
        self.store
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn read_store_deadline(&self, deadline: Option<Instant>) -> Result<RwLockReadGuard<'_, S>> {
        let Some(deadline) = deadline else {
            return Ok(self.read_store());
        };

        loop {
            match self.store.try_read() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(poisoned)) => return Ok(poisoned.into_inner()),
                Err(TryLockError::WouldBlock) if Instant::now() >= deadline => {
                    return Err(anyhow!("query deadline exceeded"));
                }
                Err(TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    fn write_store_deadline(&self, deadline: Option<Instant>) -> Result<RwLockWriteGuard<'_, S>> {
        let Some(deadline) = deadline else {
            return Ok(self.write_store());
        };

        loop {
            match self.store.try_write() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(poisoned)) => return Ok(poisoned.into_inner()),
                Err(TryLockError::WouldBlock) if Instant::now() >= deadline => {
                    return Err(anyhow!("query deadline exceeded"));
                }
                Err(TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    fn compile_document_against(&self, document: &Document, store: &S) -> Result<CompiledQuery> {
        let resolved = {
            let mut analyzer = Analyzer::new(store);
            analyzer.analyze(document)?
        };

        Ok(Compiler::compile(&resolved))
    }

    /// Execute a query and return its result.
    pub fn execute(&self, query: &str, options: Option<ExecuteOptions>) -> Result<QueryResult> {
        self.execute_with_params(query, options, BTreeMap::new())
    }

    /// Execute a query with a cooperative deadline. The timeout is checked at
    /// executor operator boundaries and hot scan loops; if it fires, the query
    /// returns an error and any WAL-backed mutating query is aborted through
    /// the existing failure path.
    pub fn execute_with_timeout(
        &self,
        query: &str,
        options: Option<ExecuteOptions>,
        timeout: Duration,
    ) -> Result<QueryResult> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        let rows =
            self.execute_rows_with_params_deadline(query, BTreeMap::new(), Some(deadline))?;
        Ok(project_rows(rows, options.unwrap_or_default()))
    }

    /// Execute a query with bound parameters.
    ///
    /// When a WAL is attached the call is bracketed by a transaction:
    ///
    /// 1. `recorder.arm()` after analyze + compile (so a parse /
    ///    semantic / compile error never opens a tx that has to be
    ///    immediately aborted). Arming is *cheap*: no record is
    ///    appended to the WAL yet, so a pure read query that
    ///    completes here pays nothing for the WAL hot path.
    /// 2. The executor runs; every primitive mutation fires
    ///    `MutationRecorder::record`, which buffers events in memory.
    /// 3. On Ok, `recorder.commit()` writes `TxBegin`, one batched
    ///    mutation record, and `TxCommit` only when mutations occurred;
    ///    the surrounding `recorder.flush()` runs only in that case so
    ///    a read-only query never pays an `fsync`.
    /// 4. On Err, `recorder.abort()` clears the pending batch. The
    ///    engine has no rollback, so the in-memory state may already
    ///    be partially mutated; the live handle is quarantined while
    ///    durable recovery stays atomic because no committed batch was
    ///    written.
    /// 5. The recorder's poisoned flag is polled once (it also
    ///    surfaces background-flusher fsync failures from
    ///    `SyncMode::Group`). If set, the query fails loudly with the
    ///    durability error so the caller can act on it; the WAL
    ///    refuses further appends until the operator restarts the
    ///    database, which recovers from the last consistent
    ///    snapshot + WAL.
    pub fn execute_with_params(
        &self,
        query: &str,
        options: Option<ExecuteOptions>,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<QueryResult> {
        let rows = self.execute_rows_with_params_deadline(query, params, None)?;
        Ok(project_rows(rows, options.unwrap_or_default()))
    }

    /// Execute a parameterised query with a cooperative deadline.
    pub fn execute_with_params_timeout(
        &self,
        query: &str,
        options: Option<ExecuteOptions>,
        params: BTreeMap<String, LoraValue>,
        timeout: Duration,
    ) -> Result<QueryResult> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        let rows = self.execute_rows_with_params_deadline(query, params, Some(deadline))?;
        Ok(project_rows(rows, options.unwrap_or_default()))
    }

    /// Execute a query and return hydrated rows before final result-format
    /// projection.
    pub fn execute_rows(&self, query: &str) -> Result<Vec<Row>> {
        self.execute_rows_with_params(query, BTreeMap::new())
    }

    /// Execute a query with parameters and return hydrated rows before final
    /// result-format projection.
    pub fn execute_rows_with_params(
        &self,
        query: &str,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Vec<Row>> {
        self.execute_rows_with_params_deadline(query, params, None)
    }

    fn execute_rows_with_params_deadline(
        &self,
        query: &str,
        params: BTreeMap<String, LoraValue>,
        deadline: Option<Instant>,
    ) -> Result<Vec<Row>> {
        let document = self.parse(query)?;
        let shape = {
            let store = self.read_store_deadline(deadline)?;
            let compiled = self.compile_document_against(&document, &*store)?;

            if matches!(classify_stream(&compiled), StreamShape::ReadOnly) {
                if let Some(rec) = &self.wal {
                    if let Some(reason) = rec.poisoned() {
                        return Err(anyhow!("WAL arm failed: WAL poisoned: {reason}"));
                    }
                }
                let executor = lora_executor::Executor::with_deadline(
                    lora_executor::ExecutionContext {
                        storage: &*store,
                        params,
                    },
                    deadline,
                );
                return executor
                    .execute_compiled_rows(&compiled)
                    .map_err(|e| anyhow!(e));
            }

            classify_stream(&compiled)
        };

        debug_assert!(shape.is_mutating());

        let mut store = self.write_store_deadline(deadline)?;
        let compiled = self.compile_document_against(&document, &*store)?;

        if let Some(rec) = &self.wal {
            rec.arm().map_err(|e| anyhow!("WAL arm failed: {e}"))?;
        }

        let exec_result: Result<Vec<Row>> = (|| {
            let mut executor = MutableExecutor::with_deadline(
                MutableExecutionContext {
                    storage: &mut *store,
                    params,
                },
                deadline,
            );
            Ok(executor.execute_compiled_rows(&compiled)?)
        })();

        if let Some(rec) = &self.wal {
            match &exec_result {
                Ok(_) => match rec.commit() {
                    Ok(WroteCommit::Yes) => {
                        rec.flush().map_err(|e| anyhow!("WAL flush failed: {e}"))?;
                        self.observe_snapshot_commit_if_needed(&store, rec)?;
                    }
                    Ok(WroteCommit::No) => {
                        // Read-only query: no records were written
                        // and there is nothing to fsync. Skip flush
                        // entirely so PerCommit pays zero fsyncs on
                        // pure reads.
                    }
                    Err(e) => return Err(anyhow!("WAL commit failed: {e}")),
                },
                Err(_) => {
                    // Best-effort abort. If the WAL saw mutations, durable
                    // recovery will discard them but the live in-memory store
                    // may already be ahead of durable state. Quarantine this
                    // handle so callers restart instead of serving from a
                    // potentially divergent graph.
                    if matches!(rec.abort(), Ok(true)) {
                        rec.poison(
                            "query mutated the live graph before failing; restart from snapshot + WAL required",
                        );
                    }
                }
            }
            if let Some(reason) = rec.poisoned() {
                return Err(anyhow!("WAL poisoned: {reason}"));
            }
        }

        exec_result
    }

    // ---------- Storage-agnostic utility helpers ----------
    //
    // Bindings previously reached into the shared store lock to answer
    // stat / admin calls; these helpers let them depend on `Database<S>`
    // instead, so swapping in a new backend only requires changing one type
    // parameter.

    /// Drop every node and relationship, returning WAL/archive errors to the
    /// caller.
    ///
    /// When a WAL is attached, the clear is wrapped in `arm`/`commit` so the
    /// `MutationEvent::Clear` fired by the store reaches the log inside a
    /// transaction. If a failure happens after the in-memory graph has been
    /// cleared, the recorder is poisoned by the failing WAL path and future
    /// writes fail until the database is reopened from durable state.
    pub fn try_clear(&self) -> Result<()> {
        let mut guard = self.write_store();
        let Some(rec) = &self.wal else {
            guard.clear();
            return Ok(());
        };

        rec.arm().map_err(|e| anyhow!("WAL arm failed: {e}"))?;
        guard.clear();
        match rec.commit() {
            Ok(WroteCommit::Yes) => {
                rec.flush().map_err(|e| anyhow!("WAL flush failed: {e}"))?;
                self.observe_snapshot_commit_if_needed(&guard, rec)?;
            }
            Ok(WroteCommit::No) => {}
            Err(e) => return Err(anyhow!("WAL commit failed: {e}")),
        }
        if let Some(reason) = rec.poisoned() {
            return Err(anyhow!("WAL poisoned: {reason}"));
        }
        Ok(())
    }

    /// Drop every node and relationship.
    ///
    /// This compatibility helper keeps the historical infallible Rust API.
    /// Bindings that can report errors should call [`Self::try_clear`].
    pub fn clear(&self) {
        let _ = self.try_clear();
    }

    /// Number of nodes currently in the graph.
    pub fn node_count(&self) -> usize {
        let guard = self.read_store();
        guard.node_count()
    }

    /// Number of relationships currently in the graph.
    pub fn relationship_count(&self) -> usize {
        let guard = self.read_store();
        guard.relationship_count()
    }

    /// Run a closure with a shared borrow of the underlying store. Used by
    /// bindings to answer ad-hoc queries without locking the RwLock themselves.
    pub fn with_store<R>(&self, f: impl FnOnce(&S) -> R) -> R {
        let guard = self.read_store();
        f(&*guard)
    }

    /// Run a closure with an exclusive borrow of the underlying store. Reserved
    /// for admin paths (restore, bulk load); regular mutation goes through
    /// `execute_with_params`.
    pub fn with_store_mut<R>(&self, f: impl FnOnce(&mut S) -> R) -> R {
        let mut guard = self.write_store();
        f(&mut *guard)
    }

    fn with_logged_store_mut<R>(&self, f: impl FnOnce(&mut S) -> Result<R>) -> Result<R> {
        let mut guard = self.write_store();
        let Some(rec) = &self.wal else {
            return f(&mut *guard);
        };

        rec.arm().map_err(|e| anyhow!("WAL arm failed: {e}"))?;
        let result = f(&mut *guard);
        match &result {
            Ok(_) => match rec.commit() {
                Ok(WroteCommit::Yes) => {
                    rec.flush().map_err(|e| anyhow!("WAL flush failed: {e}"))?;
                    self.observe_snapshot_commit_if_needed(&guard, rec)?;
                }
                Ok(WroteCommit::No) => {}
                Err(e) => return Err(anyhow!("WAL commit failed: {e}")),
            },
            Err(_) => {
                let _ = rec.abort();
            }
        }
        if let Some(reason) = rec.poisoned() {
            return Err(anyhow!("WAL poisoned: {reason}"));
        }
        result
    }

    // ---------- Direct graph read surface ----------

    pub fn graph_contains_node(&self, id: NodeId) -> bool {
        self.with_store(|store| store.contains_node(id))
    }

    pub fn graph_node(&self, id: NodeId) -> Option<NodeRecord> {
        self.with_store(|store| store.node(id))
    }

    pub fn graph_all_node_ids(&self) -> Vec<NodeId> {
        self.with_store(|store| store.all_node_ids())
    }

    pub fn graph_node_ids_by_label(&self, label: &str) -> Vec<NodeId> {
        self.with_store(|store| store.node_ids_by_label(label))
    }

    pub fn graph_all_nodes(&self) -> Vec<NodeRecord> {
        self.with_store(|store| store.all_nodes())
    }

    pub fn graph_nodes_by_label(&self, label: &str) -> Vec<NodeRecord> {
        self.with_store(|store| store.nodes_by_label(label))
    }

    pub fn graph_node_has_label(&self, id: NodeId, label: &str) -> bool {
        self.with_store(|store| store.node_has_label(id, label))
    }

    pub fn graph_node_labels(&self, id: NodeId) -> Option<Vec<String>> {
        self.with_store(|store| store.node_labels(id))
    }

    pub fn graph_node_properties(&self, id: NodeId) -> Option<BTreeMap<String, LoraValue>> {
        self.with_store(|store| {
            store.node_properties(id).map(|props| {
                props
                    .into_iter()
                    .map(|(key, value)| (key, LoraValue::from(value)))
                    .collect()
            })
        })
    }

    pub fn graph_node_property(&self, id: NodeId, key: &str) -> Option<LoraValue> {
        self.with_store(|store| store.node_property(id, key).map(LoraValue::from))
    }

    pub fn graph_contains_relationship(&self, id: RelationshipId) -> bool {
        self.with_store(|store| store.contains_relationship(id))
    }

    pub fn graph_relationship(&self, id: RelationshipId) -> Option<RelationshipRecord> {
        self.with_store(|store| store.relationship(id))
    }

    pub fn graph_all_relationship_ids(&self) -> Vec<RelationshipId> {
        self.with_store(|store| store.all_rel_ids())
    }

    pub fn graph_relationship_ids_by_type(&self, rel_type: &str) -> Vec<RelationshipId> {
        self.with_store(|store| store.rel_ids_by_type(rel_type))
    }

    pub fn graph_all_relationships(&self) -> Vec<RelationshipRecord> {
        self.with_store(|store| store.all_relationships())
    }

    pub fn graph_relationships_by_type(&self, rel_type: &str) -> Vec<RelationshipRecord> {
        self.with_store(|store| store.relationships_by_type(rel_type))
    }

    pub fn graph_relationship_endpoints(&self, id: RelationshipId) -> Option<(NodeId, NodeId)> {
        self.with_store(|store| store.relationship_endpoints(id))
    }

    pub fn graph_relationship_type(&self, id: RelationshipId) -> Option<String> {
        self.with_store(|store| store.relationship_type(id))
    }

    pub fn graph_relationship_properties(
        &self,
        id: RelationshipId,
    ) -> Option<BTreeMap<String, LoraValue>> {
        self.with_store(|store| {
            store.relationship_properties(id).map(|props| {
                props
                    .into_iter()
                    .map(|(key, value)| (key, LoraValue::from(value)))
                    .collect()
            })
        })
    }

    pub fn graph_relationship_property(&self, id: RelationshipId, key: &str) -> Option<LoraValue> {
        self.with_store(|store| store.relationship_property(id, key).map(LoraValue::from))
    }

    pub fn graph_relationship_ids_of(
        &self,
        node_id: NodeId,
        direction: GraphDirection,
    ) -> Vec<RelationshipId> {
        self.with_store(|store| store.relationship_ids_of(node_id, direction.as_store_direction()))
    }

    pub fn graph_degree(&self, node_id: NodeId, direction: GraphDirection) -> usize {
        self.with_store(|store| store.degree(node_id, direction.as_store_direction()))
    }

    pub fn graph_neighbors(
        &self,
        node_id: NodeId,
        direction: GraphDirection,
        types: &[String],
    ) -> Vec<NodeRecord> {
        self.with_store(|store| store.neighbors(node_id, direction.as_store_direction(), types))
    }

    pub fn graph_expand_ids(
        &self,
        node_id: NodeId,
        direction: GraphDirection,
        types: &[String],
    ) -> Vec<(RelationshipId, NodeId)> {
        self.with_store(|store| store.expand_ids(node_id, direction.as_store_direction(), types))
    }

    pub fn graph_all_labels(&self) -> Vec<String> {
        self.with_store(|store| store.all_labels())
    }

    pub fn graph_all_relationship_types(&self) -> Vec<String> {
        self.with_store(|store| store.all_relationship_types())
    }

    pub fn graph_all_property_keys(&self) -> Vec<String> {
        self.with_store(|store| store.all_property_keys())
    }

    pub fn graph_all_node_property_keys(&self) -> Vec<String> {
        self.with_store(|store| store.all_node_property_keys())
    }

    pub fn graph_all_relationship_property_keys(&self) -> Vec<String> {
        self.with_store(|store| store.all_relationship_property_keys())
    }

    pub fn graph_label_property_keys(&self, label: &str) -> Vec<String> {
        self.with_store(|store| store.label_property_keys(label))
    }

    pub fn graph_relationship_type_property_keys(&self, rel_type: &str) -> Vec<String> {
        self.with_store(|store| store.rel_type_property_keys(rel_type))
    }

    pub fn graph_find_node_ids_by_property(
        &self,
        label: Option<&str>,
        key: &str,
        value: LoraValue,
    ) -> Result<Vec<NodeId>> {
        let value = lora_value_to_property(value).map_err(|e| anyhow!(e))?;
        Ok(self.with_store(|store| store.find_node_ids_by_property(label, key, &value)))
    }

    pub fn graph_find_relationship_ids_by_property(
        &self,
        rel_type: Option<&str>,
        key: &str,
        value: LoraValue,
    ) -> Result<Vec<RelationshipId>> {
        let value = lora_value_to_property(value).map_err(|e| anyhow!(e))?;
        Ok(self.with_store(|store| store.find_relationship_ids_by_property(rel_type, key, &value)))
    }

    // ---------- Direct graph mutation surface ----------

    pub fn graph_create_node(
        &self,
        labels: Vec<String>,
        properties: BTreeMap<String, LoraValue>,
    ) -> Result<NodeRecord> {
        let properties = values_to_properties(properties)?;
        self.with_logged_store_mut(|store| Ok(store.create_node(labels, properties)))
    }

    pub fn graph_create_relationship(
        &self,
        src: NodeId,
        dst: NodeId,
        rel_type: &str,
        properties: BTreeMap<String, LoraValue>,
    ) -> Result<Option<RelationshipRecord>> {
        let properties = values_to_properties(properties)?;
        self.with_logged_store_mut(|store| {
            Ok(store.create_relationship(src, dst, rel_type, properties))
        })
    }

    pub fn graph_set_node_property(
        &self,
        node_id: NodeId,
        key: String,
        value: LoraValue,
    ) -> Result<bool> {
        let value = lora_value_to_property(value).map_err(|e| anyhow!(e))?;
        self.with_logged_store_mut(|store| Ok(store.set_node_property(node_id, key, value)))
    }

    pub fn graph_remove_node_property(&self, node_id: NodeId, key: &str) -> Result<bool> {
        self.with_logged_store_mut(|store| Ok(store.remove_node_property(node_id, key)))
    }

    pub fn graph_add_node_label(&self, node_id: NodeId, label: &str) -> Result<bool> {
        self.with_logged_store_mut(|store| Ok(store.add_node_label(node_id, label)))
    }

    pub fn graph_remove_node_label(&self, node_id: NodeId, label: &str) -> Result<bool> {
        self.with_logged_store_mut(|store| Ok(store.remove_node_label(node_id, label)))
    }

    pub fn graph_set_node_labels(&self, node_id: NodeId, labels: Vec<String>) -> Result<bool> {
        self.with_logged_store_mut(|store| Ok(store.set_node_labels(node_id, labels)))
    }

    pub fn graph_replace_node_properties(
        &self,
        node_id: NodeId,
        properties: BTreeMap<String, LoraValue>,
    ) -> Result<bool> {
        let properties = values_to_properties(properties)?;
        self.with_logged_store_mut(|store| Ok(store.replace_node_properties(node_id, properties)))
    }

    pub fn graph_merge_node_properties(
        &self,
        node_id: NodeId,
        properties: BTreeMap<String, LoraValue>,
    ) -> Result<bool> {
        let properties = values_to_properties(properties)?;
        self.with_logged_store_mut(|store| Ok(store.merge_node_properties(node_id, properties)))
    }

    pub fn graph_set_relationship_property(
        &self,
        rel_id: RelationshipId,
        key: String,
        value: LoraValue,
    ) -> Result<bool> {
        let value = lora_value_to_property(value).map_err(|e| anyhow!(e))?;
        self.with_logged_store_mut(|store| Ok(store.set_relationship_property(rel_id, key, value)))
    }

    pub fn graph_remove_relationship_property(
        &self,
        rel_id: RelationshipId,
        key: &str,
    ) -> Result<bool> {
        self.with_logged_store_mut(|store| Ok(store.remove_relationship_property(rel_id, key)))
    }

    pub fn graph_replace_relationship_properties(
        &self,
        rel_id: RelationshipId,
        properties: BTreeMap<String, LoraValue>,
    ) -> Result<bool> {
        let properties = values_to_properties(properties)?;
        self.with_logged_store_mut(|store| {
            Ok(store.replace_relationship_properties(rel_id, properties))
        })
    }

    pub fn graph_merge_relationship_properties(
        &self,
        rel_id: RelationshipId,
        properties: BTreeMap<String, LoraValue>,
    ) -> Result<bool> {
        let properties = values_to_properties(properties)?;
        self.with_logged_store_mut(|store| {
            Ok(store.merge_relationship_properties(rel_id, properties))
        })
    }

    pub fn graph_delete_relationship(&self, rel_id: RelationshipId) -> Result<bool> {
        self.with_logged_store_mut(|store| Ok(store.delete_relationship(rel_id)))
    }

    pub fn graph_delete_node(&self, node_id: NodeId) -> Result<bool> {
        self.with_logged_store_mut(|store| Ok(store.delete_node(node_id)))
    }

    pub fn graph_detach_delete_node(&self, node_id: NodeId) -> Result<bool> {
        self.with_logged_store_mut(|store| Ok(store.detach_delete_node(node_id)))
    }
}

// ---------------------------------------------------------------------------
// Snapshot helpers
//
// A second impl block so the `Snapshotable` bound only constrains backends
// that actually need it. `Database<InMemoryGraph>` picks these up
// automatically; hypothetical backends that don't implement `Snapshotable`
// still get the core query API above.
// ---------------------------------------------------------------------------

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Snapshotable + Any,
{
    /// Serialize the current graph state to the given path. Writes are
    /// atomic: the payload goes to `<path>.tmp`, is `fsync`'d, and then
    /// renamed over the target; a torn write can never leave a half-written
    /// file at `path`. If any step before the rename fails, the stale
    /// `<path>.tmp` is removed so a crashed save never leaks scratch files.
    ///
    /// Holds a store read lock for the duration of the save so concurrent
    /// readers can proceed and writers wait behind a consistent snapshot.
    pub fn save_snapshot_to(&self, path: impl AsRef<Path>) -> Result<SnapshotMeta> {
        let path = path.as_ref();
        let tmp = snapshot_tmp_path(path);

        // Acquire the lock once so the snapshot is point-in-time consistent.
        let guard = self.read_store();

        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        // Arm cleanup immediately after `open` succeeds: every early return
        // below must either surface an error *and* unlink the tmp, or commit
        // the guard once the rename takes effect.
        let tmp_guard = TempFileGuard::new(tmp.clone());
        let mut writer = BufWriter::new(file);

        let meta = guard.save_snapshot(&mut writer)?;

        // Flush the BufWriter before fsync; otherwise we fsync an empty
        // underlying file.
        use std::io::Write;
        writer.flush()?;
        let file = writer.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);

        std::fs::rename(&tmp, path)?;
        // The tmp path no longer has a file behind it — disarm the guard so
        // it doesn't try to remove the just-renamed target by name race.
        tmp_guard.commit();

        // Best-effort parent-dir fsync so the rename itself is durable on
        // power loss. Non-fatal if the parent can't be opened.
        if let Some(parent) = path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }

        Ok(meta)
    }

    /// Replace the current graph state with a snapshot loaded from `path`.
    /// Holds the store write lock for the duration of the load; concurrent
    /// queries block until restore completes.
    pub fn load_snapshot_from(&self, path: impl AsRef<Path>) -> Result<SnapshotMeta> {
        let file = File::open(path.as_ref())?;
        let reader = BufReader::new(file);

        let mut guard = self.write_store();
        Ok(guard.load_snapshot(reader)?)
    }
}

impl Database<InMemoryGraph> {
    /// Convenience constructor: open (or create) an empty in-memory database
    /// and immediately restore it from `path`. Errors if the file cannot be
    /// opened or the snapshot is malformed.
    pub fn in_memory_from_snapshot(path: impl AsRef<Path>) -> Result<Self> {
        let db = Self::in_memory();
        db.load_snapshot_from_with_credentials(path, None)?;
        Ok(db)
    }

    /// Serialize the current graph state into the database snapshot byte
    /// format.
    ///
    /// This uses the same column-oriented `lora-snapshot` codec as managed
    /// snapshots, but without a WAL fence. The default is uncompressed so
    /// bytes stay portable across native and WASM builds; callers that want a
    /// specific codec can use [`Self::save_snapshot_to_bytes_with_options`].
    pub fn save_snapshot_to_bytes(&self) -> Result<Vec<u8>> {
        let options = SnapshotOptions {
            compression: Compression::None,
            encryption: None,
        };
        let (bytes, _) = self.save_snapshot_to_bytes_with_options(&options)?;
        Ok(bytes)
    }

    /// Serialize the current graph state into database snapshot bytes with
    /// explicit codec options.
    pub fn save_snapshot_to_bytes_with_options(
        &self,
        options: &SnapshotOptions,
    ) -> Result<(Vec<u8>, SnapshotInfo)> {
        let guard = self.read_store();
        let payload = guard.snapshot_payload();
        let mut bytes = Vec::new();
        let info = write_database_snapshot(&mut bytes, &payload, None, options)
            .map_err(|e| anyhow!("encode database snapshot failed: {e}"))?;
        Ok((bytes, info))
    }

    /// Serialize the current graph state to a database snapshot file with
    /// explicit codec options. This is the path form of
    /// [`Self::save_snapshot_to_bytes_with_options`] and supports the same
    /// compression and encryption options.
    pub fn save_snapshot_to_with_options(
        &self,
        path: impl AsRef<Path>,
        options: &SnapshotOptions,
    ) -> Result<SnapshotMeta> {
        let path = path.as_ref();
        let tmp = snapshot_tmp_path(path);
        let guard = self.read_store();

        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        let tmp_guard = TempFileGuard::new(tmp.clone());
        let mut writer = BufWriter::new(file);

        let payload = guard.snapshot_payload();
        let info = write_database_snapshot(&mut writer, &payload, None, options)
            .map_err(|e| anyhow!("encode database snapshot failed: {e}"))?;

        use std::io::Write;
        writer.flush()?;
        let file = writer.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);

        std::fs::rename(&tmp, path)?;
        tmp_guard.commit();

        if let Some(parent) = path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }

        Ok(snapshot_info_to_meta(info))
    }

    /// Replace the current graph state from snapshot bytes.
    ///
    /// The database snapshot format is decoded first. Legacy `lora-store`
    /// snapshot bytes are still accepted so existing bindings can load older
    /// browser / native byte snapshots after this API moves to the columnar
    /// database codec.
    pub fn load_snapshot_from_bytes(&self, bytes: &[u8]) -> Result<SnapshotMeta> {
        self.load_snapshot_from_bytes_with_credentials(bytes, None)
    }

    /// Replace the current graph state from snapshot bytes, supplying
    /// credentials when loading an encrypted database snapshot.
    pub fn load_snapshot_from_bytes_with_credentials(
        &self,
        bytes: &[u8],
        credentials: Option<&SnapshotCredentials>,
    ) -> Result<SnapshotMeta> {
        let mut guard = self.write_store();
        match SnapshotByteFormat::detect(bytes) {
            Some(SnapshotByteFormat::Database) => {
                let (payload, info) = decode_database_snapshot(bytes, credentials)
                    .map_err(|e| anyhow!("decode database snapshot failed: {e}"))?;
                let meta = SnapshotMeta {
                    format_version: info.format_version,
                    node_count: info.node_count,
                    relationship_count: info.relationship_count,
                    wal_lsn: info.wal_lsn,
                };
                guard.load_snapshot_payload(payload)?;
                Ok(meta)
            }
            Some(SnapshotByteFormat::LegacyStore) => Ok(guard.load_snapshot(bytes)?),
            None => Err(anyhow!("snapshot bytes have unrecognized magic")),
        }
    }

    /// Replace the current graph state from a database snapshot file,
    /// supplying credentials when the snapshot is encrypted. Legacy
    /// `lora-store` snapshots are accepted when no database credentials are
    /// needed.
    pub fn load_snapshot_from_with_credentials(
        &self,
        path: impl AsRef<Path>,
        credentials: Option<&SnapshotCredentials>,
    ) -> Result<SnapshotMeta> {
        let bytes = std::fs::read(path.as_ref())?;
        self.load_snapshot_from_bytes_with_credentials(&bytes, credentials)
    }

    /// Take a checkpoint: snapshot the current state with the WAL's
    /// `durable_lsn` stamped into the header, append a `Checkpoint`
    /// marker to the WAL, then drop sealed segments at or below the
    /// fence.
    ///
    /// Errors with "checkpoint requires WAL enabled" when called on a
    /// database constructed without a WAL — operators that just want
    /// a fence-less dump should use [`save_snapshot_to`] instead.
    ///
    /// The write-lock-held window covers snapshot serialization plus the
    /// checkpoint marker append. Truncation runs after the rename
    /// but still under the write lock; making it concurrent with queries
    /// is a v2 concern (see `docs/decisions/0004-wal.md`).
    pub fn checkpoint_to(&self, path: impl AsRef<Path>) -> Result<SnapshotMeta> {
        let recorder = self
            .wal
            .as_ref()
            .ok_or_else(|| anyhow!("checkpoint requires WAL enabled"))?;
        let path = path.as_ref();
        let tmp = snapshot_tmp_path(path);

        let guard = self.write_store();

        // Make every record appended so far durable, then capture
        // the LSN that becomes the snapshot fence.
        recorder
            .force_fsync()
            .map_err(|e| anyhow!("WAL fsync before checkpoint failed: {e}"))?;
        let snapshot_lsn = recorder.wal().durable_lsn();

        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        let tmp_guard = TempFileGuard::new(tmp.clone());
        let mut writer = BufWriter::new(file);
        let meta = guard.save_checkpoint(&mut writer, snapshot_lsn.raw())?;

        use std::io::Write;
        writer.flush()?;
        let file = writer.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);

        std::fs::rename(&tmp, path)?;
        tmp_guard.commit();

        if let Some(parent) = path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }

        // Append the checkpoint marker AFTER the rename succeeds —
        // this preserves the invariant that a `Checkpoint` record
        // in the WAL implies the snapshot it points at exists.
        recorder
            .checkpoint_marker(snapshot_lsn)
            .map_err(|e| anyhow!("WAL checkpoint marker failed: {e}"))?;
        recorder
            .force_fsync()
            .map_err(|e| anyhow!("WAL fsync after checkpoint marker failed: {e}"))?;

        // Best-effort segment truncation. Failure here doesn't undo
        // the checkpoint — the next call will retry.
        let _ = recorder.truncate_up_to(snapshot_lsn);

        Ok(meta)
    }

    /// Take a checkpoint into the managed snapshot directory configured by
    /// [`Self::open_with_wal_snapshots`].
    pub fn checkpoint_managed(&self) -> Result<SnapshotMeta> {
        let recorder = self
            .wal
            .as_ref()
            .ok_or_else(|| anyhow!("managed checkpoint requires WAL enabled"))?;
        let snapshots = self
            .snapshots
            .as_ref()
            .ok_or_else(|| anyhow!("managed checkpoint requires snapshots enabled"))?;
        let guard = self.write_store();
        snapshots.checkpoint(&guard, recorder)
    }
}

fn snapshot_tmp_path(target: &Path) -> PathBuf {
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

/// RAII handle that deletes its path on drop unless [`commit`] is called.
///
/// The snapshot save path creates `<target>.tmp` before the payload is
/// written; if any step between then and the final rename fails (or the
/// thread unwinds), the guard's `Drop` removes the scratch file so a crashed
/// save never leaves leftovers on disk.
///
/// [`commit`]: Self::commit
struct TempFileGuard {
    path: Option<PathBuf>,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// Disarm the guard. Call this once the tmp file's contents have been
    /// handed off (e.g. renamed to their final destination) so the `Drop`
    /// impl does not try to remove them.
    fn commit(mut self) {
        self.path.take();
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            // Best-effort: cleanup failure is not worth surfacing — the
            // worst case is a leaked scratch file that the next save
            // overwrites via `OpenOptions::truncate(true)`.
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Storage-agnostic admin surface for HTTP / binding callers that want to
/// drive snapshot operations without naming the backend type parameter.
///
/// `Database<S>` picks up a blanket impl when `S: Snapshotable + 'static`.
/// Transports (e.g. `lora-server`) type-erase on `Arc<dyn SnapshotAdmin>`.
pub trait SnapshotAdmin: Send + Sync + 'static {
    fn save_snapshot(&self, path: &Path) -> Result<SnapshotMeta>;
    fn load_snapshot(&self, path: &Path) -> Result<SnapshotMeta>;
}

impl<S> SnapshotAdmin for Database<S>
where
    S: GraphStorage + GraphStorageMut + Snapshotable + Any + Send + Sync + 'static,
{
    fn save_snapshot(&self, path: &Path) -> Result<SnapshotMeta> {
        self.save_snapshot_to(path)
    }

    fn load_snapshot(&self, path: &Path) -> Result<SnapshotMeta> {
        self.load_snapshot_from(path)
    }
}

/// Storage-agnostic admin surface for the WAL.
///
/// `Database<InMemoryGraph>` picks up the blanket impl below when a
/// WAL is attached. Transports (e.g. `lora-server`) type-erase on
/// `Arc<dyn WalAdmin>` so they don't need to name the backend type
/// parameter.
///
/// All LSNs cross the trait boundary as raw `u64` so callers don't
/// need a dependency on `lora-wal`.
pub trait WalAdmin: Send + Sync + 'static {
    /// Take a checkpoint at `path`. The snapshot's header is stamped
    /// with the WAL's `durable_lsn`; older sealed segments are then
    /// dropped.
    fn checkpoint(&self, path: &Path) -> Result<SnapshotMeta>;

    /// Snapshot of the WAL's current state — durable / next LSN,
    /// active / oldest segment id. Cheap; a single WAL mutex acquisition.
    fn wal_status(&self) -> Result<WalStatus>;

    /// Drop sealed segments at or below `fence_lsn`. Idempotent.
    fn wal_truncate(&self, fence_lsn: u64) -> Result<()>;
}

/// Snapshot of WAL state returned by [`WalAdmin::wal_status`].
///
/// `bg_failure` is the latched fsync error from the background flusher
/// (only meaningful under `SyncMode::Group`). When `Some`, the WAL is
/// poisoned and every subsequent commit will fail loudly until the
/// operator restarts from the last consistent snapshot + WAL.
#[derive(Debug, Clone)]
pub struct WalStatus {
    pub durable_lsn: u64,
    pub next_lsn: u64,
    pub active_segment_id: u64,
    pub oldest_segment_id: u64,
    pub bg_failure: Option<String>,
}

impl WalAdmin for Database<InMemoryGraph> {
    fn checkpoint(&self, path: &Path) -> Result<SnapshotMeta> {
        self.checkpoint_to(path)
    }

    fn wal_status(&self) -> Result<WalStatus> {
        let recorder = self
            .wal
            .as_ref()
            .ok_or_else(|| anyhow!("WAL not enabled"))?;
        let wal = recorder.wal();
        Ok(WalStatus {
            durable_lsn: wal.durable_lsn().raw(),
            next_lsn: wal.next_lsn().raw(),
            active_segment_id: wal.active_segment_id(),
            oldest_segment_id: wal.oldest_segment_id(),
            bg_failure: wal.bg_failure(),
        })
    }

    fn wal_truncate(&self, fence_lsn: u64) -> Result<()> {
        let recorder = self
            .wal
            .as_ref()
            .ok_or_else(|| anyhow!("WAL not enabled"))?;
        recorder.truncate_up_to(Lsn::new(fence_lsn))?;
        Ok(())
    }
}

impl<S> QueryRunner for Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Send + Sync + 'static,
{
    fn execute(&self, query: &str, options: Option<ExecuteOptions>) -> Result<QueryResult> {
        Database::execute(self, query, options)
    }
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

/// Apply a `MutationEvent` stream to an in-memory graph by dispatching
/// each variant to the matching store operation.
///
/// Creation events are replayed through id-preserving paths, not the
/// normal allocator-backed mutation methods. That matters after aborted
/// transactions: an aborted create can consume id `N` in the original
/// process, be dropped by replay, and leave the next committed create at
/// id `N + 1`. Reusing the regular allocator would shift ids downward.
///
/// Replay must be invoked **before** the `WalRecorder` is installed
/// on the graph. Otherwise the replay's own mutations would fire the
/// recorder and re-write the same events to the WAL, doubling them on
/// the next recovery.
fn replay_into(graph: &mut InMemoryGraph, events: Vec<MutationEvent>) -> Result<()> {
    for (idx, event) in events.into_iter().enumerate() {
        match event {
            MutationEvent::CreateNode {
                id,
                labels,
                properties,
            } => {
                graph
                    .replay_create_node(id, labels, properties)
                    .map_err(|e| anyhow!("WAL replay failed at event {idx}: {e}"))?;
            }
            MutationEvent::CreateRelationship {
                id,
                src,
                dst,
                rel_type,
                properties,
            } => {
                graph
                    .replay_create_relationship(id, src, dst, &rel_type, properties)
                    .map_err(|e| anyhow!("WAL replay failed at event {idx}: {e}"))?;
            }
            MutationEvent::SetNodeProperty {
                node_id,
                key,
                value,
            } => {
                if !graph.set_node_property(node_id, key, value) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing node {node_id} for property set"
                    ));
                }
            }
            MutationEvent::RemoveNodeProperty { node_id, key } => {
                if !graph.remove_node_property(node_id, &key) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing node {node_id} for property removal"
                    ));
                }
            }
            MutationEvent::AddNodeLabel { node_id, label } => {
                if !graph.add_node_label(node_id, &label) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing node {node_id} for label add"
                    ));
                }
            }
            MutationEvent::RemoveNodeLabel { node_id, label } => {
                if !graph.remove_node_label(node_id, &label) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing node {node_id} for label removal"
                    ));
                }
            }
            MutationEvent::SetRelationshipProperty { rel_id, key, value } => {
                if !graph.set_relationship_property(rel_id, key, value) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing relationship {rel_id} for property set"
                    ));
                }
            }
            MutationEvent::RemoveRelationshipProperty { rel_id, key } => {
                if !graph.remove_relationship_property(rel_id, &key) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing relationship {rel_id} for property removal"
                    ));
                }
            }
            MutationEvent::DeleteRelationship { rel_id } => {
                if !graph.delete_relationship(rel_id) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing relationship {rel_id} for delete"
                    ));
                }
            }
            MutationEvent::DeleteNode { node_id } => {
                if !graph.delete_node(node_id) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing or attached node {node_id} for delete"
                    ));
                }
            }
            MutationEvent::DetachDeleteNode { node_id } => {
                // After the cascading DeleteRelationship +
                // DeleteNode events have already replayed, the node
                // is gone and this becomes a no-op. Calling it
                // anyway is harmless.
                graph.detach_delete_node(node_id);
            }
            MutationEvent::Clear => {
                graph.clear();
            }
        }
    }
    Ok(())
}
