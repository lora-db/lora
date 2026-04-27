//! C ABI for the Lora graph database.
//!
//! This crate is the shared FFI entry point that out-of-tree language
//! bindings (first consumer: `crates/lora-go` via cgo) link against. It
//! mirrors the execution model used by `lora-node`, `lora-wasm`, and
//! `lora-python`:
//!
//! - A `Database` handle wraps `lora_database::Database` (which in turn
//!   owns the `Arc<RwLock<InMemoryGraph>>`).
//! - Queries run via `execute_with_params` with
//!   `ExecuteOptions { format: ResultFormat::RowArrays }`.
//! - Parameters come in as JSON (tagged value model); results are
//!   serialised back out as JSON (`{"columns":[…], "rows":[…]}`), with
//!   nodes, relationships, paths, temporal and spatial values carrying
//!   the same `kind` discriminator as the other bindings.
//!
//! Every exported function is `extern "C"` and wraps its body in
//! [`std::panic::catch_unwind`] so a Rust panic cannot unwind across the
//! FFI boundary. Panics surface as [`LoraStatus::Panic`] with a captured
//! message in the out-error string.
//!
//! ## Ownership rules
//!
//! | Symbol                        | Ownership                                                        |
//! | ----------------------------- | ---------------------------------------------------------------- |
//! | `LoraDatabase *`              | Allocated by `lora_db_new` / `lora_db_new_with_wal`, freed by `lora_db_free`. |
//! | `char *` (out strings)        | Allocated by Rust, freed by the caller via `lora_string_free`.   |
//! | `const char *` (in strings)   | Borrowed; Rust copies what it needs before returning.            |
//!
//! Passing a `LoraDatabase *` to any function after `lora_db_free` is
//! undefined behaviour. Passing a `char *` not previously returned by
//! this crate to `lora_string_free` is also UB.

#![deny(clippy::all)]
// The FFI deliberately uses raw pointers; the `missing_safety_doc` lint
// is satisfied by the crate-level safety contract documented above.
#![allow(clippy::missing_safety_doc)]

use std::collections::BTreeMap;
use std::ffi::{c_char, c_int, CStr, CString};
use std::os::raw::c_uchar;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::{Arc, Mutex};

use lora_database::{
    Database as InnerDatabase, DatabaseOpenOptions, ExecuteOptions, InMemoryGraph, LoraValue,
    QueryResult, ResultFormat, Row, Snapshotable, TransactionMode, WalConfig,
};
use lora_store::{
    LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint, LoraTime,
    LoraVector, RawCoordinate, VectorCoordinateType, VectorValues,
};

// ============================================================================
// Status codes
// ============================================================================

/// Status codes returned by every FFI entry point.
///
/// The numeric values are part of the stable ABI — do not reorder.
#[repr(C)]
pub enum LoraStatus {
    /// The call succeeded. Any out-pointers are populated.
    Ok = 0,
    /// Parse / analyze / execute failure. The out-error string starts
    /// with `LORA_ERROR: `.
    LoraError = 1,
    /// A parameter value could not be mapped to a Lora value. The
    /// out-error string starts with `INVALID_PARAMS: `.
    InvalidParams = 2,
    /// A required pointer argument was null.
    NullPointer = 3,
    /// The provided UTF-8 input was invalid.
    InvalidUtf8 = 4,
    /// Rust panicked inside the FFI. The out-error captures the panic
    /// message when one could be recovered.
    Panic = 5,
}

const LORA_ERROR_PREFIX: &str = "LORA_ERROR";
const INVALID_PARAMS_PREFIX: &str = "INVALID_PARAMS";

// ============================================================================
// Opaque handle
// ============================================================================

/// Opaque database handle. Wraps a single `lora_database::Database<InMemoryGraph>`
/// so execution semantics are identical across bindings.
pub struct LoraDatabase {
    inner: Arc<InnerDatabase<InMemoryGraph>>,
}

impl LoraDatabase {
    fn new() -> Self {
        Self {
            inner: Arc::new(InnerDatabase::in_memory()),
        }
    }

    fn new_with_wal(wal_dir: &str) -> Result<Self, String> {
        let inner =
            InnerDatabase::open_with_wal(WalConfig::enabled(wal_dir)).map_err(|e| e.to_string())?;
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    fn new_named(database_name: &str, database_dir: Option<&str>) -> Result<Self, String> {
        let mut options = DatabaseOpenOptions::default();
        if let Some(database_dir) = database_dir {
            options.database_dir = database_dir.into();
        }
        let inner = InnerDatabase::open_named(database_name, options).map_err(|e| e.to_string())?;
        Ok(Self {
            inner: Arc::new(inner),
        })
    }
}

/// Opaque native row stream handle.
pub struct LoraQueryStream {
    _db: Arc<InnerDatabase<InMemoryGraph>>,
    stream: Mutex<Option<lora_database::QueryStream<'static>>>,
}

// ============================================================================
// Version
// ============================================================================

/// Crate version as a NUL-terminated static string. Safe to call at any
/// time; the returned pointer lives for the process lifetime and must
/// **not** be passed to `lora_string_free`.
#[no_mangle]
pub extern "C" fn lora_version() -> *const c_char {
    // Emit a fresh NUL-terminated &'static [u8] so the returned pointer
    // never collides with the `char*`s we allocate on the heap. A const
    // CStr would be cleaner but requires `c"…"` literals (1.77+); this
    // spelling works on the pinned toolchain.
    static VERSION: &[u8] = concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes();
    VERSION.as_ptr() as *const c_char
}

// ============================================================================
// Constructor / destructor
// ============================================================================

/// Allocate a new in-memory Lora database. Writes a handle into
/// `*out_db` on success. The handle must be freed with `lora_db_free`.
#[no_mangle]
pub unsafe extern "C" fn lora_db_new(out_db: *mut *mut LoraDatabase) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if out_db.is_null() {
            return LoraStatus::NullPointer;
        }
        let db = Box::new(LoraDatabase::new());
        *out_db = Box::into_raw(db);
        LoraStatus::Ok
    }));
    match result {
        Ok(status) => status as c_int,
        Err(_) => LoraStatus::Panic as c_int,
    }
}

/// Allocate a new WAL-backed Lora database rooted at `wal_dir`.
///
/// On success writes a handle into `*out_db`. On failure writes an
/// error string into `*out_error`. The returned handle must be freed
/// with `lora_db_free`.
#[no_mangle]
pub unsafe extern "C" fn lora_db_new_with_wal(
    out_db: *mut *mut LoraDatabase,
    wal_dir: *const c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if !out_error.is_null() {
            *out_error = ptr::null_mut();
        }
        if !out_db.is_null() {
            *out_db = ptr::null_mut();
        }
        if out_db.is_null() || wal_dir.is_null() {
            return LoraStatus::NullPointer;
        }

        let wal_dir = match CStr::from_ptr(wal_dir).to_str() {
            Ok(s) => s,
            Err(_) => {
                write_error(
                    out_error,
                    LORA_ERROR_PREFIX,
                    "wal directory is not valid UTF-8",
                );
                return LoraStatus::InvalidUtf8;
            }
        };

        match LoraDatabase::new_with_wal(wal_dir) {
            Ok(db) => {
                *out_db = Box::into_raw(Box::new(db));
                LoraStatus::Ok
            }
            Err(e) => {
                write_error(out_error, LORA_ERROR_PREFIX, &e);
                LoraStatus::LoraError
            }
        }
    }));
    match result {
        Ok(status) => status as c_int,
        Err(panic) => {
            if !out_error.is_null() {
                let msg = panic_message(panic);
                write_error(out_error, LORA_ERROR_PREFIX, &msg);
            }
            LoraStatus::Panic as c_int
        }
    }
}

