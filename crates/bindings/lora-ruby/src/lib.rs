#![deny(clippy::all)]

//! Magnus + rb-sys bindings for the Lora graph database.
//!
//! The Rust engine is synchronous. We expose a single `LoraRuby::Database`
//! class and release Ruby's GVL for the duration of each query via
//! `rb_thread_call_without_gvl` so other Ruby threads can progress while
//! the engine runs. Concurrent calls against the same `Database`
//! serialise on an internal mutex but do not hold the GVL.
//!
//! Value conversion follows the shared `LoraValue` contract used by
//! `lora-node`, `lora-wasm`, and `lora-python`: primitives pass through
//! as Ruby natives; graph, temporal, and spatial values are returned as
//! tagged `Hash`es (string keys) with a `"kind"` discriminator.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use magnus::{
    function, method, prelude::*, value::ReprValue, Error as MagnusError, RHash, RString, Ruby,
    Value,
};

use lora_database::{
    CancellableDeadline, Database as InnerDatabase, DatabaseOpenOptions, ExecuteOptions,
    InMemoryGraph, LoraError, LoraValue, QueryResult, ResultFormat, SnapshotConfig,
    SnapshotOptions, WalConfig,
};

mod errors;
mod from_ruby;
mod gvl;
mod to_ruby;

use errors::{invalid_params, query_error, query_error_from_anyhow};
use from_ruby::{hash_get_any, read_nonnegative_u64, ruby_optional_to_json, ruby_value_to_params};
use gvl::{without_gvl, without_gvl_cancellable};
use to_ruby::{lora_value_to_ruby, query_plan_to_ruby, query_profile_to_ruby};

// ============================================================================
// Module / exception registration
// ============================================================================

/// rb-sys init hook.
///
/// `extconf.rb` (at the gem/crate root) calls
/// `create_rust_makefile("lora_ruby/lora_ruby")`, which names the
/// resulting shared object `lora_ruby.{so,bundle,dll}`. Ruby then
/// calls `Init_lora_ruby` when the extension is loaded;
/// `magnus::init` wraps that C-ABI entry point for us.
#[magnus::init]
fn init(ruby: &Ruby) -> Result<(), MagnusError> {
    let lora_ruby = ruby.define_module("LoraRuby")?;

    // Error hierarchy — mirrors the Python binding's LoraError /
    // LoraQueryError / InvalidParamsError tree, but follows Ruby naming
    // (`Error` as the base class, subclasses for each concrete case).
    // `Module::define_class` wants an `RClass`; `ExceptionClass::as_r_class`
    // strips the exception-typed wrapper while keeping the underlying
    // class intact. The subclasses are later retrieved as
    // `ExceptionClass` via `const_get`, which is sound because they
    // still descend from `Exception` on the Ruby side.
    let standard_error = ruby.exception_standard_error().as_r_class();
    let error = lora_ruby.define_class("Error", standard_error)?;
    lora_ruby.define_class("QueryError", error)?;
    lora_ruby.define_class("InvalidParamsError", error)?;

    let database = lora_ruby.define_class("Database", ruby.class_object())?;
    database.define_singleton_method("create", function!(database_create, -1))?;
    database.define_singleton_method("new", function!(database_new, -1))?;
    database.define_singleton_method("open_wal", function!(database_open_wal, -1))?;
    database.define_method("execute", method!(database_execute, -1))?;
    database.define_method("explain", method!(database_explain, -1))?;
    database.define_method("profile", method!(database_profile, -1))?;
    database.define_method("clear", method!(database_clear, 0))?;
    database.define_method("close", method!(database_close, 0))?;
    database.define_method("node_count", method!(database_node_count, 0))?;
    database.define_method(
        "relationship_count",
        method!(database_relationship_count, 0),
    )?;
    database.define_method("inspect", method!(database_inspect, 0))?;
    database.define_method("to_s", method!(database_inspect, 0))?;
    database.define_method("save_snapshot", method!(database_save_snapshot, -1))?;
    database.define_method("load_snapshot", method!(database_load_snapshot, -1))?;

    // Wait for databases the GC handed to background droppers before the
    // process exits, so their WALs are flushed and closed (see `Drop`).
    unsafe { rb_sys::rb_set_end_proc(Some(finish_background_drops), 0) };

    // `LoraRuby::VERSION` is owned by `lib/lora_ruby/version.rb` so the
    // gem can expose a version before the native extension compiles
    // (during `gem build` / `bundle install`). Defining it here too
    // would trigger a "already initialized constant" warning on load.

    Ok(())
}

