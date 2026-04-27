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
use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::sync::{Arc, Mutex};

use magnus::{
    function, method, prelude::*, r_hash::ForEach, value::ReprValue, Error as MagnusError,
    ExceptionClass, Float, Integer, RArray, RHash, RModule, RString, Ruby, Symbol, Value,
};

use lora_database::{
    Database as InnerDatabase, DatabaseOpenOptions, ExecuteOptions, InMemoryGraph, LoraValue,
    QueryResult, ResultFormat,
};
use lora_store::{
    LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint, LoraTime,
    LoraVector, RawCoordinate, VectorCoordinateType, VectorValues,
};

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
    database.define_method("execute", method!(database_execute, -1))?;
    database.define_method("clear", method!(database_clear, 0))?;
    database.define_method("close", method!(database_close, 0))?;
    database.define_method("node_count", method!(database_node_count, 0))?;
    database.define_method(
        "relationship_count",
        method!(database_relationship_count, 0),
    )?;
    database.define_method("inspect", method!(database_inspect, 0))?;
    database.define_method("to_s", method!(database_inspect, 0))?;
    database.define_method("save_snapshot", method!(database_save_snapshot, 1))?;
    database.define_method("load_snapshot", method!(database_load_snapshot, 1))?;

    // `LoraRuby::VERSION` is owned by `lib/lora_ruby/version.rb` so the
    // gem can expose a version before the native extension compiles
    // (during `gem build` / `bundle install`). Defining it here too
    // would trigger a "already initialized constant" warning on load.

    Ok(())
}

// ============================================================================
// Error lookups
// ============================================================================

fn lora_module(ruby: &Ruby) -> RModule {
    ruby.class_object()
        .const_get::<_, RModule>("LoraRuby")
        .expect("LoraRuby module is defined by `init` before any method runs")
}

fn lora_error_class(ruby: &Ruby, name: &str) -> ExceptionClass {
    // `const_get::<_, ExceptionClass>` converts the stored RClass into
    // an ExceptionClass — this is the sound path, because our subclasses
    // of StandardError retain the exception-class trait on the Ruby
    // side even though `define_class` typed them as RClass.
    lora_module(ruby)
        .const_get::<_, ExceptionClass>(name)
        .unwrap_or_else(|_| ruby.exception_standard_error())
}

fn query_error(ruby: &Ruby, msg: impl Into<String>) -> MagnusError {
    MagnusError::new(lora_error_class(ruby, "QueryError"), msg.into())
}

fn invalid_params(ruby: &Ruby, msg: impl Into<String>) -> MagnusError {
    MagnusError::new(lora_error_class(ruby, "InvalidParamsError"), msg.into())
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
    let db = without_gvl(move || open_database(database_name, options))
        .map_err(|e| query_error(ruby, e))?;
    Ok(Database::from_db(db))
}

fn database_create(ruby: &Ruby, args: &[Value]) -> Result<Database, MagnusError> {
    database_new(ruby, args)
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
    path: RString,
) -> Result<RHash, MagnusError> {
    let path = path.to_string()?;
    let db = database_inner(ruby, rb_self)?;
    let meta = without_gvl(move || db.save_snapshot_to(&path))
        .map_err(|e| query_error(ruby, format!("{e}")))?;
    snapshot_meta_to_rhash(ruby, meta)
}

fn database_load_snapshot(
    ruby: &Ruby,
    rb_self: &Database,
    path: RString,
) -> Result<RHash, MagnusError> {
    let path = path.to_string()?;
    let db = database_inner(ruby, rb_self)?;
    let meta = without_gvl(move || db.load_snapshot_from(&path))
        .map_err(|e| query_error(ruby, format!("{e}")))?;
    snapshot_meta_to_rhash(ruby, meta)
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
) -> Result<(Option<String>, DatabaseOpenOptions), MagnusError> {
    match args.len() {
        0 => Ok((None, DatabaseOpenOptions::default())),
        1 => {
            if args[0].is_nil() {
                Ok((None, DatabaseOpenOptions::default()))
            } else {
                Ok((
                    Some(RString::try_convert(args[0])?.to_string()?),
                    DatabaseOpenOptions::default(),
                ))
            }
        }
        2 => {
            let database_name = if args[0].is_nil() {
                None
            } else {
                Some(RString::try_convert(args[0])?.to_string()?)
            };
            let mut options = DatabaseOpenOptions::default();
            let hash = RHash::try_convert(args[1])?;
            if let Some(dir) = hash_get_either(ruby, hash, "database_dir")
                .or_else(|| hash_get_either(ruby, hash, "databaseDir"))
            {
                options.database_dir = RString::try_convert(dir)?.to_string()?.into();
            }
            Ok((database_name, options))
        }
        n => Err(MagnusError::new(
            ruby.exception_arg_error(),
            format!("wrong number of arguments (given {n}, expected 0..2)"),
        )),
    }
}