/// Allocate a new named WAL-backed Lora database.
///
/// `database_name` is a logical name, not a path. It must contain only
/// letters, digits, `_`, `-`, and `.`. `database_dir` may be null to use the
/// current working directory. On success the WAL root is
/// `<database_dir>/<database_name>.lora`.
#[no_mangle]
pub unsafe extern "C" fn lora_db_new_named(
    out_db: *mut *mut LoraDatabase,
    database_name: *const c_char,
    database_dir: *const c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if !out_error.is_null() {
            *out_error = ptr::null_mut();
        }
        if !out_db.is_null() {
            *out_db = ptr::null_mut();
        }
        if out_db.is_null() || database_name.is_null() {
            return LoraStatus::NullPointer;
        }

        let database_name = match CStr::from_ptr(database_name).to_str() {
            Ok(s) => s,
            Err(_) => {
                write_error(
                    out_error,
                    LORA_ERROR_PREFIX,
                    "database name is not valid UTF-8",
                );
                return LoraStatus::InvalidUtf8;
            }
        };
        let database_dir = if database_dir.is_null() {
            None
        } else {
            match CStr::from_ptr(database_dir).to_str() {
                Ok(s) => Some(s),
                Err(_) => {
                    write_error(
                        out_error,
                        LORA_ERROR_PREFIX,
                        "database directory is not valid UTF-8",
                    );
                    return LoraStatus::InvalidUtf8;
                }
            }
        };

        match LoraDatabase::new_named(database_name, database_dir) {
            Ok(db) => {
                *out_db = Box::into_raw(Box::new(db));
                LoraStatus::Ok
            }
            Err(e) => {
                write_error(out_error, LORA_ERROR_PREFIX, &e);
                LoraStatus::LoraError
            }
        }
    }));
    match result {
        Ok(status) => status as c_int,
        Err(panic) => {
            if !out_error.is_null() {
                let msg = panic_message(panic);
                write_error(out_error, LORA_ERROR_PREFIX, &msg);
            }
            LoraStatus::Panic as c_int
        }
    }
}

/// Free a database handle previously returned by `lora_db_new`. Passing
/// a null pointer is a no-op. Passing anything else is undefined.
#[no_mangle]
pub unsafe extern "C" fn lora_db_free(db: *mut LoraDatabase) {
    if db.is_null() {
        return;
    }
    // `Box::from_raw` + drop. Wrapped in `catch_unwind` defensively so a
    // panicking `Drop` on the inner store cannot propagate into the
    // caller.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        drop(Box::from_raw(db));
    }));
}

// ============================================================================
// Execute
// ============================================================================

/// Execute a Lora query with optional JSON-encoded parameters.
///
/// `params_json` may be null / empty / `"null"` for the no-params case.
/// On success the result JSON (`{"columns": […], "rows": […]}`) is
/// written to `*out_result`; on failure an error string is written to
/// `*out_error`. Both are heap-allocated by Rust and must be released
/// with `lora_string_free`.
///
/// Exactly one of `*out_result` / `*out_error` is populated on return,
/// matching the returned status.
#[no_mangle]
pub unsafe extern "C" fn lora_db_execute_json(
    db: *mut LoraDatabase,
    query: *const c_char,
    params_json: *const c_char,
    out_result: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // Zero both out-pointers up front so a caller who forgets to
        // check the status doesn't read uninitialised memory.
        if !out_result.is_null() {
            *out_result = ptr::null_mut();
        }
        if !out_error.is_null() {
            *out_error = ptr::null_mut();
        }
        if db.is_null() || query.is_null() || out_result.is_null() || out_error.is_null() {
            return LoraStatus::NullPointer;
        }

        let query = match CStr::from_ptr(query).to_str() {
            Ok(s) => s,
            Err(_) => {
                write_error(out_error, LORA_ERROR_PREFIX, "query is not valid UTF-8");
                return LoraStatus::InvalidUtf8;
            }
        };

        let params_str = if params_json.is_null() {
            None
        } else {
            match CStr::from_ptr(params_json).to_str() {
                Ok("") => None,
                Ok(s) => Some(s),
                Err(_) => {
                    write_error(
                        out_error,
                        INVALID_PARAMS_PREFIX,
                        "params JSON is not valid UTF-8",
                    );
                    return LoraStatus::InvalidParams;
                }
            }
        };

        let params_map = match parse_params(params_str) {
            Ok(map) => map,
            Err(msg) => {
                write_error(out_error, INVALID_PARAMS_PREFIX, &msg);
                return LoraStatus::InvalidParams;
            }
        };

        match execute_json_payload(&(*db).inner, query, params_map) {
            Ok(json) => *out_result = to_c_string(json),
            Err(msg) => {
                write_error(out_error, LORA_ERROR_PREFIX, &msg);
                return LoraStatus::LoraError;
            }
        }
        LoraStatus::Ok
    }));
    match result {
        Ok(status) => status as c_int,
        Err(panic) => {
            if !out_error.is_null() {
                let msg = panic_message(panic);
                write_error(out_error, LORA_ERROR_PREFIX, &msg);
            }
            LoraStatus::Panic as c_int
        }
    }
}

