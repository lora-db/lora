//! Process-wide property-key intern table.
//!
//! Every `NodeRecord` / `RelationshipRecord` stores its properties in a
//! `BTreeMap<Arc<str>, PropertyValue>`. Without sharing, the byte buffer
//! behind each key string is duplicated per record — a 5-million-row
//! CSV import with eight columns ends up with forty million heap copies
//! of the same handful of column-name strings, dwarfing the actual
//! payload. Routing every key insertion through [`intern`] collapses
//! those copies to one allocation per *distinct* key, which is the
//! single largest memory win available without a wider columnar
//! rewrite of the node layout.
//!
//! The table is intentionally weak: entries live forever. Realistic
//! workloads have at most a few hundred distinct property names, so
//! the steady-state footprint is negligible. Callers that need to
//! intern an unbounded universe of strings (e.g. user-controlled keys
//! at high cardinality) should keep using owned `String`s upstream
//! and only intern at storage boundaries.
//!
//! Concurrency: each thread keeps a tiny front cache for hot keys. A
//! cache hit avoids the process-wide table entirely; misses go through
//! a `RwLock`, taking the write lock only when a genuinely-new key
//! appears.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, RwLock};

thread_local! {
    static LOCAL_TABLE: RefCell<HashMap<String, Arc<str>>> =
        RefCell::new(HashMap::with_capacity(32));
}

fn table() -> &'static RwLock<HashSet<Arc<str>>> {
    static TABLE: OnceLock<RwLock<HashSet<Arc<str>>>> = OnceLock::new();
    TABLE.get_or_init(|| RwLock::new(HashSet::with_capacity(64)))
}

/// Return a process-wide-shared `Arc<str>` for `s`. The first caller
/// with a given byte sequence allocates; every subsequent caller with
/// the same bytes gets an `Arc::clone` of the original — one refcount
/// bump, no heap allocation.
pub fn intern(s: &str) -> Arc<str> {
    if let Some(existing) = LOCAL_TABLE.with(|local| local.borrow().get(s).cloned()) {
        return existing;
    }

    let shared = intern_global(s);
    LOCAL_TABLE.with(|local| {
        local
            .borrow_mut()
            .insert(shared.as_ref().to_owned(), shared.clone());
    });
    shared
}

fn intern_global(s: &str) -> Arc<str> {
    // Fast path: read lock + lookup. After warmup virtually all calls
    // land here, so concurrent writers (e.g. parallel ingest) don't
    // serialize on the table.
    {
        let guard = table().read().unwrap_or_else(|p| p.into_inner());
        if let Some(existing) = guard.get(s) {
            return existing.clone();
        }
    }
    let mut guard = table().write().unwrap_or_else(|p| p.into_inner());
    if let Some(existing) = guard.get(s) {
        return existing.clone();
    }
    let fresh: Arc<str> = Arc::from(s);
    guard.insert(fresh.clone());
    fresh
}

/// Variant that consumes an owned `String`. When the value is already
/// in the table, the input allocation is dropped on return; when it
/// isn't, the input bytes are moved into the new `Arc<str>` without a
/// re-copy.
pub fn intern_owned(s: String) -> Arc<str> {
    if let Some(existing) = LOCAL_TABLE.with(|local| local.borrow().get(s.as_str()).cloned()) {
        return existing;
    }

    let shared = intern_owned_global(s);
    LOCAL_TABLE.with(|local| {
        local
            .borrow_mut()
            .insert(shared.as_ref().to_owned(), shared.clone());
    });
    shared
}

fn intern_owned_global(s: String) -> Arc<str> {
    {
        let guard = table().read().unwrap_or_else(|p| p.into_inner());
        if let Some(existing) = guard.get(s.as_str()) {
            return existing.clone();
        }
    }
    let mut guard = table().write().unwrap_or_else(|p| p.into_inner());
    if let Some(existing) = guard.get(s.as_str()) {
        return existing.clone();
    }
    let fresh: Arc<str> = Arc::from(s.into_boxed_str());
    guard.insert(fresh.clone());
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_returns_pointer_equal_arcs() {
        let a = intern("first_name");
        let b = intern("first_name");
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn distinct_keys_get_distinct_arcs() {
        let a = intern("alpha");
        let b = intern("beta");
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(&*a, "alpha");
        assert_eq!(&*b, "beta");
    }

    #[test]
    fn intern_owned_dedups_with_intern() {
        let a = intern("from_borrowed");
        let b = intern_owned(String::from("from_borrowed"));
        assert!(Arc::ptr_eq(&a, &b));
    }
}
