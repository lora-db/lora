use std::any::Any;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;

use anyhow::{anyhow, Result};
use lora_ast::{Direction, Document};
use lora_executor::{lora_value_to_property, ExecuteOptions, LoraValue, QueryResult};
use lora_parser::parse_query;
use lora_store::{GraphStorage, GraphStorageMut, InMemoryGraph, Properties};
use lora_wal::WalRecorder;

mod builder;
mod execute;
mod graph_api;
mod occ;
mod pull_mode;
mod replay;
mod stream;
mod write_guard;

use crate::error::LoraError;
use crate::plan_cache::PlanCache;
use crate::snapshot::ManagedSnapshotStore;
use crate::wal::write_scope::WalAbortPolicy;

/// Minimal abstraction any transport can depend on to run Lora queries.
pub trait QueryRunner: Send + Sync + 'static {
    fn execute(
        &self,
        query: &str,
        options: Option<ExecuteOptions>,
    ) -> Result<QueryResult, LoraError>;
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
    /// The current authoritative store, atomically swappable. Reads call
    /// `store.load_full()` to obtain an `Arc<S>` snapshot — no lock, no
    /// blocking — and run their executor against `&*snapshot`. Writes
    /// take the `writer` Mutex (for commit-order serialization), clone
    /// the current snapshot into a working copy, mutate that copy, append
    /// to the WAL, then `store.store(Arc::new(staged))` to publish.
    /// Concurrent reads keep their old `Arc<S>` alive until they drop it,
    /// which gives natural snapshot isolation.
    pub(crate) store: Arc<ArcSwap<S>>,
    /// Serializes commit ordering. Held only across `clone-mutate-WAL-publish`,
    /// not around any read. Multiple readers proceed concurrently with a
    /// writer; only writers contend with each other on this Mutex.
    pub(crate) writer: Arc<Mutex<()>>,
    /// Per-record write locks, plumbed in Phase 4.1. Phase 4.2 keeps
    /// the writer Mutex serialization model so the lock table sits
    /// idle for now; it becomes load-bearing in a future phase that
    /// drops the writer Mutex in favour of ArcSwap CAS for true
    /// concurrent commits, where two writers with overlapping write
    /// sets need a real serialization point.
    #[allow(dead_code)]
    pub(crate) lock_table: Arc<lora_store::LockTable>,
    pub(crate) wal: Option<Arc<WalRecorder>>,
    pub(crate) snapshots: Option<Arc<ManagedSnapshotStore>>,
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
    /// Force any pending WAL bytes to durable storage and, for archive-backed
    /// databases, refresh the portable `.loradb` file before returning.
    ///
    /// Managed snapshot checkpoints are explicit via
    /// [`Self::checkpoint_managed`] or threshold-driven via
    /// [`SnapshotConfig::checkpoint_every_commits`]; `sync()` remains a
    /// durability operation rather than an O(graph) checkpoint.
    pub fn sync(&self) -> Result<(), LoraError> {
        if let Some(wal) = &self.wal {
            wal.force_fsync()?;
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

    /// Handle to the underlying shared store — useful for callers that need
    /// to snapshot or share the graph across multiple databases.
    pub fn store(&self) -> &Arc<ArcSwap<S>> {
        &self.store
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
    /// When a WAL is attached, the clear is wrapped in `arm`/`commit` so the
    /// `MutationEvent::Clear` fired by the store reaches the log inside a
    /// transaction. If a failure happens after the in-memory graph has been
    /// cleared, the recorder is poisoned by the failing WAL path and future
    /// writes fail until the database is reopened from durable state.
    pub fn try_clear(&self) -> Result<(), LoraError> {
        let guard = self.write_store();
        self.with_logged_write_guard(guard, WalAbortPolicy::AbortOnly, |store| {
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
    /// `execute_with_params`. The closure mutates a staged copy that is
    /// published atomically when the closure returns.
    pub fn with_store_mut<R>(&self, f: impl FnOnce(&mut S) -> R) -> R {
        let mut guard = self.write_store();
        let result = f(&mut *guard);
        guard.publish();
        result
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
}