// ============================================================================
// Database
// ============================================================================

/// Lora graph database handle exposed to Ruby.
///
/// Wraps an `Arc<Database<InMemoryGraph>>`; the same handle is cloned
/// across the GVL-release boundary for query execution without borrowing
/// any Ruby state.
#[magnus::wrap(class = "LoraRuby::Database", free_immediately, size)]
struct Database {
    db: Mutex<Option<Arc<InnerDatabase<InMemoryGraph>>>>,
}

#[derive(Default)]
struct RubyDatabaseOpenOptions {
    named: DatabaseOpenOptions,
    has_database_dir: bool,
    wal_dir: Option<String>,
    snapshot_dir: Option<String>,
    snapshot_every_commits: Option<u64>,
    snapshot_keep_old: Option<usize>,
    has_snapshot_codec: bool,
    snapshot_codec: SnapshotOptions,
}

impl RubyDatabaseOpenOptions {
    fn has_explicit_wal_options(&self) -> bool {
        self.wal_dir.is_some()
            || self.snapshot_dir.is_some()
            || self.snapshot_every_commits.is_some()
            || self.snapshot_keep_old.is_some()
            || self.has_snapshot_codec
    }

    fn has_snapshot_tuning_options(&self) -> bool {
        self.snapshot_every_commits.is_some()
            || self.snapshot_keep_old.is_some()
            || self.has_snapshot_codec
    }
}

impl Database {
    fn from_db(db: Arc<InnerDatabase<InMemoryGraph>>) -> Self {
        Self {
            db: Mutex::new(Some(db)),
        }
    }
}

/// Drops of persistent databases handed off by the GC, not yet finished.
static BACKGROUND_DROPS: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

/// Set once the process is exiting: drops then run in place.
static EXITING: AtomicBool = AtomicBool::new(false);

/// The GC frees an unreachable handle during a sweep, with the GVL held.
/// Dropping the last handle to a persistent database flushes and closes its
/// WAL (`fsync`, joining its flusher), which would stall every Ruby thread
/// for the duration, so that drop moves to a thread of its own, like
/// `close` runs it GVL-free. An in-memory database, or any drop at exit,
/// runs in place. Opening a database first waits for pending drops (see
/// [`wait_background_drops`]) so a reopened WAL directory is free.
impl Drop for Database {
    fn drop(&mut self) {
        let slot = self.db.get_mut().unwrap_or_else(|p| p.into_inner()).take();
        let Some(db) = slot else { return };
        if db.wal().is_none() || EXITING.load(Ordering::SeqCst) {
            return;
        }
        let mut pending = BACKGROUND_DROPS.lock().unwrap_or_else(|p| p.into_inner());
        pending.retain(|handle| !handle.is_finished());
        // Spawning moves `db` into the closure even when it fails, so a
        // failed spawn drops it here, in place.
        if let Ok(handle) = std::thread::Builder::new()
            .name("lora-ruby-drop".into())
            .spawn(move || drop(db))
        {
            pending.push(handle);
        }
    }
}

/// Join every pending background drop.
fn join_background_drops() {
    let pending = std::mem::take(&mut *BACKGROUND_DROPS.lock().unwrap_or_else(|p| p.into_inner()));
    for handle in pending {
        let _ = handle.join();
    }
}

/// Before opening a database: let GC-dropped databases finish closing, with
/// the GVL released, so their WAL directories are free again.
fn wait_background_drops() {
    let idle = BACKGROUND_DROPS
        .lock()
        .map(|pending| pending.iter().all(|h| h.is_finished()))
        .unwrap_or(false);
    if idle {
        BACKGROUND_DROPS
            .lock()
            .map(|mut pending| pending.clear())
            .ok();
        return;
    }
    let _ = without_gvl(join_background_drops);
}

/// `at_exit` hook: drop in place from now on and wait for pending drops.
unsafe extern "C" fn finish_background_drops(_data: rb_sys::VALUE) {
    EXITING.store(true, Ordering::SeqCst);
    let _ = without_gvl(join_background_drops);
}

