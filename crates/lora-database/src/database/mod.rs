use std::any::Any;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use lora_ast::{Direction, Document};
use lora_executor::{lora_value_to_property, ExecuteOptions, LoraValue, QueryResult};
use lora_parser::parse_query;
use lora_store::{GraphStorage, GraphStorageMut, InMemoryGraph, Properties};
use lora_wal::WalRecorder;

mod builder;
mod compile;
mod execute;
mod explain;
mod graph_api;
mod occ;
mod procedures;
mod profile;
mod pull_mode;
mod replay;
mod row_projection;
mod schema;
mod show_pipeline;
mod stream;
mod write_guard;

use crate::error::LoraError;
use crate::explain::{QueryPlan, QueryProfile};
use crate::live_store::LiveStore;
use crate::plan_cache::PlanCache;
use crate::snapshot::ManagedSnapshotStore;
use crate::wal::archive::WalArchive;

/// Minimal abstraction any transport can depend on to run Lora queries.
///
/// `execute` runs a query and returns rows. `explain` and `profile` are
/// deliberately separate methods: `explain` never invokes the executor
/// (so it can be called on mutating queries without side effects) and
/// `profile` runs the executor and reports runtime metrics. Transports
/// MUST NOT route plan / profile requests through `execute` — exposing
/// the plan-only and profile-with-metrics behaviours as separate
/// methods is part of the public contract.
pub trait QueryRunner: Send + Sync + 'static {
    fn execute(
        &self,
        query: &str,
        options: Option<ExecuteOptions>,
    ) -> Result<QueryResult, LoraError>;

    /// Compile a query and return its plan without executing it.
    fn explain(
        &self,
        query: &str,
        params: Option<BTreeMap<String, LoraValue>>,
    ) -> Result<QueryPlan, LoraError>;

    /// Execute a query and return its plan plus runtime metrics.
    /// Mutating queries are persisted exactly as in `execute`.
    fn profile(
        &self,
        query: &str,
        params: Option<BTreeMap<String, LoraValue>>,
    ) -> Result<QueryProfile, LoraError>;
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
    /// The current authoritative store. Reads call `store.load_full()`
    /// to obtain an independent `Arc<S>` snapshot; writers take the
    /// `writer` Mutex and then a brief write-lock on the inner
    /// `RwLock<Arc<S>>`, mutating in-place via `Arc::make_mut`. When
    /// no in-flight reader holds a snapshot Arc, `make_mut` returns
    /// `&mut S` without cloning the graph — that's the single-writer
    /// fast path that "CREATE one node" / "SET property" depend on
    /// for graph-size-independent throughput. When concurrent readers
    /// are alive, `make_mut` clones once and the readers keep
    /// observing the pre-mutation state via their old Arc.
    pub(crate) store: Arc<LiveStore<S>>,
    /// Serializes commit ordering. Held across `mutate-WAL-publish` so
    /// WAL records are appended in the same order live state advances.
    /// Readers never touch this Mutex; only writers contend.
    pub(crate) writer: Arc<Mutex<()>>,
    /// Per-record write locks. Plumbed for a future phase that allows
    /// concurrent commits across disjoint write sets; today the writer
    /// Mutex provides single-writer-at-a-time semantics so this table
    /// is idle.
    #[allow(dead_code)]
    pub(crate) lock_table: Arc<lora_store::LockTable>,
    pub(crate) wal: Option<Arc<WalRecorder>>,
    pub(crate) snapshots: Option<Arc<ManagedSnapshotStore>>,
    /// Present for named `.loradb` databases. Runtime state is still the
    /// in-memory graph; this handle lets `sync()` refresh the portable
    /// checkpointed container with a base snapshot frame plus WAL delta frames.
    pub(crate) named_archive: Option<Arc<WalArchive>>,
    /// Cache of compiled query plans, content-keyed by raw query text. Shared
    /// across the read- and write-lock phases of a single execute (so a
    /// mutating query compiles at most once instead of twice) and across
    /// every subsequent call that uses the same query string.
    pub(crate) plan_cache: Arc<PlanCache>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphDirection {
    Outgoing,
    Incoming,
    Both,
}

impl GraphDirection {
    pub(crate) fn as_store_direction(self) -> Direction {
        match self {
            Self::Outgoing => Direction::Right,
            Self::Incoming => Direction::Left,
            Self::Both => Direction::Undirected,
        }
    }
}

pub(crate) fn values_to_properties(values: BTreeMap<String, LoraValue>) -> Result<Properties> {
    values
        .into_iter()
        .map(|(key, value)| {
            let value = lora_value_to_property(value).map_err(|e| anyhow!(e))?;
            Ok((key, value))
        })
        .collect()
}

pub(crate) const QUERY_FAILURE_POISON: &str =
    "query mutated the live graph before failing; restart from snapshot + WAL required";

