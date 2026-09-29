//! Real heap-footprint probe for `InMemoryGraph`.
//!
//! Run with: `cargo run --release -p lora-database --example heap_probe`
//!
//! `benches/memory.rs` reports `MemoryReport`, which is an *estimate*
//! computed from container lengths and capacities. It cannot see
//! allocator-level overhead such as a `BTreeMap` leaf node sized for
//! eleven entries holding one. This probe wraps the system allocator
//! with a live-byte counter and reports what a graph actually retains,
//! so storage-shape changes can be checked against ground truth.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

use lora_database::{Database, ExecuteOptions, InMemoryGraph, ResultFormat};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = System.realloc(ptr, layout, new_size);
        if !p.is_null() {
            LIVE.fetch_add(
                new_size as isize - layout.size() as isize,
                Ordering::Relaxed,
            );
        }
        p
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const BATCH: usize = 2_000;

fn run(db: &Database<InMemoryGraph>, q: &str) {
    db.execute(
        q,
        Some(ExecuteOptions {
            format: ResultFormat::Rows,
        }),
    )
    .unwrap_or_else(|e| panic!("{q}: {e}"));
}

fn batched(db: &Database<InMemoryGraph>, n: usize, body: impl Fn(usize, usize) -> String) {
    let mut i = 0;
    while i < n {
        let end = (i + BATCH).min(n);
        run(db, &body(i, end - 1));
        i = end;
    }
}

fn measure(name: &str, nodes: usize, rels: usize, build: impl FnOnce(&Database<InMemoryGraph>)) {
    let before = LIVE.load(Ordering::Relaxed);
    let db = Database::in_memory();
    build(&db);
    let after = LIVE.load(Ordering::Relaxed);
    let bytes = (after - before) as f64;
    let per = if rels > 0 {
        format!(
            "bytes/(node+rel)={:.1}",
            bytes / (nodes as f64 + rels as f64)
        )
    } else {
        format!("bytes/node={:.1}", bytes / nodes as f64)
    };
    println!(
        "heap scenario={name} nodes={nodes} rels={rels} live_bytes={after_minus} {per}",
        after_minus = after - before
    );
    drop(db);
}

/// Festimap-shaped graph from the P2-5 report: 20k festivals, 5k users,
/// 100k FOLLOWS relationships, a few properties each.
fn festimap(db: &Database<InMemoryGraph>, schema: bool) {
    if schema {
        run(
            db,
            "CREATE CONSTRAINT fk FOR (f:Festival) REQUIRE f.key IS UNIQUE",
        );
        run(
            db,
            "CREATE CONSTRAINT uk FOR (u:User) REQUIRE u.key IS UNIQUE",
        );
        run(
            db,
            "CREATE FULLTEXT INDEX ft FOR (f:Festival) ON EACH [f.name]",
        );
        run(
            db,
            "CREATE POINT INDEX loc FOR (f:Festival) ON (f.location)",
        );
    }
    batched(db, 20_000, |a, b| {
        format!(
            "UNWIND list.range({a}, {b}) AS i CREATE (:Festival {{key: 'f' + toString(i), \
             name: 'Festival number ' + toString(i), year: 2026, \
             location: point({{latitude: 50.0 + (i % 100) / 100.0, longitude: 4.0 + (i % 50) / 50.0}})}})"
        )
    });
    batched(db, 5_000, |a, b| {
        format!(
            "UNWIND list.range({a}, {b}) AS i CREATE (:User {{key: 'u' + toString(i), \
             email: 'user' + toString(i) + '@example.com'}})"
        )
    });
    if !schema {
        // Relationship creation looks nodes up by key; give it an index.
        run(db, "CREATE INDEX fk FOR (f:Festival) ON (f.key)");
        run(db, "CREATE INDEX uk FOR (u:User) ON (u.key)");
    }
    batched(db, 100_000, |a, b| {
        format!(
            "UNWIND list.range({a}, {b}) AS i \
             MATCH (u:User {{key: 'u' + toString(i % 5000)}}), (f:Festival {{key: 'f' + toString((i * 7) % 20000)}}) \
             CREATE (u)-[:FOLLOWS {{since: 2020 + i % 6}}]->(f)"
        )
    });
}

fn main() {
    if std::env::args().any(|a| a == "--festimap") {
        measure("festimap_plain", 25_000, 100_000, |db| festimap(db, false));
        measure("festimap_schema", 25_000, 100_000, |db| festimap(db, true));
        return;
    }
    let n = 100_000;

    measure("nodes_3props", n, 0, |db| {
        batched(db, n, |a, b| {
            format!(
                "UNWIND list.range({a}, {b}) AS i CREATE (:Node {{id: i, name: 'node_' + type.cast(i, STRING), value: i % 100}})"
            )
        })
    });

    measure("nodes_1prop", n, 0, |db| {
        batched(db, n, |a, b| {
            format!("UNWIND list.range({a}, {b}) AS i CREATE (:Chain {{idx: i}})")
        })
    });

    measure("nodes_no_props", n, 0, |db| {
        batched(db, n, |a, b| {
            format!("UNWIND list.range({a}, {b}) AS i CREATE (:Bare)")
        })
    });

    // Same nodes as `nodes_1prop` plus a property index, to isolate
    // the index's per-entry cost.
    measure("nodes_1prop_indexed", n, 0, |db| {
        run(db, "CREATE INDEX c_idx FOR (c:Chain) ON (c.idx)");
        batched(db, n, |a, b| {
            format!("UNWIND list.range({a}, {b}) AS i CREATE (:Chain {{idx: i}})")
        })
    });

    // Chain with property-less relationships: rel records dominate.
    measure("chain_rels", n, n - 1, |db| {
        run(
            db,
            &format!(
                "UNWIND list.range(0, {}) AS i CREATE (:C {{idx: i}})",
                n - 1
            ),
        );
        run(db, "CREATE INDEX c_idx FOR (c:C) ON (c.idx)");
        batched(db, n - 1, |a, b| {
            format!(
                "UNWIND list.range({a}, {b}) AS i MATCH (x:C {{idx: i}}), (y:C {{idx: i + 1}}) CREATE (x)-[:NEXT]->(y)"
            )
        })
    });

    // Relationships carrying a property.
    measure("chain_rel_props", n, n - 1, |db| {
        run(
            db,
            &format!(
                "UNWIND list.range(0, {}) AS i CREATE (:C {{idx: i}})",
                n - 1
            ),
        );
        run(db, "CREATE INDEX c_idx FOR (c:C) ON (c.idx)");
        batched(db, n - 1, |a, b| {
            format!(
                "UNWIND list.range({a}, {b}) AS i MATCH (x:C {{idx: i}}), (y:C {{idx: i + 1}}) CREATE (x)-[:NEXT {{w: i}}]->(y)"
            )
        })
    });
}