// Constructors — we expose `Database.create` and `Database.new` as
// singletons so callers can use whichever idiom they prefer; both are
// cost-equivalent.
fn database_new(ruby: &Ruby, args: &[Value]) -> Result<Database, MagnusError> {
    let (database_name, options) = database_open_args(ruby, args)?;
    if database_name.is_some() {
        wait_background_drops();
    }
    let db = without_gvl_string_result(ruby, move || open_database(database_name, options))?;
    Ok(Database::from_db(db))
}

fn database_create(ruby: &Ruby, args: &[Value]) -> Result<Database, MagnusError> {
    database_new(ruby, args)
}

fn database_open_wal(ruby: &Ruby, args: &[Value]) -> Result<Database, MagnusError> {
    let (wal_dir, mut options) = match args.len() {
        1 | 2 => {
            let wal_dir = RString::try_convert(args[0])?.to_string()?;
            let options = if args.len() == 2 {
                ruby_database_open_options(ruby, RHash::try_convert(args[1])?)?
            } else {
                RubyDatabaseOpenOptions::default()
            };
            (wal_dir, options)
        }
        n => {
            return Err(MagnusError::new(
                ruby.exception_arg_error(),
                format!("wrong number of arguments (given {n}, expected 1..2)"),
            ));
        }
    };
    if options.wal_dir.is_some() {
        return Err(invalid_params(
            ruby,
            "wal_dir must be passed as the first argument to open_wal",
        ));
    }
    options.wal_dir = Some(wal_dir);
    wait_background_drops();
    let db = without_gvl_string_result(ruby, move || open_wal_database(options))?;
    Ok(Database::from_db(db))
}

/// `clear` takes the engine's writer lock, so it waits for any write in
/// flight. That wait runs with the GVL released like every other engine
/// call: holding it would freeze every Ruby thread until the running
/// writer committed. When no write can be in flight (an in-memory database
/// and no GVL-free call running, see [`gvl::nothing_released`]) the lock
/// is free and clear runs at once, so it skips the GVL round trip.
fn database_clear(ruby: &Ruby, rb_self: &Database) -> Result<(), MagnusError> {
    let db = database_inner(ruby, rb_self)?;
    if db.wal().is_none() && gvl::nothing_released() {
        return db.try_clear().map_err(|e| query_error_from_anyhow(ruby, e));
    }
    without_gvl_lora_result(ruby, move || db.try_clear())
}

/// Dropping the last handle to a persistent database flushes and closes its
/// WAL, so that drop runs with the GVL released like any other engine
/// call. An in-memory database has nothing to flush and drops in place.
fn database_close(ruby: &Ruby, rb_self: &Database) -> Result<(), MagnusError> {
    let db = rb_self
        .db
        .lock()
        .map_err(|_| query_error(ruby, "database lock poisoned"))?
        .take();
    match db {
        Some(db) if db.wal().is_some() => {
            without_gvl(move || drop(db)).map_err(|panic| query_error(ruby, panic.to_string()))?
        }
        _ => {}
    }
    Ok(())
}

fn database_node_count(ruby: &Ruby, rb_self: &Database) -> Result<u64, MagnusError> {
    Ok(database_inner(ruby, rb_self)?.node_count() as u64)
}

fn database_relationship_count(ruby: &Ruby, rb_self: &Database) -> Result<u64, MagnusError> {
    Ok(database_inner(ruby, rb_self)?.relationship_count() as u64)
}

fn database_inspect(rb_self: &Database) -> String {
    match rb_self
        .db
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().cloned())
    {
        Some(db) => format!(
            "#<LoraRuby::Database nodes={} relationships={}>",
            db.node_count(),
            db.relationship_count(),
        ),
        None => "#<LoraRuby::Database closed>".to_string(),
    }
}

fn database_save_snapshot(
    ruby: &Ruby,
    rb_self: &Database,
    args: &[Value],
) -> Result<RHash, MagnusError> {
    let (path, options) = snapshot_file_args(ruby, args)?;
    let db = database_inner(ruby, rb_self)?;
    let meta = without_gvl_lora_result(ruby, move || {
        db.save_snapshot_to_with_options(&path, &options)
    })?;
    snapshot_meta_to_rhash(ruby, meta)
}