/// Execute a JSON-encoded statement array inside one native transaction.
///
/// `statements_json` must be an array of `{ "query": string, "params"?: object }`.
/// `mode` may be null for read-write, or one of read_write/read_only/rw/ro.
/// On success `*out_result` receives a JSON array of normal query result
/// envelopes in statement order.
#[no_mangle]
pub unsafe extern "C" fn lora_db_transaction_json(
    db: *mut LoraDatabase,
    statements_json: *const c_char,
    mode: *const c_char,
    out_result: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if !out_result.is_null() {
            *out_result = ptr::null_mut();
        }
        if !out_error.is_null() {
            *out_error = ptr::null_mut();
        }
        if db.is_null() || statements_json.is_null() || out_result.is_null() || out_error.is_null()
        {
            return LoraStatus::NullPointer;
        }

        let statements_raw = match CStr::from_ptr(statements_json).to_str() {
            Ok(s) => s,
            Err(_) => {
                write_error(
                    out_error,
                    INVALID_PARAMS_PREFIX,
                    "statements JSON is not valid UTF-8",
                );
                return LoraStatus::InvalidUtf8;
            }
        };
        let mode_raw = if mode.is_null() {
            None
        } else {
            match CStr::from_ptr(mode).to_str() {
                Ok(s) => Some(s),
                Err(_) => {
                    write_error(out_error, INVALID_PARAMS_PREFIX, "mode is not valid UTF-8");
                    return LoraStatus::InvalidUtf8;
                }
            }
        };

        let mode = match parse_transaction_mode(mode_raw) {
            Ok(mode) => mode,
            Err(msg) => {
                write_error(out_error, INVALID_PARAMS_PREFIX, &msg);
                return LoraStatus::InvalidParams;
            }
        };
        let statements_value = match serde_json::from_str(statements_raw) {
            Ok(value) => value,
            Err(e) => {
                write_error(out_error, INVALID_PARAMS_PREFIX, &format!("{e}"));
                return LoraStatus::InvalidParams;
            }
        };
        let statements = match parse_transaction_statements(statements_value) {
            Ok(statements) => statements,
            Err(msg) => {
                write_error(out_error, INVALID_PARAMS_PREFIX, &msg);
                return LoraStatus::InvalidParams;
            }
        };

        let mut tx = match (*db).inner.begin_transaction(mode) {
            Ok(tx) => tx,
            Err(e) => {
                write_error(out_error, LORA_ERROR_PREFIX, &format!("{e}"));
                return LoraStatus::LoraError;
            }
        };
        let mut results = Vec::with_capacity(statements.len());
        for statement in statements {
            let options = ExecuteOptions {
                format: ResultFormat::RowArrays,
            };
            let exec = tx.execute_with_params(&statement.query, Some(options), statement.params);
            let row_arrays = match exec {
                Ok(QueryResult::RowArrays(r)) => r,
                Ok(_) => {
                    write_error(out_error, LORA_ERROR_PREFIX, "expected RowArrays result");
                    return LoraStatus::LoraError;
                }
                Err(e) => {
                    write_error(out_error, LORA_ERROR_PREFIX, &format!("{e}"));
                    return LoraStatus::LoraError;
                }
            };
            results.push(serialize_rows(&row_arrays.columns, &row_arrays.rows));
        }
        if let Err(e) = tx.commit() {
            write_error(out_error, LORA_ERROR_PREFIX, &format!("{e}"));
            return LoraStatus::LoraError;
        }

        let json = match serde_json::to_string(&serde_json::Value::Array(results)) {
            Ok(s) => s,
            Err(e) => {
                write_error(out_error, LORA_ERROR_PREFIX, &format!("{e}"));
                return LoraStatus::LoraError;
            }
        };
        *out_result = to_c_string(json);
        LoraStatus::Ok
    }));
    match result {
        Ok(status) => status as c_int,
        Err(panic) => {
            if !out_error.is_null() {
                let msg = panic_message(panic);
                write_error(out_error, LORA_ERROR_PREFIX, &msg);
            }
            LoraStatus::Panic as c_int
        }
    }
}

/// Open a true native row stream for `query`.
///
/// On success writes a stream handle into `*out_stream`. The handle owns the
/// Rust cursor and must be freed with `lora_stream_free`.
#[no_mangle]
pub unsafe extern "C" fn lora_db_stream_open_json(
    db: *mut LoraDatabase,
    query: *const c_char,
    params_json: *const c_char,
    out_stream: *mut *mut LoraQueryStream,
    out_error: *mut *mut c_char,
) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if !out_stream.is_null() {
            *out_stream = ptr::null_mut();
        }
        if !out_error.is_null() {
            *out_error = ptr::null_mut();
        }
        if db.is_null() || query.is_null() || out_stream.is_null() || out_error.is_null() {
            return LoraStatus::NullPointer;
        }

        let query = match CStr::from_ptr(query).to_str() {
            Ok(s) => s,
            Err(_) => {
                write_error(out_error, LORA_ERROR_PREFIX, "query is not valid UTF-8");
                return LoraStatus::InvalidUtf8;
            }
        };
        let params_str = if params_json.is_null() {
            None
        } else {
            match CStr::from_ptr(params_json).to_str() {
                Ok("") => None,
                Ok(s) => Some(s),
                Err(_) => {
                    write_error(
                        out_error,
                        INVALID_PARAMS_PREFIX,
                        "params JSON is not valid UTF-8",
                    );
                    return LoraStatus::InvalidParams;
                }
            }
        };
        let params_map = match parse_params(params_str) {
            Ok(map) => map,
            Err(msg) => {
                write_error(out_error, INVALID_PARAMS_PREFIX, &msg);
                return LoraStatus::InvalidParams;
            }
        };

        let inner = (*db).inner.clone();
        let stream = match unsafe { inner.stream_with_params_owned(query, params_map) } {
            Ok(stream) => stream,
            Err(e) => {
                write_error(out_error, LORA_ERROR_PREFIX, &format!("{e}"));
                return LoraStatus::LoraError;
            }
        };
        *out_stream = Box::into_raw(Box::new(LoraQueryStream {
            _db: inner,
            stream: Mutex::new(Some(stream)),
        }));
        LoraStatus::Ok
    }));
    match result {
        Ok(status) => status as c_int,
        Err(panic) => {
            if !out_error.is_null() {
                let msg = panic_message(panic);
                write_error(out_error, LORA_ERROR_PREFIX, &msg);
            }
            LoraStatus::Panic as c_int
        }
    }
}

/// Return the stream's column names as a JSON array.
#[no_mangle]
pub unsafe extern "C" fn lora_stream_columns_json(
    stream: *mut LoraQueryStream,
    out_result: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if !out_result.is_null() {
            *out_result = ptr::null_mut();
        }
        if !out_error.is_null() {
            *out_error = ptr::null_mut();
        }
        if stream.is_null() || out_result.is_null() || out_error.is_null() {
            return LoraStatus::NullPointer;
        }
        let guard = match (*stream).stream.lock() {
            Ok(guard) => guard,
            Err(_) => {
                write_error(out_error, LORA_ERROR_PREFIX, "stream lock poisoned");
                return LoraStatus::LoraError;
            }
        };
        let Some(stream) = guard.as_ref() else {
            write_error(out_error, LORA_ERROR_PREFIX, "query stream is closed");
            return LoraStatus::LoraError;
        };
        let json = match serde_json::to_string(stream.columns()) {
            Ok(json) => json,
            Err(e) => {
                write_error(out_error, LORA_ERROR_PREFIX, &format!("{e}"));
                return LoraStatus::LoraError;
            }
        };
        *out_result = to_c_string(json);
        LoraStatus::Ok
    }));
    match result {
        Ok(status) => status as c_int,
        Err(panic) => {
            if !out_error.is_null() {
                let msg = panic_message(panic);
                write_error(out_error, LORA_ERROR_PREFIX, &msg);
            }
            LoraStatus::Panic as c_int
        }
    }
}

