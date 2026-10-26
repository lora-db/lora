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
//! Entries nothing else holds are swept: whenever the table has doubled
//! since the last sweep, the next insert drops every key only the table
//! still refers to, so dynamic keys (`SET n += $props` with user-chosen
//! names, keys of a rolled-back write) do not pile up for the life of the
//! process. The sweep is amortised over the inserts that doubled it.
//!
//! Concurrency: each thread keeps a small front cache for hot keys. A
//! cache hit avoids the process-wide table entirely; misses go through
//! a `RwLock`, taking the write lock only when a genuinely-new key
//! appears. A cache is cleared when full, so keys it holds (which the
//! sweep must keep: the cache is a reference) stay bounded per thread.
//!
//! The sweep cannot race a lookup: it runs under the write lock and
//! drops only entries with a strong count of one, and every other `Arc`
//! leaves the table under its lock. An entry another thread can still
//! clone without the lock has a second reference, so it is kept.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock, RwLock};

/// Keys a thread's front cache holds before it is cleared.
const LOCAL_CAP: usize = 256;
/// Table size below which no sweep runs.
const SWEEP_MIN: usize = 1024;

thread_local! {
    static LOCAL_TABLE: RefCell<HashSet<Arc<str>>> =
        RefCell::new(HashSet::with_capacity(32));
}

struct Table {
    keys: HashSet<Arc<str>>,
    /// Size at which the next insert sweeps.
    sweep_at: usize,
}

impl Table {
    /// Insert `fresh`, first sweeping unreferenced keys when the table has
    /// grown to `sweep_at`. Runs under the write lock.
    fn insert(&mut self, fresh: Arc<str>) {
        if self.keys.len() >= self.sweep_at {
            self.keys.retain(|key| Arc::strong_count(key) > 1);
            self.sweep_at = (self.keys.len() * 2).max(SWEEP_MIN);
        }
        self.keys.insert(fresh);
    }
}

fn table() -> &'static RwLock<Table> {
    static TABLE: OnceLock<RwLock<Table>> = OnceLock::new();
    TABLE.get_or_init(|| {
        RwLock::new(Table {
            keys: HashSet::with_capacity(64),
            sweep_at: SWEEP_MIN,
        })
    })
}

#[inline]
fn local_get(s: &str) -> Option<Arc<str>> {
    LOCAL_TABLE.with(|local| local.borrow().get(s).cloned())
}

#[inline(never)]
fn local_insert(shared: &Arc<str>) {
    LOCAL_TABLE.with(|local| {
        let mut local = local.borrow_mut();
        if local.len() >= LOCAL_CAP {
            local.clear();
        }
        local.insert(shared.clone());
    });
}

/// Return a process-wide-shared `Arc<str>` for `s`. The first caller
/// with a given byte sequence allocates; every subsequent caller with
/// the same bytes gets an `Arc::clone` of the original — one refcount
/// bump, no heap allocation.
#[inline]
pub fn intern(s: &str) -> Arc<str> {
    if let Some(existing) = local_get(s) {
        return existing;
    }
    let shared = intern_global(s, Arc::from);
    local_insert(&shared);
    shared
}

/// Variant that consumes an owned `String`. When the value is already
/// in the table, the input allocation is dropped on return; when it
/// isn't, the input bytes are moved into the new `Arc<str>` without a
/// re-copy.
pub fn intern_owned(s: String) -> Arc<str> {
    if let Some(existing) = local_get(&s) {
        return existing;
    }
    let shared = intern_global(s, |s: String| Arc::from(s.into_boxed_str()));
    local_insert(&shared);
    shared
}

#[inline(never)]
fn intern_global<S: AsRef<str>>(s: S, make: impl FnOnce(S) -> Arc<str>) -> Arc<str> {
    // Fast path: read lock + lookup. After warmup virtually all calls
    // land here, so concurrent writers (e.g. parallel ingest) don't
    // serialize on the table.
    {
        let guard = table().read().unwrap_or_else(|p| p.into_inner());
        if let Some(existing) = guard.keys.get(s.as_ref()) {
            return existing.clone();
        }
    }
    let mut guard = table().write().unwrap_or_else(|p| p.into_inner());
    if let Some(existing) = guard.keys.get(s.as_ref()) {
        return existing.clone();
    }
    let fresh = make(s);
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

    /// Keys nothing holds any more are swept once the table doubles, and a
    /// thread's front cache stays capped, so a stream of one-off keys does
    /// not grow memory without bound.
    #[test]
    fn unreferenced_keys_are_swept() {
        for i in 0..100_000 {
            drop(intern_owned(format!("one_off_key_{i}")));
        }
        let held = intern("held_across_sweeps");
        for i in 0..100_000 {
            drop(intern(&format!("another_one_off_key_{i}")));
        }
        let len = table().read().unwrap().keys.len();
        // Other tests intern a few keys concurrently; one-off keys alone
        // would be 200k without the sweep.
        assert!(
            len < 4 * SWEEP_MIN + 4 * LOCAL_CAP,
            "table holds {len} keys"
        );
        assert!(LOCAL_TABLE.with(|local| local.borrow().len()) <= LOCAL_CAP);
        // A key still referenced survives and stays the shared copy.
        assert!(Arc::ptr_eq(
            &held,
            &intern_owned("held_across_sweeps".into())
        ));
        assert_eq!(&*intern("one_off_key_7"), "one_off_key_7");
    }
}