fn database_load_snapshot(
    ruby: &Ruby,
    rb_self: &Database,
    args: &[Value],
) -> Result<RHash, MagnusError> {
    let (path, credentials) = snapshot_load_file_args(ruby, args)?;
    let db = database_inner(ruby, rb_self)?;
    let meta = without_gvl_lora_result(ruby, move || {
        db.load_snapshot_from_with_credentials(&path, credentials.as_ref())
    })?;
    snapshot_meta_to_rhash(ruby, meta)
}

fn snapshot_file_args(
    ruby: &Ruby,
    args: &[Value],
) -> Result<(String, SnapshotOptions), MagnusError> {
    match args.len() {
        1 | 2 => {
            let path = RString::try_convert(args[0])?.to_string()?;
            let json = if args.len() == 2 {
                ruby_optional_to_json(ruby, args[1])?
            } else {
                None
            };
            let options = lora_database::snapshot_options_from_json(json)
                .map_err(|e| invalid_params(ruby, format!("invalid snapshot options: {e}")))?;
            Ok((path, options))
        }
        n => Err(MagnusError::new(
            ruby.exception_arg_error(),
            format!("wrong number of arguments (given {n}, expected 1..2)"),
        )),
    }
}

fn snapshot_load_file_args(
    ruby: &Ruby,
    args: &[Value],
) -> Result<(String, Option<lora_database::SnapshotCredentials>), MagnusError> {
    match args.len() {
        1 | 2 => {
            let path = RString::try_convert(args[0])?.to_string()?;
            let json = if args.len() == 2 {
                ruby_optional_to_json(ruby, args[1])?
            } else {
                None
            };
            let credentials = lora_database::snapshot_credentials_from_json(json)
                .map_err(|e| invalid_params(ruby, format!("invalid snapshot credentials: {e}")))?;
            Ok((path, credentials))
        }
        n => Err(MagnusError::new(
            ruby.exception_arg_error(),
            format!("wrong number of arguments (given {n}, expected 1..2)"),
        )),
    }
}

fn snapshot_meta_to_rhash(
    ruby: &Ruby,
    meta: lora_database::SnapshotMeta,
) -> Result<RHash, MagnusError> {
    let h = ruby.hash_new();
    h.aset(
        ruby.str_new("formatVersion"),
        ruby.integer_from_i64(meta.format_version as i64),
    )?;
    h.aset(
        ruby.str_new("nodeCount"),
        ruby.integer_from_i64(meta.node_count as i64),
    )?;
    h.aset(
        ruby.str_new("relationshipCount"),
        ruby.integer_from_i64(meta.relationship_count as i64),
    )?;
    match meta.wal_lsn {
        Some(lsn) => h.aset(ruby.str_new("walLsn"), ruby.integer_from_i64(lsn as i64))?,
        None => h.aset(ruby.str_new("walLsn"), ruby.qnil())?,
    }
    Ok(h)
}

fn database_open_args(
    ruby: &Ruby,
    args: &[Value],
) -> Result<(Option<String>, RubyDatabaseOpenOptions), MagnusError> {
    match args.len() {
        0 => Ok((None, RubyDatabaseOpenOptions::default())),
        1 => {
            if args[0].is_nil() {
                Ok((None, RubyDatabaseOpenOptions::default()))
            } else if let Ok(hash) = RHash::try_convert(args[0]) {
                Ok((None, ruby_database_open_options(ruby, hash)?))
            } else {
                Ok((
                    Some(RString::try_convert(args[0])?.to_string()?),
                    RubyDatabaseOpenOptions::default(),
                ))
            }
        }
        2 => {
            let database_name = if args[0].is_nil() {
                None
            } else {
                Some(RString::try_convert(args[0])?.to_string()?)
            };
            let options = ruby_database_open_options(ruby, RHash::try_convert(args[1])?)?;
            Ok((database_name, options))
        }
        n => Err(MagnusError::new(
            ruby.exception_arg_error(),
            format!("wrong number of arguments (given {n}, expected 0..2)"),
        )),
    }
}