impl Database<InMemoryGraph> {
    /// Force any pending WAL bytes to durable storage and, for container-backed
    /// named databases, refresh the portable `.loradb` file before returning.
    ///
    /// Managed snapshot checkpoints are explicit via
    /// [`Self::checkpoint_managed`] or threshold-driven via
    /// [`SnapshotConfig::checkpoint_every_commits`]; `sync()` remains a
    /// durability operation rather than an O(graph) checkpoint.
    pub fn sync(&self) -> Result<(), LoraError> {
        if let Some(wal) = &self.wal {
            if let Some(archive) = &self.named_archive {
                let _commit_lock = self
                    .writer
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                wal.force_fsync_wal_only()?;
                let snapshot_lsn = wal.wal().durable_lsn();
                let graph = self.store.load_full();
                let payload = graph.snapshot_payload();
                let mut bytes = Vec::new();
                let options = lora_snapshot::SnapshotOptions::default();
                crate::snapshot::encode_snapshot_to(
                    &mut bytes,
                    &payload,
                    Some(snapshot_lsn.raw()),
                    &options,
                )?;
                archive.persist_snapshot_bytes(bytes)?;
            } else {
                wal.force_fsync()?;
            }
        }
        Ok(())
    }
}

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    /// Handle to the installed WAL recorder, if any. Exposed for
    /// admin paths (checkpoint, truncate, observability) that need
    /// to drive the WAL outside the standard query lifecycle.
    pub fn wal(&self) -> Option<&Arc<WalRecorder>> {
        self.wal.as_ref()
    }

    /// Snapshot the current authoritative graph. Equivalent to the
    /// historical `database.store().load_full()` pattern, exposed here
    /// so external callers don't need to name the internal storage
    /// wrapper.
    pub fn snapshot(&self) -> Arc<S> {
        self.store.load_full()
    }

    /// Parse a query string into an AST without executing it.
    pub fn parse(&self, query: &str) -> Result<Document, LoraError> {
        Ok(parse_query(query)?)
    }

    /// Read the current authoritative snapshot. Lock-free: returns an
    /// `Arc<S>` whose lifetime is independent of any writer.
    pub(crate) fn read_store(&self) -> Arc<S> {
        self.store.load_full()
    }
}

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    // ---------- Storage-agnostic utility helpers ----------
    //
    // Bindings previously reached into the shared store lock to answer
    // stat / admin calls; these helpers let them depend on `Database<S>`
    // instead, so swapping in a new backend only requires changing one type
    // parameter.

    /// Drop every node and relationship, returning WAL/archive errors to the
    /// caller.
    ///
    /// When a WAL is attached, the buffered `MutationEvent::Clear` is appended
    /// to the log on success. The clear runs in place against the live
    /// graph via the same fast path as `with_logged_store_mut`, so a
    /// large graph clear no longer pays an O(N+E) snapshot clone.
    pub fn try_clear(&self) -> Result<(), LoraError> {
        self.with_logged_store_mut(|store| {
            store.clear();
            Ok(())
        })
        .map_err(LoraError::from_anyhow)
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
        let snapshot = self.read_store();
        snapshot.node_count()
    }

    /// Number of relationships currently in the graph.
    pub fn relationship_count(&self) -> usize {
        let snapshot = self.read_store();
        snapshot.relationship_count()
    }

    /// Run a closure with a shared borrow of the underlying store.
    /// Lock-free: callers see a consistent snapshot for the duration of
    /// the closure even while writers commit new versions.
    pub fn with_store<R>(&self, f: impl FnOnce(&S) -> R) -> R {
        let snapshot = self.read_store();
        f(&*snapshot)
    }

    /// Run a closure with an exclusive borrow of the underlying store. Reserved
    /// for admin paths (restore, bulk load); regular mutation goes through
    /// `execute_with_params`. The closure mutates the live graph in place
    /// via `Arc::make_mut`, so callers don't pay an O(N+E) snapshot clone
    /// just to overwrite the graph. Concurrent readers, when present,
    /// force a single CoW clone and keep observing their pre-mutation
    /// snapshot via the `Arc<S>` they already hold.
    pub fn with_store_mut<R>(&self, f: impl FnOnce(&mut S) -> R) -> R {
        let _lock = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut handle = self.store.write();
        f(handle.as_mut())
    }
}

impl<S> QueryRunner for Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    fn execute(
        &self,
        query: &str,
        options: Option<ExecuteOptions>,
    ) -> Result<QueryResult, LoraError> {
        Database::execute(self, query, options)
    }

    fn explain(
        &self,
        query: &str,
        params: Option<BTreeMap<String, LoraValue>>,
    ) -> Result<QueryPlan, LoraError> {
        Database::explain(self, query, params)
    }

    fn profile(
        &self,
        query: &str,
        params: Option<BTreeMap<String, LoraValue>>,
    ) -> Result<QueryProfile, LoraError> {
        Database::profile(self, query, params)
    }
}