/// Pull one row from a stream as a JSON object.
///
/// On end-of-stream this returns OK with `*out_result == NULL`.
#[no_mangle]
pub unsafe extern "C" fn lora_stream_next_json(
    stream: *mut LoraQueryStream,
    out_result: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if !out_result.is_null() {
            *out_result = ptr::null_mut();
        }
        if !out_error.is_null() {
            *out_error = ptr::null_mut();
        }
        if stream.is_null() || out_result.is_null() || out_error.is_null() {
            return LoraStatus::NullPointer;
        }
        let mut guard = match (*stream).stream.lock() {
            Ok(guard) => guard,
            Err(_) => {
                write_error(out_error, LORA_ERROR_PREFIX, "stream lock poisoned");
                return LoraStatus::LoraError;
            }
        };
        let Some(stream) = guard.as_mut() else {
            return LoraStatus::Ok;
        };
        match stream.next_row() {
            Ok(Some(row)) => {
                let json = match serde_json::to_string(&row_to_json(&row)) {
                    Ok(json) => json,
                    Err(e) => {
                        write_error(out_error, LORA_ERROR_PREFIX, &format!("{e}"));
                        return LoraStatus::LoraError;
                    }
                };
                *out_result = to_c_string(json);
                LoraStatus::Ok
            }
            Ok(None) => {
                guard.take();
                LoraStatus::Ok
            }
            Err(e) => {
                guard.take();
                write_error(out_error, LORA_ERROR_PREFIX, &format!("{e}"));
                LoraStatus::LoraError
            }
        }
    }));
    match result {
        Ok(status) => status as c_int,
        Err(panic) => {
            if !out_error.is_null() {
                let msg = panic_message(panic);
                write_error(out_error, LORA_ERROR_PREFIX, &msg);
            }
            LoraStatus::Panic as c_int
        }
    }
}

/// Free a stream handle. A non-exhausted mutating stream rolls back on drop.
#[no_mangle]
pub unsafe extern "C" fn lora_stream_free(stream: *mut LoraQueryStream) {
    if stream.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        drop(Box::from_raw(stream));
    }));
}

// ============================================================================
// Clear / counts
// ============================================================================

/// Drop every node and relationship in the database. Constant-time.
#[no_mangle]
pub unsafe extern "C" fn lora_db_clear(db: *mut LoraDatabase) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if db.is_null() {
            return LoraStatus::NullPointer;
        }
        (*db).inner.clear();
        LoraStatus::Ok
    }));
    match result {
        Ok(status) => status as c_int,
        Err(_) => LoraStatus::Panic as c_int,
    }
}

/// Write the current node count into `*out`.
#[no_mangle]
pub unsafe extern "C" fn lora_db_node_count(db: *mut LoraDatabase, out: *mut u64) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if db.is_null() || out.is_null() {
            return LoraStatus::NullPointer;
        }
        *out = (*db).inner.node_count() as u64;
        LoraStatus::Ok
    }));
    match result {
        Ok(status) => status as c_int,
        Err(_) => LoraStatus::Panic as c_int,
    }
}

/// Write the current relationship count into `*out`.
#[no_mangle]
pub unsafe extern "C" fn lora_db_relationship_count(db: *mut LoraDatabase, out: *mut u64) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if db.is_null() || out.is_null() {
            return LoraStatus::NullPointer;
        }
        *out = (*db).inner.relationship_count() as u64;
        LoraStatus::Ok
    }));
    match result {
        Ok(status) => status as c_int,
        Err(_) => LoraStatus::Panic as c_int,
    }
}

// ============================================================================
// Snapshots
// ============================================================================

/// Snapshot metadata. The FFI returns this by value from `lora_db_save_snapshot`
/// and `lora_db_load_snapshot`; callers treat it as plain data.
///
/// `wal_lsn_set` is `1` iff the snapshot carries a meaningful WAL LSN; when
/// `0`, the `wal_lsn` field is zero and should be ignored. Pure snapshots
/// write `wal_lsn_set = 0`; checkpoint snapshots write a fence value.
#[repr(C)]
pub struct LoraSnapshotMeta {
    pub format_version: u32,
    pub wal_lsn_set: u32,
    pub node_count: u64,
    pub relationship_count: u64,
    pub wal_lsn: u64,
}

impl LoraSnapshotMeta {
    fn from_meta(meta: lora_database::SnapshotMeta) -> Self {
        Self {
            format_version: meta.format_version,
            wal_lsn_set: if meta.wal_lsn.is_some() { 1 } else { 0 },
            node_count: meta.node_count as u64,
            relationship_count: meta.relationship_count as u64,
            wal_lsn: meta.wal_lsn.unwrap_or(0),
        }
    }

    fn zeroed() -> Self {
        Self {
            format_version: 0,
            wal_lsn_set: 0,
            node_count: 0,
            relationship_count: 0,
            wal_lsn: 0,
        }
    }
}

/// Save a snapshot to `path`. Atomic: the target is only replaced once the
/// full payload has been written + fsync'd.
///
/// On success, `*out_meta` is populated and the return value is `Ok`. On
/// failure, `*out_error` is set and `*out_meta` is left zeroed.
#[no_mangle]
pub unsafe extern "C" fn lora_db_save_snapshot(
    db: *mut LoraDatabase,
    path: *const c_char,
    out_meta: *mut LoraSnapshotMeta,
    out_error: *mut *mut c_char,
) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if db.is_null() || path.is_null() || out_meta.is_null() {
            return LoraStatus::NullPointer;
        }
        *out_meta = LoraSnapshotMeta::zeroed();

        let path_str = match CStr::from_ptr(path).to_str() {
            Ok(s) => s,
            Err(_) => {
                write_error(
                    out_error,
                    LORA_ERROR_PREFIX,
                    "snapshot path is not valid UTF-8",
                );
                return LoraStatus::InvalidUtf8;
            }
        };

        match (*db).inner.save_snapshot_to(path_str) {
            Ok(meta) => {
                *out_meta = LoraSnapshotMeta::from_meta(meta);
                LoraStatus::Ok
            }
            Err(e) => {
                write_error(out_error, LORA_ERROR_PREFIX, &format!("{e}"));
                LoraStatus::LoraError
            }
        }
    }));
    match result {
        Ok(status) => status as c_int,
        Err(panic) => {
            if !out_error.is_null() {
                let msg = panic_message(panic);
                write_error(out_error, LORA_ERROR_PREFIX, &msg);
            }
            LoraStatus::Panic as c_int
        }
    }
}

