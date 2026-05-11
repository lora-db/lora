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
use std::sync::{Arc, Mutex};

use magnus::{
    function, method, prelude::*, value::ReprValue, Error as MagnusError, RHash, RString, Ruby,
    Value,
};

use lora_database::{
    Database as InnerDatabase, DatabaseOpenOptions, ExecuteOptions, InMemoryGraph, QueryResult,
    ResultFormat, SnapshotConfig, SnapshotOptions, WalConfig,
};

mod errors;
mod from_ruby;
mod gvl;
mod to_ruby;

use errors::{invalid_params, query_error, query_error_from_anyhow};
use from_ruby::{hash_get_any, read_nonnegative_u64, ruby_optional_to_json, ruby_value_to_params};
use gvl::without_gvl;
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

// Constructors — we expose `Database.create` and `Database.new` as
// singletons so callers can use whichever idiom they prefer; both are
// cost-equivalent.
fn database_new(ruby: &Ruby, args: &[Value]) -> Result<Database, MagnusError> {
    let (database_name, options) = database_open_args(ruby, args)?;
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
    let db = without_gvl_string_result(ruby, move || open_wal_database(options))?;
    Ok(Database::from_db(db))
}

fn database_clear(ruby: &Ruby, rb_self: &Database) -> Result<(), MagnusError> {
    database_inner(ruby, rb_self)?.clear();
    Ok(())
}

fn database_close(ruby: &Ruby, rb_self: &Database) -> Result<(), MagnusError> {
    let mut slot = rb_self
        .db
        .lock()
        .map_err(|_| query_error(ruby, "database lock poisoned"))?;
    slot.take();
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

/// `execute(query, params = nil)` — `-1` arity so `params` is optional and
/// we can distinguish "not passed" from `nil`/`{}` (both map to empty
/// params). Everything that touches Ruby values happens under the GVL;
/// only the pure-Rust engine call is run GVL-released.
fn database_execute(ruby: &Ruby, rb_self: &Database, args: &[Value]) -> Result<RHash, MagnusError> {
    let (query, params_value): (String, Option<Value>) = match args.len() {
        1 => {
            let q = RString::try_convert(args[0])?.to_string()?;
            (q, None)
        }
        2 => {
            let q = RString::try_convert(args[0])?.to_string()?;
            let p = if args[1].is_nil() {
                None
            } else {
                Some(args[1])
            };
            (q, p)
        }
        n => {
            return Err(MagnusError::new(
                ruby.exception_arg_error(),
                format!("wrong number of arguments (given {n}, expected 1..2)"),
            ));
        }
    };

    // Parse params while we still hold the GVL — touching Ruby `RHash` /
    // `RArray` from a GVL-released region is undefined behaviour.
    let params_map = match params_value {
        Some(v) => ruby_value_to_params(ruby, v)?,
        None => BTreeMap::new(),
    };

    // Run the engine with the GVL released. Everything inside the closure
    // is pure Rust — no Ruby values cross the boundary — which keeps this
    // sound.
    let db = database_inner(ruby, rb_self)?;
    let exec_result = without_gvl_lora_result(ruby, move || {
        let options = ExecuteOptions {
            format: ResultFormat::RowArrays,
        };
        db.execute_with_params(&query, Some(options), params_map)
    })?;

    let row_arrays = match exec_result {
        QueryResult::RowArrays(r) => r,
        _ => return Err(query_error(ruby, "expected RowArrays result")),
    };

    let out = ruby.hash_new();
    let columns = ruby.ary_new();
    for c in &row_arrays.columns {
        columns.push(ruby.str_new(c))?;
    }
    out.aset(ruby.str_new("columns"), columns)?;

    let rows = ruby.ary_new();
    for row in &row_arrays.rows {
        let row_hash = ruby.hash_new();
        for (col, val) in row_arrays.columns.iter().zip(row.iter()) {
            row_hash.aset(ruby.str_new(col), lora_value_to_ruby(ruby, val)?)?;
        }
        rows.push(row_hash)?;
    }
    out.aset(ruby.str_new("rows"), rows)?;
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

/// `profile(query, params = nil)` — execute a query and return the plan
/// plus runtime metrics as a Ruby `Hash`.
///
/// **PROFILE executes the query for real.** Mutating queries are
/// persisted exactly as in `execute`. Use `explain` to inspect a
/// mutating plan without running it.
fn database_profile(ruby: &Ruby, rb_self: &Database, args: &[Value]) -> Result<RHash, MagnusError> {
    let (query, params_value) = parse_query_params(ruby, args)?;
    let params_map = match params_value {
        Some(v) => Some(ruby_value_to_params(ruby, v)?),
        None => None,
    };
    let db = database_inner(ruby, rb_self)?;
    let prof = without_gvl_lora_result(ruby, move || db.profile(&query, params_map))?;
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
