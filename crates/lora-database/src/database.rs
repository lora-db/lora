use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::Result;
use lora_analyzer::Analyzer;
use lora_ast::Document;
use lora_compiler::{CompiledQuery, Compiler};
use lora_executor::{
    ExecuteOptions, LoraValue, MutableExecutionContext, MutableExecutor, QueryResult,
};
use lora_parser::parse_query;
use lora_store::{GraphStorage, GraphStorageMut, InMemoryGraph, SnapshotMeta, Snapshotable};

/// Minimal abstraction any transport can depend on to run Lora queries.
pub trait QueryRunner: Send + Sync + 'static {
    fn execute(&self, query: &str, options: Option<ExecuteOptions>) -> Result<QueryResult>;
}

/// Owns the graph store and orchestrates parse → analyze → compile → execute.
pub struct Database<S> {
    store: Arc<Mutex<S>>,
}

impl Database<InMemoryGraph> {
    /// Convenience constructor: a fresh, empty in-memory graph database.
    pub fn in_memory() -> Self {
        Self::from_graph(InMemoryGraph::new())
    }
}

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut,
{
    /// Build a database from a pre-wrapped, shared store.
    pub fn new(store: Arc<Mutex<S>>) -> Self {
        Self { store }
    }

    /// Build a database by taking ownership of a bare graph store.
    pub fn from_graph(graph: S) -> Self {
        Self::new(Arc::new(Mutex::new(graph)))
    }

    /// Handle to the underlying shared store — useful for callers that need
    /// to snapshot or share the graph across multiple databases.
    pub fn store(&self) -> &Arc<Mutex<S>> {
        &self.store
    }

    /// Parse a query string into an AST without executing it.
    pub fn parse(&self, query: &str) -> Result<Document> {
        Ok(parse_query(query)?)
    }

    fn lock_store(&self) -> MutexGuard<'_, S> {
        self.store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn compile_query(&self, query: &str) -> Result<(MutexGuard<'_, S>, CompiledQuery)> {
        let document = self.parse(query)?;
        let store = self.lock_store();

        let resolved = {
            let mut analyzer = Analyzer::new(&*store);
            analyzer.analyze(&document)?
        };

        let compiled = Compiler::compile(&resolved);
        Ok((store, compiled))
    }

    /// Execute a query and return its result.
    pub fn execute(&self, query: &str, options: Option<ExecuteOptions>) -> Result<QueryResult> {
        self.execute_with_params(query, options, BTreeMap::new())
    }

    /// Execute a query with bound parameters.
    pub fn execute_with_params(
        &self,
        query: &str,
        options: Option<ExecuteOptions>,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<QueryResult> {
        let (mut store, compiled) = self.compile_query(query)?;

        let mut executor = MutableExecutor::new(MutableExecutionContext {
            storage: &mut *store,
            params,
        });

        Ok(executor.execute_compiled(&compiled, options)?)
    }

    // ---------- Storage-agnostic utility helpers ----------
    //
    // Bindings previously reached into `Arc<Mutex<InMemoryGraph>>` to answer
    // stat / admin calls; these helpers let them depend on `Database<S>`
    // instead, so swapping in a new backend only requires changing one type
    // parameter.

    /// Drop every node and relationship.
    pub fn clear(&self) {
        let mut guard = self.lock_store();
        guard.clear();
    }

    /// Number of nodes currently in the graph.
    pub fn node_count(&self) -> usize {
        let guard = self.lock_store();
        guard.node_count()
    }

    /// Number of relationships currently in the graph.
    pub fn relationship_count(&self) -> usize {
        let guard = self.lock_store();
        guard.relationship_count()
    }

    /// Run a closure with a shared borrow of the underlying store. Used by
    /// bindings to answer ad-hoc queries without locking the mutex themselves.
    pub fn with_store<R>(&self, f: impl FnOnce(&S) -> R) -> R {
        let guard = self.lock_store();
        f(&*guard)
    }

    /// Run a closure with an exclusive borrow of the underlying store. Reserved
    /// for admin paths (restore, bulk load); regular mutation goes through
    /// `execute_with_params`.
    pub fn with_store_mut<R>(&self, f: impl FnOnce(&mut S) -> R) -> R {
        let mut guard = self.lock_store();
        f(&mut *guard)
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
    S: GraphStorage + GraphStorageMut + Snapshotable,
{
    /// Serialize the current graph state to the given path. Writes are
    /// atomic: the payload goes to `<path>.tmp`, is `fsync`'d, and then
    /// renamed over the target; a torn write can never leave a half-written
    /// file at `path`. If any step before the rename fails, the stale
    /// `<path>.tmp` is removed so a crashed save never leaks scratch files.
    ///
    /// Holds the store mutex for the duration of the save so concurrent
    /// queries see a consistent point-in-time snapshot.
    pub fn save_snapshot_to(&self, path: impl AsRef<Path>) -> Result<SnapshotMeta> {
        let path = path.as_ref();
        let tmp = snapshot_tmp_path(path);

        // Acquire the lock once so the snapshot is point-in-time consistent.
        let guard = self.lock_store();

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
    /// Holds the store mutex for the duration of the load; concurrent
    /// queries block until restore completes.
    pub fn load_snapshot_from(&self, path: impl AsRef<Path>) -> Result<SnapshotMeta> {
        let file = File::open(path.as_ref())?;
        let reader = BufReader::new(file);

        let mut guard = self.lock_store();
        Ok(guard.load_snapshot(reader)?)
    }
}

impl Database<InMemoryGraph> {
    /// Convenience constructor: open (or create) an empty in-memory database
    /// and immediately restore it from `path`. Errors if the file cannot be
    /// opened or the snapshot is malformed.
    pub fn in_memory_from_snapshot(path: impl AsRef<Path>) -> Result<Self> {
        let db = Self::in_memory();
        db.load_snapshot_from(path)?;
        Ok(db)
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
    S: GraphStorage + GraphStorageMut + Snapshotable + Send + 'static,
{
    fn save_snapshot(&self, path: &Path) -> Result<SnapshotMeta> {
        self.save_snapshot_to(path)
    }

    fn load_snapshot(&self, path: &Path) -> Result<SnapshotMeta> {
        self.load_snapshot_from(path)
    }
}

impl<S> QueryRunner for Database<S>
where
    S: GraphStorage + GraphStorageMut + Send + 'static,
{
    fn execute(&self, query: &str, options: Option<ExecuteOptions>) -> Result<QueryResult> {
        Database::execute(self, query, options)
    }
}