/// Serialize the current graph into snapshot bytes. The caller owns
/// `*out_bytes` on success and must release it with `lora_bytes_free`.
#[no_mangle]
pub unsafe extern "C" fn lora_db_save_snapshot_to_bytes(
    db: *mut LoraDatabase,
    out_bytes: *mut *mut c_uchar,
    out_len: *mut usize,
    out_meta: *mut LoraSnapshotMeta,
    out_error: *mut *mut c_char,
) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if db.is_null() || out_bytes.is_null() || out_len.is_null() || out_meta.is_null() {
            return LoraStatus::NullPointer;
        }
        *out_bytes = ptr::null_mut();
        *out_len = 0;
        *out_meta = LoraSnapshotMeta::zeroed();

        let mut bytes = Vec::new();
        match (*db)
            .inner
            .with_store(|store| store.save_snapshot(&mut bytes))
        {
            Ok(meta) => {
                let mut boxed = bytes.into_boxed_slice();
                *out_len = boxed.len();
                *out_bytes = boxed.as_mut_ptr();
                std::mem::forget(boxed);
                *out_meta = LoraSnapshotMeta::from_meta(meta);
                LoraStatus::Ok
            }
            Err(e) => {
                write_error(out_error, LORA_ERROR_PREFIX, &format!("{e}"));
                LoraStatus::LoraError
            }
        }
    }));
    match result {
        Ok(status) => status as c_int,
        Err(panic) => {
            if !out_error.is_null() {
                let msg = panic_message(panic);
                write_error(out_error, LORA_ERROR_PREFIX, &msg);
            }
            LoraStatus::Panic as c_int
        }
    }
}

/// Load a snapshot from `path` and replace the current graph state.
#[no_mangle]
pub unsafe extern "C" fn lora_db_load_snapshot(
    db: *mut LoraDatabase,
    path: *const c_char,
    out_meta: *mut LoraSnapshotMeta,
    out_error: *mut *mut c_char,
) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if db.is_null() || path.is_null() || out_meta.is_null() {
            return LoraStatus::NullPointer;
        }
        *out_meta = LoraSnapshotMeta::zeroed();

        let path_str = match CStr::from_ptr(path).to_str() {
            Ok(s) => s,
            Err(_) => {
                write_error(
                    out_error,
                    LORA_ERROR_PREFIX,
                    "snapshot path is not valid UTF-8",
                );
                return LoraStatus::InvalidUtf8;
            }
        };

        match (*db).inner.load_snapshot_from(path_str) {
            Ok(meta) => {
                *out_meta = LoraSnapshotMeta::from_meta(meta);
                LoraStatus::Ok
            }
            Err(e) => {
                write_error(out_error, LORA_ERROR_PREFIX, &format!("{e}"));
                LoraStatus::LoraError
            }
        }
    }));
    match result {
        Ok(status) => status as c_int,
        Err(panic) => {
            if !out_error.is_null() {
                let msg = panic_message(panic);
                write_error(out_error, LORA_ERROR_PREFIX, &msg);
            }
            LoraStatus::Panic as c_int
        }
    }
}

/// Load a snapshot from borrowed bytes and replace the current graph state.
#[no_mangle]
pub unsafe extern "C" fn lora_db_load_snapshot_from_bytes(
    db: *mut LoraDatabase,
    bytes: *const c_uchar,
    len: usize,
    out_meta: *mut LoraSnapshotMeta,
    out_error: *mut *mut c_char,
) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if db.is_null() || bytes.is_null() || out_meta.is_null() {
            return LoraStatus::NullPointer;
        }
        *out_meta = LoraSnapshotMeta::zeroed();

        let bytes = std::slice::from_raw_parts(bytes, len);
        match (*db)
            .inner
            .with_store_mut(|store| store.load_snapshot(bytes))
        {
            Ok(meta) => {
                *out_meta = LoraSnapshotMeta::from_meta(meta);
                LoraStatus::Ok
            }
            Err(e) => {
                write_error(out_error, LORA_ERROR_PREFIX, &format!("{e}"));
                LoraStatus::LoraError
            }
        }
    }));
    match result {
        Ok(status) => status as c_int,
        Err(panic) => {
            if !out_error.is_null() {
                let msg = panic_message(panic);
                write_error(out_error, LORA_ERROR_PREFIX, &msg);
            }
            LoraStatus::Panic as c_int
        }
    }
}

// ============================================================================
// String release
// ============================================================================

/// Free a byte buffer returned by `lora_db_save_snapshot_to_bytes`.
#[no_mangle]
pub unsafe extern "C" fn lora_bytes_free(bytes: *mut c_uchar, len: usize) {
    if bytes.is_null() {
        return;
    }
    drop(Vec::from_raw_parts(bytes, len, len));
}

/// Free a `char*` previously returned by one of the `*_out_*` parameters.
/// Passing null is a no-op. Passing anything not returned by this crate
/// is undefined.
#[no_mangle]
pub unsafe extern "C" fn lora_string_free(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        drop(CString::from_raw(s));
    }));
}

// ============================================================================
// JSON value model (shared with node / wasm / python)
// ============================================================================

fn parse_params(raw: Option<&str>) -> Result<BTreeMap<String, LoraValue>, String> {
    let Some(s) = raw else {
        return Ok(BTreeMap::new());
    };
    let value: serde_json::Value = serde_json::from_str(s).map_err(|e| format!("{e}"))?;
    json_value_to_params(value)
}

struct TransactionStatement {
    query: String,
    params: BTreeMap<String, LoraValue>,
}

fn parse_transaction_mode(mode: Option<&str>) -> Result<TransactionMode, String> {
    match mode.unwrap_or("read_write") {
        "read_write" | "readwrite" | "rw" => Ok(TransactionMode::ReadWrite),
        "read_only" | "readonly" | "ro" => Ok(TransactionMode::ReadOnly),
        other => Err(format!("unknown transaction mode '{other}'")),
    }
}

fn parse_transaction_statements(
    value: serde_json::Value,
) -> Result<Vec<TransactionStatement>, String> {
    let serde_json::Value::Array(items) = value else {
        return Err("transaction statements must be an array".to_string());
    };

    items
        .into_iter()
        .map(|item| {
            let serde_json::Value::Object(mut obj) = item else {
                return Err("transaction statement must be an object".to_string());
            };
            let query = match obj.remove("query") {
                Some(serde_json::Value::String(query)) => query,
                _ => return Err("transaction statement requires query: string".to_string()),
            };
            let params = match obj.remove("params") {
                None | Some(serde_json::Value::Null) => BTreeMap::new(),
                Some(other) => json_value_to_params(other)?,
            };
            Ok(TransactionStatement { query, params })
        })
        .collect()
}

fn execute_json_payload(
    inner: &InnerDatabase<InMemoryGraph>,
    query: &str,
    params_map: BTreeMap<String, LoraValue>,
) -> Result<String, String> {
    let options = ExecuteOptions {
        format: ResultFormat::RowArrays,
    };
    let exec = inner.execute_with_params(query, Some(options), params_map);

    let row_arrays = match exec {
        Ok(QueryResult::RowArrays(r)) => r,
        Ok(_) => return Err("expected RowArrays result".to_string()),
        Err(e) => return Err(format!("{e}")),
    };

    let payload = serialize_rows(&row_arrays.columns, &row_arrays.rows);
    serde_json::to_string(&payload).map_err(|e| format!("{e}"))
}

fn json_value_to_params(value: serde_json::Value) -> Result<BTreeMap<String, LoraValue>, String> {
    match value {
        serde_json::Value::Null => Ok(BTreeMap::new()),
        serde_json::Value::Object(obj) => {
            let mut map = BTreeMap::new();
            for (k, v) in obj {
                map.insert(k, json_value_to_cypher(v)?);
            }
            Ok(map)
        }
        _ => Err("params must be an object keyed by parameter name".to_string()),
    }
}