fn open_database(
    database_name: Option<String>,
    options: DatabaseOpenOptions,
) -> Result<Arc<InnerDatabase<InMemoryGraph>>, String> {
    let db = match database_name {
        Some(name) => InnerDatabase::open_named(name, options).map_err(|e| e.to_string())?,
        None => InnerDatabase::in_memory(),
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
    let exec_result = without_gvl(move || {
        let options = ExecuteOptions {
            format: ResultFormat::RowArrays,
        };
        db.execute_with_params(&query, Some(options), params_map)
    });

    let row_arrays = match exec_result {
        Ok(QueryResult::RowArrays(r)) => r,
        Ok(_) => return Err(query_error(ruby, "expected RowArrays result")),
        Err(e) => return Err(query_error(ruby, format!("{e}"))),
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

// ============================================================================
// LoraValue → Ruby
// ============================================================================

fn lora_value_to_ruby(ruby: &Ruby, value: &LoraValue) -> Result<Value, MagnusError> {
    match value {
        LoraValue::Null => Ok(ruby.qnil().as_value()),
        LoraValue::Bool(b) => Ok(if *b {
            ruby.qtrue().as_value()
        } else {
            ruby.qfalse().as_value()
        }),
        LoraValue::Int(i) => Ok(ruby.integer_from_i64(*i).as_value()),
        LoraValue::Float(f) => Ok(ruby.float_from_f64(*f).as_value()),
        LoraValue::String(s) => Ok(ruby.str_new(s).as_value()),
        LoraValue::List(items) => {
            let arr = ruby.ary_new();
            for item in items {
                arr.push(lora_value_to_ruby(ruby, item)?)?;
            }
            Ok(arr.as_value())
        }
        LoraValue::Map(m) => {
            let h = ruby.hash_new();
            for (k, v) in m {
                h.aset(ruby.str_new(k), lora_value_to_ruby(ruby, v)?)?;
            }
            Ok(h.as_value())
        }
        LoraValue::Node(id) => {
            let h = ruby.hash_new();
            h.aset(ruby.str_new("kind"), ruby.str_new("node"))?;
            h.aset(ruby.str_new("id"), ruby.integer_from_i64(*id as i64))?;
            h.aset(ruby.str_new("labels"), ruby.ary_new())?;
            h.aset(ruby.str_new("properties"), ruby.hash_new())?;
            Ok(h.as_value())
        }
        LoraValue::Relationship(id) => {
            let h = ruby.hash_new();
            h.aset(ruby.str_new("kind"), ruby.str_new("relationship"))?;
            h.aset(ruby.str_new("id"), ruby.integer_from_i64(*id as i64))?;
            Ok(h.as_value())
        }
        LoraValue::Path(p) => {
            let h = ruby.hash_new();
            h.aset(ruby.str_new("kind"), ruby.str_new("path"))?;
            let nodes = ruby.ary_new();
            for n in &p.nodes {
                nodes.push(ruby.integer_from_i64(*n as i64))?;
            }
            let rels = ruby.ary_new();
            for r in &p.rels {
                rels.push(ruby.integer_from_i64(*r as i64))?;
            }
            h.aset(ruby.str_new("nodes"), nodes)?;
            h.aset(ruby.str_new("rels"), rels)?;
            Ok(h.as_value())
        }
        LoraValue::Date(v) => tagged_iso(ruby, "date", v.to_string()),
        LoraValue::Time(v) => tagged_iso(ruby, "time", v.to_string()),
        LoraValue::LocalTime(v) => tagged_iso(ruby, "localtime", v.to_string()),
        LoraValue::DateTime(v) => tagged_iso(ruby, "datetime", v.to_string()),
        LoraValue::LocalDateTime(v) => tagged_iso(ruby, "localdatetime", v.to_string()),
        LoraValue::Duration(v) => tagged_iso(ruby, "duration", v.to_string()),
        LoraValue::Point(p) => point_to_ruby(ruby, p),
        LoraValue::Vector(v) => vector_to_ruby(ruby, v),
    }
}

fn vector_to_ruby(ruby: &Ruby, v: &LoraVector) -> Result<Value, MagnusError> {
    let h = ruby.hash_new();
    h.aset(ruby.str_new("kind"), ruby.str_new("vector"))?;
    h.aset(
        ruby.str_new("dimension"),
        ruby.integer_from_i64(v.dimension as i64),
    )?;
    h.aset(
        ruby.str_new("coordinateType"),
        ruby.str_new(v.coordinate_type().as_str()),
    )?;

    let values = ruby.ary_new();
    match &v.values {
        VectorValues::Float64(vs) => {
            for x in vs {
                values.push(ruby.float_from_f64(*x))?;
            }
        }
        VectorValues::Float32(vs) => {
            for x in vs {
                values.push(ruby.float_from_f64(*x as f64))?;
            }
        }
        VectorValues::Integer64(vs) => {
            for x in vs {
                values.push(ruby.integer_from_i64(*x))?;
            }
        }
        VectorValues::Integer32(vs) => {
            for x in vs {
                values.push(ruby.integer_from_i64(*x as i64))?;
            }
        }
        VectorValues::Integer16(vs) => {
            for x in vs {
                values.push(ruby.integer_from_i64(*x as i64))?;
            }
        }
        VectorValues::Integer8(vs) => {
            for x in vs {
                values.push(ruby.integer_from_i64(*x as i64))?;
            }
        }
    }
    h.aset(ruby.str_new("values"), values)?;
    Ok(h.as_value())
}

fn tagged_iso(ruby: &Ruby, kind: &str, iso: String) -> Result<Value, MagnusError> {
    let h = ruby.hash_new();
    h.aset(ruby.str_new("kind"), ruby.str_new(kind))?;
    h.aset(ruby.str_new("iso"), ruby.str_new(&iso))?;
    Ok(h.as_value())
}

/// Render a `LoraPoint` into the canonical external point shape — kept
/// 1:1 aligned with the `LoraPoint` union emitted by `lora-node` /
/// `lora-wasm` / `lora-python`.
fn point_to_ruby(ruby: &Ruby, p: &LoraPoint) -> Result<Value, MagnusError> {
    let h = ruby.hash_new();
    h.aset(ruby.str_new("kind"), ruby.str_new("point"))?;
    h.aset(ruby.str_new("srid"), ruby.integer_from_i64(p.srid as i64))?;
    h.aset(ruby.str_new("crs"), ruby.str_new(p.crs_name()))?;
    h.aset(ruby.str_new("x"), ruby.float_from_f64(p.x))?;
    h.aset(ruby.str_new("y"), ruby.float_from_f64(p.y))?;
    if let Some(z) = p.z {
        h.aset(ruby.str_new("z"), ruby.float_from_f64(z))?;
    }
    if p.is_geographic() {
        h.aset(
            ruby.str_new("longitude"),
            ruby.float_from_f64(p.longitude()),
        )?;
        h.aset(ruby.str_new("latitude"), ruby.float_from_f64(p.latitude()))?;
        if let Some(height) = p.height() {
            h.aset(ruby.str_new("height"), ruby.float_from_f64(height))?;
        }
    }
    Ok(h.as_value())
}

// ============================================================================
// Ruby → LoraValue (params)
// ============================================================================

fn ruby_value_to_params(
    ruby: &Ruby,
    value: Value,
) -> Result<BTreeMap<String, LoraValue>, MagnusError> {
    let hash = RHash::try_convert(value)
        .map_err(|_| invalid_params(ruby, "params must be a Hash keyed by parameter name"))?;
    hash_to_string_map(ruby, hash)
}

fn hash_to_string_map(
    ruby: &Ruby,
    hash: RHash,
) -> Result<BTreeMap<String, LoraValue>, MagnusError> {
    let mut out = BTreeMap::new();
    let mut inner_err: Option<MagnusError> = None;
    hash.foreach(|k: Value, v: Value| {
        let key = match coerce_key(ruby, k) {
            Ok(s) => s,
            Err(e) => {
                inner_err = Some(e);
                return Ok(ForEach::Stop);
            }
        };
        match ruby_value_to_lora(ruby, v) {
            Ok(lv) => {
                out.insert(key, lv);
                Ok(ForEach::Continue)
            }
            Err(e) => {
                inner_err = Some(e);
                Ok(ForEach::Stop)
            }
        }
    })?;
    if let Some(e) = inner_err {
        return Err(e);
    }
    Ok(out)
}

fn coerce_key(ruby: &Ruby, v: Value) -> Result<String, MagnusError> {
    // Accept both String and Symbol keys — idiomatic Ruby. Reject anything
    // else loudly; silently stringifying would mask caller mistakes.
    if let Ok(s) = RString::try_convert(v) {
        return s.to_string();
    }
    if let Ok(s) = Symbol::try_convert(v) {
        return Ok(s.name()?.into_owned());
    }
    Err(invalid_params(ruby, "param keys must be String or Symbol"))
}

fn ruby_value_to_lora(ruby: &Ruby, v: Value) -> Result<LoraValue, MagnusError> {
    if v.is_nil() {
        return Ok(LoraValue::Null);
    }
    // Check true/false before Integer — Ruby's TrueClass / FalseClass are
    // not Integer subclasses, but bool detection is cleaner first.
    if v.is_kind_of(ruby.class_true_class()) {
        return Ok(LoraValue::Bool(true));
    }
    if v.is_kind_of(ruby.class_false_class()) {
        return Ok(LoraValue::Bool(false));
    }
    // Float MUST be checked before Integer — `Integer::try_convert`
    // succeeds on Float because Ruby's `Float#to_int` (truncating
    // coercion) makes `Float` implicitly convertible. Taking that path
    // would turn `1.5` into `1` silently; callers never want that.
    if let Ok(f) = Float::try_convert(v) {
        return Ok(LoraValue::Float(f.to_f64()));
    }
    if let Ok(i) = Integer::try_convert(v) {
        return match i.to_i64() {
            Ok(n) => Ok(LoraValue::Int(n)),
            Err(_) => Err(invalid_params(
                ruby,
                "integer parameter does not fit in i64",
            )),
        };
    }
    if let Ok(s) = RString::try_convert(v) {
        return Ok(LoraValue::String(s.to_string()?));
    }
    if let Ok(sym) = Symbol::try_convert(v) {
        // Symbols round-trip as strings — same approach as YAML/JSON
        // mappings. Engine has no dedicated symbol value.
        return Ok(LoraValue::String(sym.name()?.into_owned()));
    }
    if let Ok(arr) = RArray::try_convert(v) {
        let mut out = Vec::with_capacity(arr.len());
        for item in arr.into_iter() {
            out.push(ruby_value_to_lora(ruby, item)?);
        }
        return Ok(LoraValue::List(out));
    }
    if let Ok(hash) = RHash::try_convert(v) {
        return ruby_hash_to_cypher(ruby, hash);
    }
    let class_name = unsafe { v.classname() }.into_owned();
    Err(invalid_params(
        ruby,
        format!("unsupported parameter type: {class_name}"),
    ))
}

/// A Hash might be a tagged value (date / time / …/ point) or a plain
/// map. Nodes / relationships / paths are opaque on the engine side and
/// cannot be reconstructed as params — there's no `"kind" => "node"`
/// tag handled here.
fn ruby_hash_to_cypher(ruby: &Ruby, hash: RHash) -> Result<LoraValue, MagnusError> {
    if let Some(kind) = lookup_kind(ruby, hash)? {
        match kind.as_str() {
            "date" => {
                return parse_tagged(ruby, hash, "date", |iso| {
                    LoraDate::parse(iso).map(LoraValue::Date)
                });
            }
            "time" => {
                return parse_tagged(ruby, hash, "time", |iso| {
                    LoraTime::parse(iso).map(LoraValue::Time)
                });
            }
            "localtime" => {
                return parse_tagged(ruby, hash, "localtime", |iso| {
                    LoraLocalTime::parse(iso).map(LoraValue::LocalTime)
                });
            }
            "datetime" => {
                return parse_tagged(ruby, hash, "datetime", |iso| {
                    LoraDateTime::parse(iso).map(LoraValue::DateTime)
                });
            }
            "localdatetime" => {
                return parse_tagged(ruby, hash, "localdatetime", |iso| {
                    LoraLocalDateTime::parse(iso).map(LoraValue::LocalDateTime)
                });
            }
            "duration" => {
                return parse_tagged(ruby, hash, "duration", |iso| {
                    LoraDuration::parse(iso).map(LoraValue::Duration)
                });
            }
            "point" => return build_point(ruby, hash),
            "vector" => return build_vector(ruby, hash),
            _ => { /* fall through to plain-map handling */ }
        }
    }

    Ok(LoraValue::Map(hash_to_string_map(ruby, hash)?))
}

/// Look up `"kind"` (string) or `:kind` (symbol) under either key. Keeps
/// constructor hashes usable with either Ruby idiom.
fn lookup_kind(ruby: &Ruby, hash: RHash) -> Result<Option<String>, MagnusError> {
    if let Some(v) = hash.get(ruby.str_new("kind")) {
        return kind_as_string(v).map(Some);
    }
    if let Some(v) = hash.get(ruby.to_symbol("kind")) {
        return kind_as_string(v).map(Some);
    }
    Ok(None)
}

fn kind_as_string(v: Value) -> Result<String, MagnusError> {
    if let Ok(s) = RString::try_convert(v) {
        return s.to_string();
    }
    if let Ok(s) = Symbol::try_convert(v) {
        return Ok(s.name()?.into_owned());
    }
    // Anything else means "not a tagged constructor" — return empty so
    // the caller falls through to plain-map handling instead of raising.
    Ok(String::new())
}

fn parse_tagged(
    ruby: &Ruby,
    hash: RHash,
    tag: &str,
    parse: impl FnOnce(&str) -> Result<LoraValue, String>,
) -> Result<LoraValue, MagnusError> {
    let iso = read_string(ruby, hash, "iso")?
        .ok_or_else(|| invalid_params(ruby, format!("{tag} value requires iso: String")))?;
    parse(&iso).map_err(|e| invalid_params(ruby, format!("{tag}: {e}")))
}

fn build_point(ruby: &Ruby, hash: RHash) -> Result<LoraValue, MagnusError> {
    let srid = read_u32(ruby, hash, "srid")?.unwrap_or(7203);
    let x = read_f64(ruby, hash, "x")?.ok_or_else(|| invalid_params(ruby, "point.x required"))?;
    let y = read_f64(ruby, hash, "y")?.ok_or_else(|| invalid_params(ruby, "point.y required"))?;
    let z = read_f64(ruby, hash, "z")?;
    Ok(LoraValue::Point(LoraPoint { x, y, z, srid }))
}

fn build_vector(ruby: &Ruby, hash: RHash) -> Result<LoraValue, MagnusError> {
    let dimension = read_i64(ruby, hash, "dimension")?
        .ok_or_else(|| invalid_params(ruby, "vector.dimension required"))?;
    let coordinate_type_name = read_string(ruby, hash, "coordinateType")?
        .ok_or_else(|| invalid_params(ruby, "vector.coordinateType required"))?;
    let coordinate_type = VectorCoordinateType::parse(&coordinate_type_name).ok_or_else(|| {
        invalid_params(
            ruby,
            format!("unknown vector coordinate type '{coordinate_type_name}'"),
        )
    })?;
    let values_value = hash_get_either(ruby, hash, "values")
        .ok_or_else(|| invalid_params(ruby, "vector.values required"))?;
    let arr = RArray::try_convert(values_value)
        .map_err(|_| invalid_params(ruby, "vector.values must be an Array"))?;

    let mut raw = Vec::with_capacity(arr.len());
    for item in arr.into_iter() {
        if item.is_kind_of(ruby.class_true_class()) || item.is_kind_of(ruby.class_false_class()) {
            return Err(invalid_params(
                ruby,
                "vector.values entries must be numeric",
            ));
        }
        if let Ok(f) = Float::try_convert(item) {
            let v = f.to_f64();
            if !v.is_finite() {
                return Err(invalid_params(
                    ruby,
                    "vector.values cannot be NaN or Infinity",
                ));
            }
            raw.push(RawCoordinate::Float(v));
            continue;
        }
        if let Ok(i) = Integer::try_convert(item) {
            raw.push(RawCoordinate::Int(i.to_i64()?));
            continue;
        }
        return Err(invalid_params(
            ruby,
            "vector.values entries must be numeric",
        ));
    }

    let v = LoraVector::try_new(raw, dimension, coordinate_type)
        .map_err(|e| invalid_params(ruby, e.to_string()))?;
    Ok(LoraValue::Vector(v))
}

fn read_i64(ruby: &Ruby, hash: RHash, key: &str) -> Result<Option<i64>, MagnusError> {
    let Some(v) = hash_get_either(ruby, hash, key) else {
        return Ok(None);
    };
    Ok(Some(Integer::try_convert(v)?.to_i64().map_err(|_| {
        invalid_params(ruby, format!("{key} out of i64 range"))
    })?))
}

// ---- Hash accessors that accept either string or symbol keys ------------

fn hash_get_either(ruby: &Ruby, hash: RHash, key: &str) -> Option<Value> {
    if let Some(v) = hash.get(ruby.str_new(key)) {
        return Some(v);
    }
    hash.get(ruby.to_symbol(key))
}

fn read_string(ruby: &Ruby, hash: RHash, key: &str) -> Result<Option<String>, MagnusError> {
    let Some(v) = hash_get_either(ruby, hash, key) else {
        return Ok(None);
    };
    let s = RString::try_convert(v)?.to_string()?;
    Ok(Some(s))
}

fn read_u32(ruby: &Ruby, hash: RHash, key: &str) -> Result<Option<u32>, MagnusError> {
    let Some(v) = hash_get_either(ruby, hash, key) else {
        return Ok(None);
    };
    let n = Integer::try_convert(v)?.to_i64()?;
    u32::try_from(n)
        .map(Some)
        .map_err(|_| invalid_params(ruby, "srid out of u32 range"))
}

fn read_f64(ruby: &Ruby, hash: RHash, key: &str) -> Result<Option<f64>, MagnusError> {
    let Some(v) = hash_get_either(ruby, hash, key) else {
        return Ok(None);
    };
    // Accept either Float or Integer — `cartesian(1, 2)` passing ints
    // shouldn't force the caller to call `.to_f` first.
    if let Ok(f) = Float::try_convert(v) {
        return Ok(Some(f.to_f64()));
    }
    if let Ok(i) = Integer::try_convert(v) {
        return Ok(Some(i.to_i64()? as f64));
    }
    Ok(None)
}

// ============================================================================
// GVL release
// ============================================================================

/// Run `f` with Ruby's Global VM Lock released.
///
/// Semantics match `rb_thread_call_without_gvl` — other Ruby threads can
/// progress while `f` runs. The closure MUST NOT touch Ruby state (no
/// `Value`s, no allocations into the Ruby heap), which we arrange by
/// keeping all such work on the calling thread. Everything inside
/// `database_execute`'s closure is pure Rust on pre-extracted data, so
/// this is sound.
fn without_gvl<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
    F: Send,
    R: Send,
{
    struct Data<F, R> {
        func: Option<F>,
        result: MaybeUninit<R>,
    }

    unsafe extern "C" fn trampoline<F, R>(data: *mut c_void) -> *mut c_void
    where
        F: FnOnce() -> R,
    {
        let data = &mut *(data as *mut Data<F, R>);
        let f = data
            .func
            .take()
            .expect("without_gvl: closure already taken");
        data.result.write(f());
        std::ptr::null_mut()
    }

    let mut data = Data::<F, R> {
        func: Some(f),
        result: MaybeUninit::uninit(),
    };

    unsafe {
        rb_sys::rb_thread_call_without_gvl(
            Some(trampoline::<F, R>),
            &mut data as *mut _ as *mut c_void,
            // No unblock function — the engine doesn't implement
            // cooperative cancellation, and a forced longjmp out of a
            // mutex-holding section would be worse than waiting.
            None,
            std::ptr::null_mut(),
        );
        data.result.assume_init()
    }
}