fn ruby_database_open_options(
    ruby: &Ruby,
    hash: RHash,
) -> Result<RubyDatabaseOpenOptions, MagnusError> {
    let mut options = RubyDatabaseOpenOptions::default();
    if let Some(dir) = hash_get_any(ruby, hash, &["database_dir", "databaseDir"]) {
        options.named.database_dir = RString::try_convert(dir)?.to_string()?.into();
        options.has_database_dir = true;
    }
    if let Some(dir) = hash_get_any(ruby, hash, &["wal_dir", "walDir"]) {
        options.wal_dir = Some(RString::try_convert(dir)?.to_string()?);
    }
    if let Some(dir) = hash_get_any(ruby, hash, &["snapshot_dir", "snapshotDir"]) {
        options.snapshot_dir = Some(RString::try_convert(dir)?.to_string()?);
    }
    if let Some(value) = hash_get_any(
        ruby,
        hash,
        &["snapshot_every_commits", "snapshotEveryCommits"],
    ) {
        options.snapshot_every_commits = Some(read_nonnegative_u64(ruby, value)?);
    }
    if let Some(value) = hash_get_any(ruby, hash, &["snapshot_keep_old", "snapshotKeepOld"]) {
        let keep_old = read_nonnegative_u64(ruby, value)?;
        options.snapshot_keep_old = Some(
            usize::try_from(keep_old)
                .map_err(|_| invalid_params(ruby, "snapshot_keep_old does not fit in usize"))?,
        );
    }
    if let Some(value) = hash_get_any(ruby, hash, &["snapshot_options", "snapshotOptions"]) {
        let json = ruby_optional_to_json(ruby, value)?;
        options.has_snapshot_codec = true;
        options.snapshot_codec = lora_database::snapshot_options_from_json(json)
            .map_err(|e| invalid_params(ruby, format!("invalid snapshot options: {e}")))?;
    }
    Ok(options)
}

fn open_database(
    database_name: Option<String>,
    options: RubyDatabaseOpenOptions,
) -> Result<Arc<InnerDatabase<InMemoryGraph>>, String> {
    if options.has_explicit_wal_options() {
        return Err(
            "wal_dir/snapshot_dir are not valid for Database.create; use Database.open_wal"
                .to_string(),
        );
    }
    let db = match database_name {
        Some(name) => InnerDatabase::open_named(name, options.named).map_err(|e| e.to_string())?,
        None => {
            if options.has_database_dir {
                return Err("database_name is required when database_dir is provided".to_string());
            }
            InnerDatabase::in_memory()
        }
    };
    Ok(Arc::new(db))
}

fn open_wal_database(
    options: RubyDatabaseOpenOptions,
) -> Result<Arc<InnerDatabase<InMemoryGraph>>, String> {
    if options.has_database_dir {
        return Err("database_dir is not valid for Database.open_wal".to_string());
    }
    let has_snapshot_tuning = options.has_snapshot_tuning_options();
    if options.snapshot_dir.is_none() && has_snapshot_tuning {
        return Err(
            "snapshot_dir is required when managed snapshot options are provided".to_string(),
        );
    }
    let wal_dir = options
        .wal_dir
        .ok_or_else(|| "wal_dir is required for Database.open_wal".to_string())?;
    let wal_config = WalConfig::enabled(wal_dir);
    let db = if let Some(snapshot_dir) = options.snapshot_dir {
        let mut snapshots = SnapshotConfig::enabled(snapshot_dir)
            .keep_old(options.snapshot_keep_old.unwrap_or(1))
            .codec(options.snapshot_codec);
        if let Some(every) = options.snapshot_every_commits {
            if every != 0 {
                snapshots = snapshots.every_commits(every);
            }
        }
        InnerDatabase::open_with_wal_snapshots(wal_config, snapshots).map_err(|e| e.to_string())?
    } else {
        InnerDatabase::open_with_wal(wal_config).map_err(|e| e.to_string())?
    };
    Ok(Arc::new(db))
}

fn database_inner(
    ruby: &Ruby,
    rb_self: &Database,
) -> Result<Arc<InnerDatabase<InMemoryGraph>>, MagnusError> {
    let slot = rb_self
        .db
        .lock()
        .map_err(|_| query_error(ruby, "database lock poisoned"))?;
    slot.as_ref()
        .cloned()
        .ok_or_else(|| query_error(ruby, "database is closed"))
}

fn without_gvl_string_result<T>(
    ruby: &Ruby,
    f: impl FnOnce() -> Result<T, String> + Send,
) -> Result<T, MagnusError>
where
    T: Send,
{
    without_gvl(f)
        .map_err(|panic| query_error(ruby, panic.to_string()))?
        .map_err(|e| query_error(ruby, e))
}