fn json_value_to_cypher(value: serde_json::Value) -> Result<LoraValue, String> {
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
                Err("unsupported numeric value".to_string())
            }
        }
        J::String(s) => Ok(LoraValue::String(s)),
        J::Array(items) => {
            let list = items
                .into_iter()
                .map(json_value_to_cypher)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(LoraValue::List(list))
        }
        J::Object(obj) => {
            if let Some(serde_json::Value::String(kind)) = obj.get("kind") {
                match kind.as_str() {
                    "date" => {
                        let iso = require_iso(&obj, "date")?;
                        let d = LoraDate::parse(iso).map_err(|e| e.to_string())?;
                        return Ok(LoraValue::Date(d));
                    }
                    "time" => {
                        let iso = require_iso(&obj, "time")?;
                        let t = LoraTime::parse(iso).map_err(|e| e.to_string())?;
                        return Ok(LoraValue::Time(t));
                    }
                    "localtime" => {
                        let iso = require_iso(&obj, "localtime")?;
                        let t = LoraLocalTime::parse(iso).map_err(|e| e.to_string())?;
                        return Ok(LoraValue::LocalTime(t));
                    }
                    "datetime" => {
                        let iso = require_iso(&obj, "datetime")?;
                        let dt = LoraDateTime::parse(iso).map_err(|e| e.to_string())?;
                        return Ok(LoraValue::DateTime(dt));
                    }
                    "localdatetime" => {
                        let iso = require_iso(&obj, "localdatetime")?;
                        let dt = LoraLocalDateTime::parse(iso).map_err(|e| e.to_string())?;
                        return Ok(LoraValue::LocalDateTime(dt));
                    }
                    "duration" => {
                        let iso = require_iso(&obj, "duration")?;
                        let d = LoraDuration::parse(iso).map_err(|e| e.to_string())?;
                        return Ok(LoraValue::Duration(d));
                    }
                    "point" => {
                        let srid = obj.get("srid").and_then(|v| v.as_u64()).unwrap_or(7203) as u32;
                        let x = obj
                            .get("x")
                            .and_then(|v| v.as_f64())
                            .ok_or_else(|| "point.x must be a number".to_string())?;
                        let y = obj
                            .get("y")
                            .and_then(|v| v.as_f64())
                            .ok_or_else(|| "point.y must be a number".to_string())?;
                        let z = obj.get("z").and_then(|v| v.as_f64());
                        return Ok(LoraValue::Point(LoraPoint { x, y, z, srid }));
                    }
                    "vector" => {
                        return vector_from_json_map(&obj).map(LoraValue::Vector);
                    }
                    _ => { /* fall through to generic map */ }
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

fn require_iso<'a>(
    obj: &'a serde_json::Map<String, serde_json::Value>,
    tag: &str,
) -> Result<&'a str, String> {
    obj.get("iso")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{tag} value requires iso: string"))
}

fn vector_from_json_map(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<LoraVector, String> {
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

fn serialize_rows(columns: &[String], rows: &[Vec<LoraValue>]) -> serde_json::Value {
    let cols_json = columns
        .iter()
        .map(|c| serde_json::Value::String(c.clone()))
        .collect::<Vec<_>>();

    let rows_json = rows
        .iter()
        .map(|row| {
            let mut obj = serde_json::Map::with_capacity(columns.len());
            for (col, val) in columns.iter().zip(row.iter()) {
                obj.insert(col.clone(), lora_value_to_json(val));
            }
            serde_json::Value::Object(obj)
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "columns": serde_json::Value::Array(cols_json),
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

fn vector_to_json(v: &LoraVector) -> serde_json::Value {
    let values: serde_json::Value = match &v.values {
        VectorValues::Float64(vs) => serde_json::Value::Array(
            vs.iter()
                .map(|x| {
                    serde_json::Number::from_f64(*x)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null)
                })
                .collect(),
        ),
        VectorValues::Float32(vs) => serde_json::Value::Array(
            vs.iter()
                .map(|x| {
                    serde_json::Number::from_f64(*x as f64)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null)
                })
                .collect(),
        ),
        VectorValues::Integer64(vs) => {
            serde_json::Value::Array(vs.iter().map(|x| serde_json::json!(*x)).collect())
        }
        VectorValues::Integer32(vs) => {
            serde_json::Value::Array(vs.iter().map(|x| serde_json::json!(*x as i64)).collect())
        }
        VectorValues::Integer16(vs) => {
            serde_json::Value::Array(vs.iter().map(|x| serde_json::json!(*x as i64)).collect())
        }
        VectorValues::Integer8(vs) => {
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

// ============================================================================
// C string helpers
// ============================================================================

fn to_c_string(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        // `CString::new` only fails when the string contains an interior
        // NUL byte. Serialised JSON never does, so this is unreachable in
        // practice; returning null keeps the ABI simple for the caller.
        Err(_) => ptr::null_mut(),
    }
}

unsafe fn write_error(out_error: *mut *mut c_char, prefix: &str, message: &str) {
    if out_error.is_null() {
        return;
    }
    let full = format!("{prefix}: {message}");
    let ptr = to_c_string(full);
    *out_error = ptr;
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<String>() {
        format!("panic: {s}")
    } else if let Some(s) = panic.downcast_ref::<&'static str>() {
        format!("panic: {s}")
    } else {
        "panic: (unrecoverable message)".to_string()
    }
}

// ============================================================================
// In-process tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn new_db() -> *mut LoraDatabase {
        let mut db: *mut LoraDatabase = ptr::null_mut();
        let s = unsafe { lora_db_new(&mut db) };
        assert_eq!(s, LoraStatus::Ok as c_int);
        assert!(!db.is_null());
        db
    }

    fn tempdir(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "lora-ffi-test-{}-{}-{}",
            tag,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn new_db_with_wal(wal_dir: &Path) -> *mut LoraDatabase {
        let mut db: *mut LoraDatabase = ptr::null_mut();
        let wal_dir = CString::new(wal_dir.to_str().unwrap()).unwrap();
        let mut out_error: *mut c_char = ptr::null_mut();
        let s = unsafe { lora_db_new_with_wal(&mut db, wal_dir.as_ptr(), &mut out_error) };
        if !out_error.is_null() {
            let e = unsafe { CStr::from_ptr(out_error).to_str().unwrap().to_owned() };
            unsafe { lora_string_free(out_error) };
            panic!("lora_db_new_with_wal failed: status={s} err={e}");
        }
        assert_eq!(s, LoraStatus::Ok as c_int);
        assert!(!db.is_null());
        db
    }

    unsafe fn exec(
        db: *mut LoraDatabase,
        q: &str,
        p: Option<&str>,
    ) -> (c_int, Option<String>, Option<String>) {
        let qc = CString::new(q).unwrap();
        let pc = p.map(|s| CString::new(s).unwrap());
        let pc_ptr = pc.as_ref().map_or(ptr::null(), |c| c.as_ptr());
        let mut out_result: *mut c_char = ptr::null_mut();
        let mut out_error: *mut c_char = ptr::null_mut();
        let s = lora_db_execute_json(db, qc.as_ptr(), pc_ptr, &mut out_result, &mut out_error);
        let result = if out_result.is_null() {
            None
        } else {
            let r = CStr::from_ptr(out_result).to_str().unwrap().to_owned();
            lora_string_free(out_result);
            Some(r)
        };
        let error = if out_error.is_null() {
            None
        } else {
            let e = CStr::from_ptr(out_error).to_str().unwrap().to_owned();
            lora_string_free(out_error);
            Some(e)
        };
        (s, result, error)
    }

    unsafe fn tx(
        db: *mut LoraDatabase,
        statements: &str,
        mode: Option<&str>,
    ) -> (c_int, Option<String>, Option<String>) {
        let sc = CString::new(statements).unwrap();
        let mc = mode.map(|s| CString::new(s).unwrap());
        let mc_ptr = mc.as_ref().map_or(ptr::null(), |c| c.as_ptr());
        let mut out_result: *mut c_char = ptr::null_mut();
        let mut out_error: *mut c_char = ptr::null_mut();
        let s = lora_db_transaction_json(db, sc.as_ptr(), mc_ptr, &mut out_result, &mut out_error);
        let result = if out_result.is_null() {
            None
        } else {
            let r = CStr::from_ptr(out_result).to_str().unwrap().to_owned();
            lora_string_free(out_result);
            Some(r)
        };
        let error = if out_error.is_null() {
            None
        } else {
            let e = CStr::from_ptr(out_error).to_str().unwrap().to_owned();
            lora_string_free(out_error);
            Some(e)
        };
        (s, result, error)
    }

    #[test]
    fn version_is_crate_version() {
        let v = unsafe { CStr::from_ptr(lora_version()).to_str().unwrap() };
        assert_eq!(v, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn new_and_free_roundtrip() {
        let db = new_db();
        unsafe { lora_db_free(db) };
    }

    #[test]
    fn wal_constructor_replays_committed_writes() {
        let dir = tempdir("wal-replay");
        let db = new_db_with_wal(&dir);
        let (s, _, e) = unsafe {
            exec(
                db,
                "CREATE (:Person {name: 'Ada'})-[:KNOWS]->(:Person {name: 'Grace'})",
                None,
            )
        };
        assert_eq!(s, LoraStatus::Ok as c_int, "err={:?}", e);
        unsafe { lora_db_free(db) };

        let reopened = new_db_with_wal(&dir);
        let (s, r, e) = unsafe {
            exec(
                reopened,
                "MATCH (p:Person) RETURN p.name AS name ORDER BY name",
                None,
            )
        };
        assert_eq!(s, LoraStatus::Ok as c_int, "err={:?}", e);
        let payload: serde_json::Value = serde_json::from_str(&r.unwrap()).unwrap();
        assert_eq!(
            payload["rows"],
            serde_json::json!([
                { "name": "Ada" },
                { "name": "Grace" }
            ])
        );
        unsafe { lora_db_free(reopened) };
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_match_returns_empty_rows() {
        let db = new_db();
        let (s, r, _) = unsafe { exec(db, "MATCH (n) RETURN n", None) };
        assert_eq!(s, LoraStatus::Ok as c_int);
        let payload: serde_json::Value = serde_json::from_str(&r.unwrap()).unwrap();
        assert_eq!(payload["rows"], serde_json::json!([]));
        unsafe { lora_db_free(db) };
    }

    #[test]
    fn create_and_count() {
        let db = new_db();
        let (s, _, e) = unsafe { exec(db, "CREATE (:X), (:Y)-[:R]->(:Z)", None) };
        assert_eq!(s, LoraStatus::Ok as c_int, "err={:?}", e);
        let mut nc: u64 = 0;
        assert_eq!(unsafe { lora_db_node_count(db, &mut nc) }, 0);
        assert_eq!(nc, 3);
        let mut rc: u64 = 0;
        assert_eq!(unsafe { lora_db_relationship_count(db, &mut rc) }, 0);
        assert_eq!(rc, 1);
        assert_eq!(unsafe { lora_db_clear(db) }, 0);
        unsafe { lora_db_node_count(db, &mut nc) };
        assert_eq!(nc, 0);
        unsafe { lora_db_free(db) };
    }

    #[test]
    fn transaction_json_commits_and_rolls_back() {
        let db = new_db();
        let (s, r, e) = unsafe {
            tx(
                db,
                r#"[
                    {"query":"CREATE (:T {id: $id})","params":{"id":1}},
                    {"query":"MATCH (n:T) RETURN n.id AS id"}
                ]"#,
                None,
            )
        };
        assert_eq!(s, LoraStatus::Ok as c_int, "err={:?}", e);
        let payload: serde_json::Value = serde_json::from_str(&r.unwrap()).unwrap();
        assert_eq!(payload[1]["rows"], serde_json::json!([{ "id": 1 }]));

        let (s, _, e) = unsafe {
            tx(
                db,
                r#"[
                    {"query":"CREATE (:T {id: 2})"},
                    {"query":"THIS IS NOT CYPHER"}
                ]"#,
                None,
            )
        };
        assert_eq!(s, LoraStatus::LoraError as c_int);
        assert!(e.unwrap().starts_with("LORA_ERROR:"));

        let (s, r, e) = unsafe { exec(db, "MATCH (n:T) RETURN n.id AS id ORDER BY id", None) };
        assert_eq!(s, LoraStatus::Ok as c_int, "err={:?}", e);
        let payload: serde_json::Value = serde_json::from_str(&r.unwrap()).unwrap();
        assert_eq!(payload["rows"], serde_json::json!([{ "id": 1 }]));
        unsafe { lora_db_free(db) };
    }

    #[test]
    fn params_scalar_round_trip() {
        let db = new_db();
        let (s, _, e) = unsafe {
            exec(
                db,
                "CREATE (:I {n: $n, q: $q, a: $a, s: $s})",
                Some(r#"{"n":"widget","q":42,"a":true,"s":1.5}"#),
            )
        };
        assert_eq!(s, LoraStatus::Ok as c_int, "err={:?}", e);
        let (s, r, _) = unsafe {
            exec(
                db,
                "MATCH (i:I) RETURN i.n AS n, i.q AS q, i.a AS a, i.s AS s",
                None,
            )
        };
        assert_eq!(s, LoraStatus::Ok as c_int);
        let payload: serde_json::Value = serde_json::from_str(&r.unwrap()).unwrap();
        let row = &payload["rows"][0];
        assert_eq!(row["n"], "widget");
        assert_eq!(row["q"], 42);
        assert_eq!(row["a"], true);
        assert_eq!(row["s"], 1.5);
        unsafe { lora_db_free(db) };
    }

    #[test]
    fn parse_error_reports_lora_error() {
        let db = new_db();
        let (s, r, e) = unsafe { exec(db, "NOT CYPHER", None) };
        assert_eq!(s, LoraStatus::LoraError as c_int);
        assert!(r.is_none());
        let e = e.unwrap();
        assert!(e.starts_with("LORA_ERROR: "), "got: {e}");
        unsafe { lora_db_free(db) };
    }

    #[test]
    fn invalid_params_reports_invalid_params() {
        let db = new_db();
        let (s, _, e) = unsafe { exec(db, "RETURN $x AS x", Some("[1,2,3]")) };
        assert_eq!(s, LoraStatus::InvalidParams as c_int);
        let e = e.unwrap();
        assert!(e.starts_with("INVALID_PARAMS: "), "got: {e}");
        unsafe { lora_db_free(db) };
    }

    #[test]
    fn vector_round_trip_via_json() {
        let db = new_db();

        // Construct a vector and read the resulting JSON.
        let (s, r, _) = unsafe { exec(db, "RETURN vector([1,2,3], 3, INTEGER) AS v", None) };
        assert_eq!(s, LoraStatus::Ok as c_int);
        let payload: serde_json::Value = serde_json::from_str(&r.unwrap()).unwrap();
        let v = &payload["rows"][0]["v"];
        assert_eq!(v["kind"], "vector");
        assert_eq!(v["dimension"], 3);
        assert_eq!(v["coordinateType"], "INTEGER");
        assert_eq!(v["values"], serde_json::json!([1, 2, 3]));

        // Pass a vector back in as a parameter and verify round-trip.
        let params = r#"{"v":{"kind":"vector","dimension":3,"coordinateType":"FLOAT32","values":[0.1,0.2,0.3]}}"#;
        let (s, r, _) = unsafe { exec(db, "RETURN $v AS v", Some(params)) };
        assert_eq!(s, LoraStatus::Ok as c_int);
        let payload: serde_json::Value = serde_json::from_str(&r.unwrap()).unwrap();
        let v = &payload["rows"][0]["v"];
        assert_eq!(v["kind"], "vector");
        assert_eq!(v["coordinateType"], "FLOAT32");

        unsafe { lora_db_free(db) };
    }

    // ------------------------------------------------------------------
    // Vector parameter validation
    // ------------------------------------------------------------------

    fn exec_params_err(db: *mut LoraDatabase, params_json: &str) -> String {
        let (status, result, err) = unsafe { exec(db, "RETURN $v AS v", Some(params_json)) };
        assert_eq!(
            status,
            LoraStatus::InvalidParams as c_int,
            "result={result:?}"
        );
        assert!(result.is_none());
        err.unwrap()
    }

    #[test]
    fn vector_param_missing_dimension_errors() {
        let db = new_db();
        let err = exec_params_err(
            db,
            r#"{"v":{"kind":"vector","coordinateType":"FLOAT32","values":[1.0, 2.0]}}"#,
        );
        assert!(err.starts_with("INVALID_PARAMS:"), "got: {err}");
        assert!(err.contains("dimension"), "got: {err}");
        unsafe { lora_db_free(db) };
    }

    #[test]
    fn vector_param_missing_values_errors() {
        let db = new_db();
        let err = exec_params_err(
            db,
            r#"{"v":{"kind":"vector","dimension":2,"coordinateType":"FLOAT32"}}"#,
        );
        assert!(err.contains("values"), "got: {err}");
        unsafe { lora_db_free(db) };
    }

    #[test]
    fn vector_param_unknown_coord_type_errors() {
        let db = new_db();
        let err = exec_params_err(
            db,
            r#"{"v":{"kind":"vector","dimension":2,"coordinateType":"BIGINT","values":[1,2]}}"#,
        );
        assert!(
            err.contains("coordinate type") || err.contains("coordinateType"),
            "got: {err}"
        );
        unsafe { lora_db_free(db) };
    }

    #[test]
    fn vector_param_non_numeric_values_error() {
        let db = new_db();
        let err = exec_params_err(
            db,
            r#"{"v":{"kind":"vector","dimension":3,"coordinateType":"FLOAT32","values":[1.0,"oops",3.0]}}"#,
        );
        assert!(
            err.contains("numeric") || err.contains("number"),
            "got: {err}"
        );
        unsafe { lora_db_free(db) };
    }

    #[test]
    fn vector_param_dimension_mismatch_errors() {
        let db = new_db();
        let err = exec_params_err(
            db,
            r#"{"v":{"kind":"vector","dimension":4,"coordinateType":"INTEGER","values":[1,2,3]}}"#,
        );
        assert!(err.contains("dimension"), "got: {err}");
        unsafe { lora_db_free(db) };
    }

    #[test]
    fn vector_param_int8_overflow_errors() {
        let db = new_db();
        let err = exec_params_err(
            db,
            r#"{"v":{"kind":"vector","dimension":1,"coordinateType":"INTEGER8","values":[999]}}"#,
        );
        assert!(
            err.contains("range") || err.contains("INTEGER8"),
            "got: {err}"
        );
        unsafe { lora_db_free(db) };
    }

    #[test]
    fn vector_param_values_not_array_errors() {
        let db = new_db();
        let err = exec_params_err(
            db,
            r#"{"v":{"kind":"vector","dimension":3,"coordinateType":"FLOAT32","values":"[1,2,3]"}}"#,
        );
        assert!(err.contains("values"), "got: {err}");
        unsafe { lora_db_free(db) };
    }

    // JSON literally allows NaN only as a non-standard extension; serde
    // rejects it at the parser step. The closest we can drive from
    // outside is a numeric value outside the FP range.
    #[test]
    fn vector_param_float32_overflow_errors() {
        let db = new_db();
        // f32::MAX * 10 — well above f32's range, still fits in f64.
        let err = exec_params_err(
            db,
            r#"{"v":{"kind":"vector","dimension":1,"coordinateType":"FLOAT32","values":[1e100]}}"#,
        );
        assert!(
            err.contains("range") || err.contains("FLOAT32"),
            "got: {err}"
        );
        unsafe { lora_db_free(db) };
    }

    #[test]
    fn vector_json_shape_is_deterministic() {
        // Every binding depends on this exact tagged shape — pin it down.
        let db = new_db();
        let (s, r, _) = unsafe { exec(db, "RETURN vector([1, 2, 3], 3, INTEGER16) AS v", None) };
        assert_eq!(s, LoraStatus::Ok as c_int);
        let payload: serde_json::Value = serde_json::from_str(&r.unwrap()).unwrap();
        let v = &payload["rows"][0]["v"];
        assert_eq!(
            v,
            &serde_json::json!({
                "kind": "vector",
                "dimension": 3,
                "coordinateType": "INTEGER16",
                "values": [1, 2, 3],
            })
        );
        unsafe { lora_db_free(db) };
    }

    #[test]
    fn null_pointer_is_reported() {
        let mut out_result: *mut c_char = ptr::null_mut();
        let mut out_error: *mut c_char = ptr::null_mut();
        let s = unsafe {
            lora_db_execute_json(
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                &mut out_result,
                &mut out_error,
            )
        };
        assert_eq!(s, LoraStatus::NullPointer as c_int);
    }
}
