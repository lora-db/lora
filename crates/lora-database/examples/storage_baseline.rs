//! Storage baseline probe: memory per element, read/write latency,
//! snapshot and WAL recovery cost of today's `InMemoryGraph` at several
//! graph sizes. Written to give a "graphs larger than RAM" design doc
//! numbers to start from.
//!
//! Graph shape (N = node count):
//!   * N `:Person` nodes, each with 4 properties: id: Int (= i),
//!     name: ~8-byte String, score: Float, bio: 40-byte String
//!   * 4 outgoing `:KNOWS` relationships per node (rels = 4N, total degree 8),
//!     pseudo-random targets, one Int property `w`.
//!   * elements = nodes + rels = 5N.
//!
//! Modes:
//!   storage_baseline mem <variant> <N>   one variant, retained heap + RSS
//!   storage_baseline lat <N>             latency suite + snapshot save/load
//!   storage_baseline wal <N>             Cypher ingest with WAL, then recover()
//!
//! Memory method: a counting global allocator tracks live requested bytes;
//! the delta across building the graph (held alive) is "retained heap".
//! RSS (from `ps`) is reported alongside and includes allocator slack.
//! Variants differ by one feature so per-element costs come from differencing.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lora_ast::Direction;
use lora_database::{Database, ExecuteOptions, InMemoryGraph, LoraValue, ResultFormat, WalConfig};
use lora_store::{intern, GraphStorage, GraphStorageMut, Properties, PropertyValue};

// ---------------------------------------------------------------------------
// counting allocator
// ---------------------------------------------------------------------------

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

fn live() -> isize {
    LIVE.load(Ordering::Relaxed)
}

fn rss_bytes() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        * 1024
}

// ---------------------------------------------------------------------------
// data generation
// ---------------------------------------------------------------------------

const DEGREE: u64 = 4;

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn target(i: u64, k: u64, n: u64) -> u64 {
    splitmix(i * DEGREE + k) % n
}

fn name_of(i: u64) -> String {
    format!("p{i:07}")
}

/// Exactly 40 bytes.
fn bio_of(i: u64) -> String {
    let s = format!("bio {i:010} lorem ipsum dolor sit amet..");
    debug_assert_eq!(s.len(), 40);
    s
}

fn score_of(i: u64) -> f64 {
    (splitmix(i ^ 0xABCD) % 1_000_000) as f64 / 1000.0
}

#[derive(Clone, Copy, PartialEq)]
enum Props {
    None,
    All,
    Int,
    Short,
    Float,
    Long,
}

fn node_props(i: u64, which: Props) -> Properties {
    let mut p = Properties::new();
    let all = which == Props::All;
    if all || which == Props::Int {
        p.insert(intern("id"), PropertyValue::Int(i as i64));
    }
    if all || which == Props::Short {
        p.insert(intern("name"), PropertyValue::String(name_of(i)));
    }
    if all || which == Props::Float {
        p.insert(intern("score"), PropertyValue::Float(score_of(i)));
    }
    if all || which == Props::Long {
        p.insert(intern("bio"), PropertyValue::String(bio_of(i)));
    }
    p
}

/// Build the graph directly through `GraphStorageMut` (no Cypher).
fn build(n: u64, props: Props, rels: bool, rel_props: bool) -> InMemoryGraph {
    let mut g = InMemoryGraph::new();
    for i in 0..n {
        let rec = g.create_node(vec!["Person".to_string()], node_props(i, props));
        debug_assert_eq!(rec.id, i);
    }
    if rels {
        let w = intern("w");
        for i in 0..n {
            for k in 0..DEGREE {
                let mut p = Properties::new();
                if rel_props {
                    p.insert(w.clone(), PropertyValue::Int(((i + k) % 100) as i64));
                }
                g.create_relationship(i, target(i, k, n), "KNOWS", p)
                    .expect("rel");
            }
        }
    }
    g
}

// ---------------------------------------------------------------------------
// mem mode
// ---------------------------------------------------------------------------

fn run(db: &Database<InMemoryGraph>, q: &str) {
    db.execute(
        q,
        Some(ExecuteOptions {
            format: ResultFormat::Rows,
        }),
    )
    .unwrap_or_else(|e| panic!("{q}: {e}"));
}