fn without_gvl_lora_result<T, E>(
    ruby: &Ruby,
    f: impl FnOnce() -> Result<T, E> + Send,
) -> Result<T, MagnusError>
where
    T: Send,
    E: Into<lora_database::LoraError> + Send,
{
    without_gvl(f)
        .map_err(|panic| query_error(ruby, panic.to_string()))?
        .map_err(|e| query_error_from_anyhow(ruby, e))
}

/// Run `call` with the GVL released. With a `timeout` it runs under a
/// cancellable deadline: it fails with `LORA_TIMEOUT` once the timeout
/// passes, and a Ruby interrupt of this thread (`Thread#raise`,
/// `Thread#kill`, `Timeout.timeout`, Ctrl-C) cancels it. A cancelled query
/// rolls back and releases its locks; the interrupt is then delivered
/// instead of the timeout error.
///
/// Without a timeout the query runs without a deadline and cannot be
/// interrupted: the engine reads the clock at every deadline check, which
/// costs roughly 10% on scans, so plain calls do not pay for it.
fn run_query<T>(
    ruby: &Ruby,
    timeout: Option<Duration>,
    call: impl FnOnce(Option<web_time::Instant>) -> Result<T, LoraError> + Send,
) -> Result<T, MagnusError>
where
    T: Send,
{
    let Some(timeout) = timeout else {
        return without_gvl_lora_result(ruby, move || call(None));
    };
    let cancel = Mutex::new(CancellableDeadline::new(Some(timeout)));
    let deadline = cancel.lock().unwrap_or_else(|p| p.into_inner()).deadline();
    let outcome = without_gvl_cancellable(move || call(Some(deadline)), &cancel);
    if cancel
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_cancelled()
    {
        ruby.thread_check_ints()?;
    }
    outcome
        .map_err(|panic| query_error(ruby, panic.to_string()))?
        .map_err(|e| query_error_from_anyhow(ruby, e))
}

/// Arguments of `execute` / `profile`: `(query, params = nil, timeout_ms: nil)`.
struct QueryArgs {
    query: String,
    params: Option<BTreeMap<String, LoraValue>>,
    timeout: Option<Duration>,
}

/// Parse `(query, params = nil, timeout_ms: nil)`. Keywords other than
/// `timeout_ms` are query parameters when no positional `params` hash is
/// given, so `execute(q, name: "Ada")` keeps working.
fn parse_query_args(ruby: &Ruby, args: &[Value]) -> Result<QueryArgs, MagnusError> {
    // SAFETY: called from a method body, while Ruby's frame for this
    // call is current.
    let keywords_given = unsafe { rb_sys::rb_keyword_given_p() } != 0;
    let (positional, keywords) = match (keywords_given, args.split_last()) {
        (true, Some((last, rest))) => (rest, Some(RHash::try_convert(*last)?)),
        _ => (args, None),
    };
    let (query, params_value) = parse_query_params(ruby, positional)?;

    let mut timeout = None;
    let mut keyword_params = None;
    if let Some(keywords) = keywords {
        let timeout_value = keywords.delete(ruby.to_symbol("timeout_ms"))?;
        timeout = parse_timeout_ms(ruby, timeout_value)?;
        if !keywords.is_empty() {
            if params_value.is_some() {
                return Err(MagnusError::new(
                    ruby.exception_arg_error(),
                    "unknown keyword: only timeout_ms is accepted next to a params hash",
                ));
            }
            keyword_params = Some(keywords.as_value());
        }
    }
    let params = match params_value.or(keyword_params) {
        Some(v) => Some(ruby_value_to_params(ruby, v)?),
        None => None,
    };
    Ok(QueryArgs {
        query,
        params,
        timeout,
    })
}

/// `timeout_ms:` is `nil` (no timeout) or a positive integer.
fn parse_timeout_ms(ruby: &Ruby, value: Option<Value>) -> Result<Option<Duration>, MagnusError> {
    let Some(value) = value.filter(|v| !v.is_nil()) else {
        return Ok(None);
    };
    let ms = read_nonnegative_u64(ruby, value)?;
    if ms == 0 {
        return Err(invalid_params(
            ruby,
            "timeout_ms must be a positive integer",
        ));
    }
    Ok(Some(Duration::from_millis(ms)))
}