fn mem_mode(variant: &str, n: u64) {
    let rss0 = rss_bytes();
    let live0 = live();
    let t = Instant::now();
    let (nodes, rels, props_per_node, keep): (u64, u64, u64, Box<dyn std::any::Any>) = match variant
    {
        "bare" => (n, 0, 0, Box::new(build(n, Props::None, false, false))),
        "props" => (n, 0, 4, Box::new(build(n, Props::All, false, false))),
        "p_int" => (n, 0, 1, Box::new(build(n, Props::Int, false, false))),
        "p_short" => (n, 0, 1, Box::new(build(n, Props::Short, false, false))),
        "p_float" => (n, 0, 1, Box::new(build(n, Props::Float, false, false))),
        "p_long" => (n, 0, 1, Box::new(build(n, Props::Long, false, false))),
        "rels" => (
            n,
            n * DEGREE,
            4,
            Box::new(build(n, Props::All, true, false)),
        ),
        "relprops" => (n, n * DEGREE, 4, Box::new(build(n, Props::All, true, true))),
        "range_idx" | "unique" | "hash_idx" | "range_idx_name" => {
            let g = build(n, Props::All, true, true);
            let db = Database::from_graph(g);
            match variant {
                "range_idx" => run(&db, "CREATE INDEX person_id FOR (p:Person) ON (p.id)"),
                "range_idx_name" => run(&db, "CREATE INDEX person_name FOR (p:Person) ON (p.name)"),
                "unique" => run(
                    &db,
                    "CREATE CONSTRAINT person_id_u FOR (p:Person) REQUIRE p.id IS UNIQUE",
                ),
                "hash_idx" => {
                    // Implicit lazily-built hash-bucket index: first equality
                    // lookup on a key indexes every node carrying it.
                    db.with_store(|s| {
                        s.find_node_ids_by_property(Some("Person"), "id", &PropertyValue::Int(1))
                    });
                }
                _ => unreachable!(),
            }
            (n, n * DEGREE, 4, Box::new(db))
        }
        other => panic!("unknown variant {other}"),
    };
    let build_s = t.elapsed().as_secs_f64();
    let bytes = (live() - live0) as f64;
    let rss = rss_bytes().saturating_sub(rss0) as f64;
    let estimate = if let Some(g) = keep.downcast_ref::<InMemoryGraph>() {
        g.memory_estimate().total_bytes() as f64
    } else if let Some(db) = keep.downcast_ref::<Database<InMemoryGraph>>() {
        db.with_store(|g| g.memory_estimate().total_bytes()) as f64
    } else {
        0.0
    };
    println!(
        "mem variant={variant} nodes={nodes} rels={rels} props_per_node={props_per_node} \
         live_bytes={bytes:.0} rss_delta={rss:.0} estimate={estimate:.0} \
         live_per_element={:.1} rss_per_element={:.1} build_s={build_s:.2}",
        bytes / (nodes + rels) as f64,
        rss / (nodes + rels) as f64
    );
    drop(keep);
}

// ---------------------------------------------------------------------------
// latency helpers
// ---------------------------------------------------------------------------

struct Stats {
    median: f64,
    p99: f64,
    mean: f64,
    samples: usize,
}

fn stats(mut v: Vec<f64>) -> Stats {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    let pick = |q: f64| v[((n as f64 - 1.0) * q).round() as usize];
    Stats {
        median: pick(0.5),
        p99: pick(0.99),
        mean: v.iter().sum::<f64>() / n as f64,
        samples: n,
    }
}

fn fmt_ns(ns: f64) -> String {
    if ns >= 1e9 {
        format!("{:.2}s", ns / 1e9)
    } else if ns >= 1e6 {
        format!("{:.2}ms", ns / 1e6)
    } else if ns >= 1e3 {
        format!("{:.2}us", ns / 1e3)
    } else {
        format!("{ns:.0}ns")
    }
}

/// Time `f(i)` for `samples` iterations after `samples/10` warmup calls.
fn bench(name: &str, samples: usize, mut f: impl FnMut(u64)) -> Stats {
    let warm = (samples / 10).max(1);
    for i in 0..warm {
        f(splitmix(i as u64 ^ 0x5555));
    }
    let mut v = Vec::with_capacity(samples);
    for i in 0..samples {
        let r = splitmix(i as u64 + 1_000_000);
        let t = Instant::now();
        f(r);
        v.push(t.elapsed().as_nanos() as f64);
    }
    let s = stats(v);
    println!(
        "lat {name:<28} median={:>10} p99={:>10} mean={:>10} samples={}",
        fmt_ns(s.median),
        fmt_ns(s.p99),
        fmt_ns(s.mean),
        s.samples
    );
    s
}

/// For sub-microsecond ops: each sample times `BATCH_OPS` calls and
/// reports the per-op average (macOS `Instant` ticks at ~42 ns).
const BATCH_OPS: u64 = 64;
fn bench_batched(name: &str, samples: usize, mut f: impl FnMut(u64)) -> Stats {
    for i in 0..(samples / 10).max(1) as u64 * BATCH_OPS {
        f(splitmix(i ^ 0x7777));
    }
    let mut v = Vec::with_capacity(samples);
    for s in 0..samples as u64 {
        let t = Instant::now();
        for j in 0..BATCH_OPS {
            f(splitmix(s * BATCH_OPS + j + 9_000_000));
        }
        v.push(t.elapsed().as_nanos() as f64 / BATCH_OPS as f64);
    }
    let st = stats(v);
    println!(
        "lat {name:<28} median={:>10} p99={:>10} mean={:>10} samples={}x{BATCH_OPS}",
        fmt_ns(st.median),
        fmt_ns(st.p99),
        fmt_ns(st.mean),
        st.samples
    );
    st
}

fn params(pairs: &[(&str, LoraValue)]) -> BTreeMap<String, LoraValue> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn q(db: &Database<InMemoryGraph>, query: &str, p: BTreeMap<String, LoraValue>) -> usize {
    db.execute_rows_with_params(query, p)
        .unwrap_or_else(|e| panic!("{query}: {e}"))
        .len()
}

fn plan_ops(db: &Database<InMemoryGraph>, query: &str) -> String {
    fn walk(n: &lora_database::PlanTreeNode, out: &mut Vec<String>) {
        out.push(n.operator.clone());
        for c in &n.children {
            walk(c, out);
        }
    }
    match db.explain(query, None) {
        Ok(p) => {
            let mut v = Vec::new();
            walk(&p.tree.root, &mut v);
            v.join(" <- ")
        }
        Err(e) => format!("explain error: {e}"),
    }
}

fn dir_size(p: &std::path::Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let md = e.metadata().unwrap();
            if md.is_dir() {
                total += dir_size(&e.path());
            } else {
                total += md.len();
            }
        }
    }
    total
}

// ---------------------------------------------------------------------------
// lat mode
// ---------------------------------------------------------------------------

fn lat_mode(n: u64) {
    let samples: usize = std::env::var("SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20_000);
    let scan_samples: usize = std::env::var("SCAN_SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(15);
    let write_samples: usize = std::env::var("WRITE_SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5_000);
    let tmp = std::env::var("BENCH_TMP").unwrap_or_else(|_| "/tmp/lora-storage-baseline".into());
    std::fs::create_dir_all(&tmp).unwrap();

    let t = Instant::now();
    let g = build(n, Props::All, true, true);
    let build_s = t.elapsed().as_secs_f64();
    println!(
        "load direct_api nodes={n} rels={} build_s={build_s:.2} elements_per_s={:.0}",
        n * DEGREE,
        (n * (DEGREE + 1)) as f64 / build_s
    );

    // Clone cost: the staged write path clones the whole InMemoryGraph.
    let st = bench("graph_clone(no index)", 200, |_| {
        std::hint::black_box(g.clone());
    });
    let _ = st;

    let db = Database::from_graph(g);
    let t = Instant::now();
    run(&db, "CREATE INDEX person_id FOR (p:Person) ON (p.id)");
    println!("load create_range_index_s={:.3}", t.elapsed().as_secs_f64());
    // force the implicit hash index on `id` so the first measured sample
    // doesn't pay for it
    let t = Instant::now();
    q(
        &db,
        "MATCH (p:Person {id: $id}) RETURN p.name AS name",
        params(&[("id", LoraValue::Int(1))]),
    );
    println!(
        "load first_equality_lookup_s={:.3} (may build implicit hash index)",
        t.elapsed().as_secs_f64()
    );

    db.with_store(|s| {
        let c = s.clone();
        let st = Instant::now();
        let mut v = Vec::new();
        for _ in 0..200 {
            let t = Instant::now();
            std::hint::black_box(s.clone());
            v.push(t.elapsed().as_nanos() as f64);
        }
        let _ = st;
        let s2 = stats(v);
        println!(
            "lat {:<28} median={:>10} p99={:>10} mean={:>10} samples={}",
            "graph_clone(range+hash idx)",
            fmt_ns(s2.median),
            fmt_ns(s2.p99),
            fmt_ns(s2.mean),
            s2.samples
        );
        drop(c);
    });

    let pid = |r: u64| LoraValue::Int((r % n) as i64);

    let queries: Vec<(&str, &str)> = vec![
        (
            "cy_index_seek",
            "MATCH (p:Person {id: $id}) RETURN p.name AS name",
        ),
        (
            "cy_1hop",
            "MATCH (p:Person {id: $id})-[:KNOWS]->(m) RETURN m.id AS id",
        ),
        (
            "cy_2hop",
            "MATCH (p:Person {id: $id})-[:KNOWS]->()-[:KNOWS]->(m) RETURN m.id AS id",
        ),
        (
            "cy_range_seek_10",
            "MATCH (p:Person) WHERE p.id >= $id AND p.id < $id + 10 RETURN p.name AS name",
        ),
        (
            "cy_id_fn",
            "MATCH (p) WHERE id(p) = $id RETURN p.name AS name",
        ),
        ("cy_label_count", "MATCH (p:Person) RETURN count(p) AS c"),
        (
            "cy_label_scan_filter",
            "MATCH (p:Person) WHERE p.score < -1.0 RETURN count(*) AS c",
        ),
        (
            "cy_scan_str_filter",
            "MATCH (p:Person) WHERE p.bio ENDS WITH 'zzz' RETURN count(*) AS c",
        ),
        (
            "cy_eq_unindexed_bio",
            "MATCH (p:Person) WHERE p.bio = 'nope' RETURN count(*) AS c",
        ),
    ];
    for (name, query) in &queries {
        println!("plan {name}: {}", plan_ops(&db, query));
    }

    // --- Cypher reads -------------------------------------------------
    bench("cy_index_seek", samples, |r| {
        let rows = q(&db, queries[0].1, params(&[("id", pid(r))]));
        assert_eq!(rows, 1);
    });
    bench("cy_1hop", samples, |r| {
        q(&db, queries[1].1, params(&[("id", pid(r))]));
    });
    let mut rows2 = 0usize;
    bench("cy_2hop", samples, |r| {
        rows2 += q(&db, queries[2].1, params(&[("id", pid(r))]));
    });
    println!(
        "info cy_2hop avg_rows={:.1}",
        rows2 as f64 / (samples + samples / 10) as f64
    );
    bench("cy_range_seek_10", samples, |r| {
        let lo = (r % (n - 20)) as i64;
        let rows = q(&db, queries[3].1, params(&[("id", LoraValue::Int(lo))]));
        assert_eq!(rows, 10);
    });
    let id_samples = if n <= 20_000 {
        500
    } else if n <= 200_000 {
        100
    } else {
        20
    };
    bench("cy_id_fn(id(n)=$id)", id_samples, |r| {
        q(&db, queries[4].1, params(&[("id", pid(r))]));
    });
    let s = bench("cy_label_count", scan_samples, |_| {
        q(&db, queries[5].1, params(&[]));
    });
    println!("info cy_label_count ns_per_node={:.2}", s.median / n as f64);
    let s = bench("cy_label_scan_filter", scan_samples, |_| {
        q(&db, queries[6].1, params(&[]));
    });
    println!(
        "info cy_label_scan_filter ns_per_node={:.2}",
        s.median / n as f64
    );
    let s = bench("cy_scan_str_filter", scan_samples, |_| {
        q(&db, queries[7].1, params(&[]));
    });
    println!(
        "info cy_scan_str_filter ns_per_node={:.2}",
        s.median / n as f64
    );
    // Equality on an undeclared property: the first call builds an
    // implicit hash index over every node carrying the key.
    let live_before = live();
    let t = Instant::now();
    q(&db, queries[8].1, params(&[]));
    println!(
        "info cy_eq_unindexed_bio first_call={} retained_bytes_after={}",
        fmt_ns(t.elapsed().as_nanos() as f64),
        live() - live_before
    );
    bench("cy_eq_unindexed_bio(2nd+)", samples, |_| {
        q(&db, queries[8].1, params(&[]));
    });

    // --- parse / plan overhead ---------------------------------------
    bench("parse_only(index_seek)", samples, |_| {
        std::hint::black_box(db.parse(queries[0].1).unwrap());
    });
    // Cold compile: a unique query text per sample defeats the plan cache.
    let mut k = 0u64;
    bench("cy_index_seek_uncached", samples / 4, |r| {
        k += 1;
        let text = format!("MATCH (p:Person {{id: $id}}) RETURN p.name AS name{k}");
        q(&db, &text, params(&[("id", pid(r))]));
    });

    // --- raw store / facade -----------------------------------------
    let snap = db.snapshot();
    bench_batched("raw_with_node(borrow)", samples / 8, |r| {
        let v = snap.with_node(r % n, |rec| rec.properties.len()).unwrap();
        std::hint::black_box(v);
    });
    bench_batched("raw_node(clone record)", samples / 8, |r| {
        std::hint::black_box(snap.node(r % n).unwrap());
    });
    bench_batched("db.graph_node", samples / 8, |r| {
        std::hint::black_box(db.graph_node(r % n).unwrap());
    });
    bench_batched("raw_find_by_prop(hash)", samples / 8, |r| {
        let ids = snap.find_node_ids_by_property(
            Some("Person"),
            "id",
            &PropertyValue::Int((r % n) as i64),
        );
        assert_eq!(ids.len(), 1);
    });
    let types = vec!["KNOWS".to_string()];
    bench_batched("raw_1hop_ids", samples / 8, |r| {
        std::hint::black_box(snap.expand_ids(r % n, Direction::Right, &types));
    });
    bench_batched("raw_2hop_ids", samples / 8, |r| {
        let mut c = 0usize;
        for (_, m) in snap.expand_ids(r % n, Direction::Right, &types) {
            c += snap.expand_ids(m, Direction::Right, &types).len();
        }
        std::hint::black_box(c);
    });
    let s = bench("raw_label_scan(read score)", scan_samples, |_| {
        let mut c = 0usize;
        for id in snap.node_ids_by_label("Person") {
            snap.with_node(id, |rec| {
                if let Some(PropertyValue::Float(f)) = rec.properties.get("score") {
                    if *f < -1.0 {
                        c += 1;
                    }
                }
            });
        }
        std::hint::black_box(c);
    });
    println!("info raw_label_scan ns_per_node={:.2}", s.median / n as f64);
    drop(snap);

    // --- writes --------------------------------------------------------
    let mut next = n;
    let create_q = "CREATE (:Person {id: $id, name: $name, score: $score, bio: $bio})";
    println!("plan cy_create: {}", plan_ops(&db, create_q));
    bench("cy_create_node(fast path)", write_samples, |_| {
        let id = next;
        next += 1;
        q(
            &db,
            create_q,
            params(&[
                ("id", LoraValue::Int(id as i64)),
                ("name", LoraValue::String(name_of(id))),
                ("score", LoraValue::Float(score_of(id))),
                ("bio", LoraValue::String(bio_of(id))),
            ]),
        );
    });
    let set_q = "MATCH (p:Person {id: $id}) SET p.score = $v";
    bench("cy_set_prop(fast path)", write_samples, |r| {
        q(
            &db,
            set_q,
            params(&[("id", pid(r)), ("v", LoraValue::Float(1.5))]),
        );
    });
    let rel_q = "MATCH (a:Person {id: $a}), (b:Person {id: $b}) CREATE (a)-[:KNOWS {w: 1}]->(b)";
    bench("cy_create_rel(staged)", write_samples, |r| {
        q(
            &db,
            rel_q,
            params(&[("a", pid(r)), ("b", pid(splitmix(r)))]),
        );
    });
    bench("db.graph_create_node(api)", write_samples, |_| {
        let mut p = BTreeMap::new();
        p.insert("id".to_string(), LoraValue::Int(next as i64));
        next += 1;
        db.graph_create_node(vec!["Person".into()], p).unwrap();
    });

    // Uniqueness constraint on a separate, small label. Any constraint
    // also forces every SET onto the staged path.
    run(
        &db,
        "CREATE CONSTRAINT acct_key FOR (a:Account) REQUIRE a.key IS UNIQUE",
    );
    let mut acct = 0u64;
    let acct_q = "CREATE (:Account {key: $k, name: $name})";
    bench("cy_create_unique(staged)", write_samples, |_| {
        acct += 1;
        q(
            &db,
            acct_q,
            params(&[
                ("k", LoraValue::String(format!("acct-{acct}"))),
                ("name", LoraValue::String("x".into())),
            ]),
        );
    });
    bench("cy_create_node(w/ constraint)", write_samples, |_| {
        let id = next;
        next += 1;
        q(
            &db,
            create_q,
            params(&[
                ("id", LoraValue::Int(id as i64)),
                ("name", LoraValue::String(name_of(id))),
                ("score", LoraValue::Float(score_of(id))),
                ("bio", LoraValue::String(bio_of(id))),
            ]),
        );
    });
    bench("cy_set_prop(staged: constraint)", write_samples, |r| {
        q(
            &db,
            set_q,
            params(&[("id", pid(r)), ("v", LoraValue::Float(2.5))]),
        );
    });
    // Same writes while a reader holds a snapshot: forces copy-on-write
    // of every touched chunk.
    let held = db.snapshot();
    bench("cy_set_prop(reader held)", write_samples, |r| {
        q(
            &db,
            set_q,
            params(&[("id", pid(r)), ("v", LoraValue::Float(3.5))]),
        );
    });
    drop(held);
    run(&db, "DROP CONSTRAINT acct_key");

    // --- snapshots -----------------------------------------------------
    let path_raw = format!("{tmp}/snap_{n}.raw");
    let path_gz = format!("{tmp}/snap_{n}.gz");
    let t = Instant::now();
    db.save_snapshot_to(&path_raw).unwrap();
    let save_raw = t.elapsed().as_secs_f64();
    let t = Instant::now();
    db.save_snapshot_to_with_options(&path_gz, &lora_database::SnapshotOptions::default())
        .unwrap();
    let save_gz = t.elapsed().as_secs_f64();
    let size_raw = std::fs::metadata(&path_raw).unwrap().len();
    let size_gz = std::fs::metadata(&path_gz).unwrap().len();
    let (cn, cr) = (db.node_count(), db.relationship_count());
    drop(db);
    let t = Instant::now();
    let db2 = Database::in_memory_from_snapshot(&path_raw).unwrap();
    let load_raw = t.elapsed().as_secs_f64();
    assert_eq!(db2.node_count(), cn);
    assert_eq!(db2.relationship_count(), cr);
    drop(db2);
    let t = Instant::now();
    let db3 = Database::in_memory_from_snapshot(&path_gz).unwrap();
    let load_gz = t.elapsed().as_secs_f64();
    let indexes_after_load = db3.with_store(|s| s.list_indexes().len());
    drop(db3);
    println!(
        "snapshot nodes={cn} rels={cr} raw_bytes={size_raw} gz_bytes={size_gz} \
         raw_bytes_per_element={:.1} gz_bytes_per_element={:.1} save_raw_s={save_raw:.3} save_gz_s={save_gz:.3} \
         load_raw_s={load_raw:.3} load_gz_s={load_gz:.3} indexes_after_load={indexes_after_load}",
        size_raw as f64 / (cn + cr) as f64,
        size_gz as f64 / (cn + cr) as f64,
    );
    let _ = std::fs::remove_file(&path_raw);
    let _ = std::fs::remove_file(&path_gz);
}

// ---------------------------------------------------------------------------
// wal mode
// ---------------------------------------------------------------------------

fn wal_mode(n: u64) {
    let tmp = std::env::var("BENCH_TMP").unwrap_or_else(|_| "/tmp/lora-storage-baseline".into());
    let dir = std::path::PathBuf::from(format!("{tmp}/wal_{n}"));
    let _ = std::fs::remove_dir_all(&dir);
    let batch: u64 = 10_000;

    let db = Database::open_with_wal(WalConfig::enabled(&dir)).unwrap();
    let t = Instant::now();
    let mut a = 0;
    while a < n {
        let b = (a + batch).min(n) - 1;
        run(
            &db,
            &format!(
                "UNWIND range({a}, {b}) AS i CREATE (:Person {{id: i, name: 'p' + toString(i), \
                 score: toFloat(i % 1000) / 7.0, bio: 'bio ' + toString(i) + ' lorem ipsum dolor sit amet, cons'}})"
            ),
        );
        a = b + 1;
    }
    let nodes_s = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let mut a = 0;
    while a < n {
        let b = (a + batch / DEGREE).min(n) - 1;
        // relationship targets computed in Rust and passed as a list param
        let mut pairs = Vec::new();
        for i in a..=b {
            for k in 0..DEGREE {
                pairs.push(LoraValue::List(vec![
                    LoraValue::Int(i as i64),
                    LoraValue::Int(target(i, k, n) as i64),
                ]));
            }
        }
        q(
            &db,
            "UNWIND $pairs AS pr MATCH (x:Person {id: pr[0]}), (y:Person {id: pr[1]}) \
             CREATE (x)-[:KNOWS {w: pr[0] % 100}]->(y)",
            params(&[("pairs", LoraValue::List(pairs))]),
        );
        a = b + 1;
    }
    let rels_s = t.elapsed().as_secs_f64();
    db.sync().unwrap();
    let (cn, cr) = (db.node_count(), db.relationship_count());
    drop(db);
    let wal_bytes = dir_size(&dir);
    println!(
        "ingest cypher+wal nodes={cn} rels={cr} nodes_s={nodes_s:.2} rels_s={rels_s:.2} \
         node_rate={:.0}/s rel_rate={:.0}/s wal_bytes={wal_bytes} wal_bytes_per_element={:.1}",
        cn as f64 / nodes_s,
        cr as f64 / rels_s,
        wal_bytes as f64 / (cn + cr) as f64
    );
    let t = Instant::now();
    let db =
        Database::recover(format!("{tmp}/no-such-snapshot"), WalConfig::enabled(&dir)).unwrap();
    let rec_s = t.elapsed().as_secs_f64();
    assert_eq!(db.node_count(), cn);
    assert_eq!(db.relationship_count(), cr);
    println!(
        "wal_replay elements={} replay_s={rec_s:.3} elements_per_s={:.0} mb_per_s={:.1}",
        cn + cr,
        (cn + cr) as f64 / rec_s,
        wal_bytes as f64 / 1e6 / rec_s
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// restart mode: memory + time of snapshot load and WAL replay versus the
// process that wrote the data. The writer declares one RANGE index
// (Person.id) and one UNIQUE constraint (Person.name) and runs no ad-hoc
// lookups, so its only active hash indexes are the declared ones.
// ---------------------------------------------------------------------------

fn prop_idx_bytes(db: &Database<InMemoryGraph>) -> (usize, usize) {
    db.with_store(|g| {
        let r = g.memory_estimate();
        (r.property_index_bytes, r.total_bytes())
    })
}

fn restart_mode(n: u64) {
    let tmp = std::env::var("BENCH_TMP").unwrap_or_else(|_| "/tmp/lora-storage-restart".into());
    let _ = std::fs::create_dir_all(&tmp);

    // --- snapshot -------------------------------------------------------
    let live0 = live();
    let db = Database::from_graph(build(n, Props::All, true, true));
    run(&db, "CREATE INDEX person_id FOR (p:Person) ON (p.id)");
    run(
        &db,
        "CREATE CONSTRAINT person_name_u FOR (p:Person) REQUIRE p.name IS UNIQUE",
    );
    let writer_live = live() - live0;
    let (w_pidx, w_total) = prop_idx_bytes(&db);
    let path = format!("{tmp}/restart_{n}.raw");
    db.save_snapshot_to(&path).unwrap();
    drop(db);

    let live0 = live();
    let t = Instant::now();
    let db = Database::in_memory_from_snapshot(&path).unwrap();
    let load_s = t.elapsed().as_secs_f64();
    let loaded_live = live() - live0;
    let (l_pidx, l_total) = prop_idx_bytes(&db);
    println!(
        "restart snapshot n={n} writer_live={writer_live} loaded_live={loaded_live} \
         writer_prop_idx_est={w_pidx} loaded_prop_idx_est={l_pidx} writer_est={w_total} \
         loaded_est={l_total} load_s={load_s:.3}"
    );
    drop(db);
    let _ = std::fs::remove_file(&path);

    // --- WAL --------------------------------------------------------------
    let dir = std::path::PathBuf::from(format!("{tmp}/wal_restart_{n}"));
    let _ = std::fs::remove_dir_all(&dir);
    let db = Database::open_with_wal(WalConfig::enabled(&dir)).unwrap();
    // Declare before ingest: the rel load below seeks on Person.id, and a
    // declared index keeps that from being an implicit activation.
    run(&db, "CREATE INDEX person_id FOR (p:Person) ON (p.id)");
    let batch: u64 = 10_000;
    let mut a = 0;
    while a < n {
        let b = (a + batch).min(n) - 1;
        run(
            &db,
            &format!(
                "UNWIND range({a}, {b}) AS i CREATE (:Person {{id: i, name: 'p' + toString(i), \
                 score: toFloat(i % 1000) / 7.0, bio: 'bio ' + toString(i) + ' lorem ipsum dolor sit amet, cons'}})"
            ),
        );
        a = b + 1;
    }
    let mut a = 0;
    while a < n {
        let b = (a + batch / DEGREE).min(n) - 1;
        let mut pairs = Vec::new();
        for i in a..=b {
            for k in 0..DEGREE {
                pairs.push(LoraValue::List(vec![
                    LoraValue::Int(i as i64),
                    LoraValue::Int(target(i, k, n) as i64),
                ]));
            }
        }
        q(
            &db,
            "UNWIND $pairs AS pr MATCH (x:Person {id: pr[0]}), (y:Person {id: pr[1]}) \
             CREATE (x)-[:KNOWS {w: pr[0] % 100}]->(y)",
            params(&[("pairs", LoraValue::List(pairs))]),
        );
        a = b + 1;
    }
    // Constraint declared mid-log, after the data.
    run(
        &db,
        "CREATE CONSTRAINT person_name_u FOR (p:Person) REQUIRE p.name IS UNIQUE",
    );
    db.sync().unwrap();
    let (w_pidx, w_total) = prop_idx_bytes(&db);
    let (cn, cr) = (db.node_count(), db.relationship_count());
    drop(db);

    let live0 = live();
    let t = Instant::now();
    let db =
        Database::recover(format!("{tmp}/no-such-snapshot"), WalConfig::enabled(&dir)).unwrap();
    let rec_s = t.elapsed().as_secs_f64();
    let rec_live = live() - live0;
    assert_eq!(db.node_count(), cn);
    assert_eq!(db.relationship_count(), cr);
    let (r_pidx, r_total) = prop_idx_bytes(&db);
    println!(
        "restart wal n={n} elements={} recovered_live={rec_live} writer_prop_idx_est={w_pidx} \
         recovered_prop_idx_est={r_pidx} writer_est={w_total} recovered_est={r_total} \
         replay_s={rec_s:.3}",
        cn + cr
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// idlat mode: id() lookups only (seek-by-id before/after)
// ---------------------------------------------------------------------------

fn idlat_mode(n: u64) {
    let samples: usize = std::env::var("ID_SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(if n <= 200_000 { 100 } else { 20 });
    let t = Instant::now();
    let g = build(n, Props::All, true, true);
    println!("load nodes={n} build_s={:.2}", t.elapsed().as_secs_f64());
    let db = Database::from_graph(g);
    let pid = |r: u64| LoraValue::Int((r % n) as i64);
    let rid = |r: u64| LoraValue::Int((r % (n * DEGREE)) as i64);
    let queries: Vec<(&str, &str)> = vec![
        (
            "cy_id_fn",
            "MATCH (p) WHERE id(p) = $id RETURN p.name AS name",
        ),
        (
            "cy_id_fn_label",
            "MATCH (p:Person) WHERE id(p) = $id RETURN p.name AS name",
        ),
        (
            "cy_id_in3",
            "MATCH (p) WHERE id(p) IN [$id, $id + 1, $id + 2] RETURN p.name AS name",
        ),
        ("cy_id_set", "MATCH (p) WHERE id(p) = $id SET p.score = 1.5"),
        (
            "cy_rel_id",
            "MATCH (a)-[r]->(b) WHERE id(r) = $id RETURN a.id AS a, b.id AS b",
        ),
    ];
    for (name, query) in &queries {
        println!("plan {name}: {}", plan_ops(&db, query));
    }
    for (i, (name, query)) in queries.iter().enumerate() {
        let is_rel = i == 4;
        bench(name, samples, |r| {
            let v = if is_rel {
                rid(splitmix(r))
            } else {
                pid(splitmix(r))
            };
            q(&db, query, params(&[("id", v)]));
        });
    }
}

// ---------------------------------------------------------------------------
// hop mode: the traversal reads only, repeated in rounds, then a few writes
// ---------------------------------------------------------------------------

/// `hop <N>`: build the `lat` graph, then run `ROUNDS` rounds (default 5)
/// of the point and traversal reads, so one process gives several
/// medians per row to compare A/B builds with. `WRITES=1` adds the
/// staged-write rows once at the end.
fn hop_mode(n: u64) {
    let samples: usize = std::env::var("SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20_000);
    let rounds: usize = std::env::var("ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let g = build(n, Props::All, true, true);
    let db = Database::from_graph(g);
    run(&db, "CREATE INDEX person_id FOR (p:Person) ON (p.id)");
    let pid = |r: u64| LoraValue::Int((r % n) as i64);
    let seek = "MATCH (p:Person {id: $id}) RETURN p.name AS name";
    let hop1 = "MATCH (p:Person {id: $id})-[:KNOWS]->(m) RETURN m.id AS id";
    let hop2 = "MATCH (p:Person {id: $id})-[:KNOWS]->()-[:KNOWS]->(m) RETURN m.id AS id";
    q(&db, seek, params(&[("id", pid(1))]));
    let types = vec!["KNOWS".to_string()];
    for round in 0..rounds {
        println!("round {round}");
        bench("cy_index_seek", samples, |r| {
            q(&db, seek, params(&[("id", pid(r))]));
        });
        bench("cy_1hop", samples, |r| {
            q(&db, hop1, params(&[("id", pid(r))]));
        });
        bench("cy_2hop", samples, |r| {
            q(&db, hop2, params(&[("id", pid(r))]));
        });
        let snap = db.snapshot();
        bench_batched("raw_with_node(borrow)", samples / 8, |r| {
            let v = snap.with_node(r % n, |rec| rec.properties.len()).unwrap();
            std::hint::black_box(v);
        });
        bench_batched("raw_1hop_ids", samples / 8, |r| {
            std::hint::black_box(snap.expand_ids(r % n, Direction::Right, &types));
        });
        bench_batched("raw_2hop_ids", samples / 8, |r| {
            let mut c = 0usize;
            for (_, m) in snap.expand_ids(r % n, Direction::Right, &types) {
                c += snap.expand_ids(m, Direction::Right, &types).len();
            }
            std::hint::black_box(c);
        });
    }
    if std::env::var("WRITES").is_err() {
        return;
    }
    let write_samples: usize = std::env::var("WRITE_SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000);
    let rel_q = "MATCH (a:Person {id: $a}), (b:Person {id: $b}) CREATE (a)-[:KNOWS {w: 1}]->(b)";
    bench("cy_create_rel(staged)", write_samples, |r| {
        q(
            &db,
            rel_q,
            params(&[("a", pid(r)), ("b", pid(splitmix(r)))]),
        );
    });
    let mut next = n;
    bench("db.graph_create_node(api)", write_samples, |_| {
        let mut p = BTreeMap::new();
        p.insert("id".to_string(), LoraValue::Int(next as i64));
        next += 1;
        db.graph_create_node(vec!["Person".into()], p).unwrap();
    });
    run(
        &db,
        "CREATE CONSTRAINT acct_key FOR (a:Account) REQUIRE a.key IS UNIQUE",
    );
    let set_q = "MATCH (p:Person {id: $id}) SET p.score = $v";
    bench("cy_set_prop(staged: constraint)", write_samples, |r| {
        q(
            &db,
            set_q,
            params(&[("id", pid(r)), ("v", LoraValue::Float(2.5))]),
        );
    });
}

// ---------------------------------------------------------------------------
// scan mode: scans that stop early, and one that does not
// ---------------------------------------------------------------------------

/// `scan <N>`: build the `lat` graph and time label and all-node scans
/// cut short by a `LIMIT`, plus one full filtered scan for reference.
fn scan_mode(n: u64) {
    let samples: usize = std::env::var("SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000);
    let scan_samples: usize = std::env::var("SCAN_SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(15);
    let g = build(n, Props::All, true, true);
    let db = Database::from_graph(g);
    for (name, query) in [
        (
            "cy_label_scan_limit10",
            "MATCH (p:Person) RETURN p.id AS id LIMIT 10",
        ),
        (
            "cy_all_scan_limit10",
            "MATCH (p) RETURN p.id AS id LIMIT 10",
        ),
        (
            "cy_label_scan_filter_limit10",
            "MATCH (p:Person) WHERE p.score > 0.5 RETURN p.id AS id LIMIT 10",
        ),
        (
            "cy_label_scan_skip_limit",
            "MATCH (p:Person) RETURN p.id AS id SKIP 5000 LIMIT 10",
        ),
    ] {
        bench(name, samples, |_| {
            q(&db, query, params(&[]));
        });
    }
    let s = bench("cy_label_scan_filter", scan_samples, |_| {
        q(
            &db,
            "MATCH (p:Person) WHERE p.score > 1000000.0 RETURN p.id AS id",
            params(&[]),
        );
    });
    println!(
        "info cy_label_scan_filter ns_per_node={:.2}",
        s.median / n as f64
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let _ = Arc::new(0); // keep `Arc` import meaningful under cfg churn
    let _ = Duration::ZERO;
    match args.get(1).map(String::as_str) {
        Some("mem") => mem_mode(&args[2], args[3].parse().unwrap()),
        Some("lat") => lat_mode(args[2].parse().unwrap()),
        Some("wal") => wal_mode(args[2].parse().unwrap()),
        Some("restart") => restart_mode(args[2].parse().unwrap()),
        Some("idlat") => idlat_mode(args[2].parse().unwrap()),
        Some("hop") => hop_mode(args[2].parse().unwrap()),
        Some("scan") => scan_mode(args[2].parse().unwrap()),
        _ => eprintln!(
            "usage: storage_baseline mem <variant> <N> | lat <N> | wal <N> | restart <N> | idlat <N> | hop <N> | scan <N>"
        ),
    }
}