/// `execute(query, params = nil, timeout_ms: nil)` — `-1` arity so
/// `params` is optional and we can distinguish "not passed" from `nil`/`{}`
/// (both map to empty params). Everything that touches Ruby values happens
/// under the GVL; only the pure-Rust engine call is run GVL-released.
fn database_execute(ruby: &Ruby, rb_self: &Database, args: &[Value]) -> Result<RHash, MagnusError> {
    // Parse params while we still hold the GVL — touching Ruby `RHash` /
    // `RArray` from a GVL-released region is undefined behaviour.
    let QueryArgs {
        query,
        params,
        timeout,
    } = parse_query_args(ruby, args)?;
    let params_map = params.unwrap_or_default();

    // Run the engine with the GVL released. Everything inside the closure
    // is pure Rust — no Ruby values cross the boundary — which keeps this
    // sound. `Rows` hands back the engine's rows without the per-cell
    // clones of the `RowArrays` projection; the columns are the first
    // row's names, exactly as `RowArrays` infers them.
    let db = database_inner(ruby, rb_self)?;
    let exec_result = run_query(ruby, timeout, move |deadline| {
        let options = Some(ExecuteOptions {
            format: ResultFormat::Rows,
        });
        match deadline {
            Some(deadline) => {
                db.execute_with_params_deadline(&query, options, params_map, deadline)
            }
            None => db.execute_with_params(&query, options, params_map),
        }
    })?;

    let rows = match exec_result {
        QueryResult::Rows(r) => r.rows,
        _ => return Err(query_error(ruby, "expected Rows result")),
    };

    let out = ruby.hash_new();
    let columns = ruby.ary_new();
    let mut column_names = Vec::new();
    if let Some(first) = rows.first() {
        for (_, name, _) in first.iter_named() {
            columns.push(ruby.str_new(&name))?;
            // A frozen key is stored as is; an unfrozen one would be
            // copied and frozen again for every row.
            let key = ruby.str_new(&name);
            key.freeze();
            column_names.push(key);
        }
    }
    out.aset(ruby.str_new("columns"), columns)?;

    let rows_out = ruby.ary_new_capa(rows.len());
    for row in &rows {
        let row_hash = ruby.hash_new();
        for (name, (_, val)) in column_names.iter().zip(row.iter()) {
            row_hash.aset(*name, lora_value_to_ruby(ruby, val)?)?;
        }
        rows_out.push(row_hash)?;
    }
    out.aset(ruby.str_new("rows"), rows_out)?;
    Ok(out)
}

/// `explain(query, params = nil)` — compile a query and return the plan
/// as a Ruby `Hash` without invoking the executor. Mutating queries
/// produce no side effects.
fn database_explain(ruby: &Ruby, rb_self: &Database, args: &[Value]) -> Result<RHash, MagnusError> {
    let (query, params_value) = parse_query_params(ruby, args)?;
    let params_map = match params_value {
        Some(v) => Some(ruby_value_to_params(ruby, v)?),
        None => None,
    };
    let db = database_inner(ruby, rb_self)?;
    let plan = without_gvl_lora_result(ruby, move || db.explain(&query, params_map))?;
    query_plan_to_ruby(ruby, &plan)
}

/// `profile(query, params = nil, timeout_ms: nil)` — execute a query and return the plan
/// plus runtime metrics as a Ruby `Hash`.
///
/// **PROFILE executes the query for real.** Mutating queries are
/// persisted exactly as in `execute`. Use `explain` to inspect a
/// mutating plan without running it.
fn database_profile(ruby: &Ruby, rb_self: &Database, args: &[Value]) -> Result<RHash, MagnusError> {
    let QueryArgs {
        query,
        params,
        timeout,
    } = parse_query_args(ruby, args)?;
    let db = database_inner(ruby, rb_self)?;
    let prof = run_query(ruby, timeout, move |deadline| {
        db.profile_with_deadline(&query, params, deadline)
    })?;
    query_profile_to_ruby(ruby, &prof)
}

fn parse_query_params(ruby: &Ruby, args: &[Value]) -> Result<(String, Option<Value>), MagnusError> {
    match args.len() {
        1 => Ok((RString::try_convert(args[0])?.to_string()?, None)),
        2 => {
            let q = RString::try_convert(args[0])?.to_string()?;
            let p = if args[1].is_nil() {
                None
            } else {
                Some(args[1])
            };
            Ok((q, p))
        }
        n => Err(MagnusError::new(
            ruby.exception_arg_error(),
            format!("wrong number of arguments (given {n}, expected 1..2)"),
        )),
    }
}
