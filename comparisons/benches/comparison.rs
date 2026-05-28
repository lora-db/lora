//! Workload-driven graph DB comparison harness.
//!
//! The catalogue of benchmarks lives in `benches/workloads.yml` — this
//! file is just the runner that:
//!
//! 1. Loads the workload list from YAML.
//! 2. Walks the engine registry from `comparison::engine::registry()`.
//! 3. For every (workload, engine) pair where the engine has a query
//!    string, sets up the workload's fixture and runs the bench using
//!    the workload's iteration mode.
//!
//! Adding a new database system: implement an engine module under
//! `src/engines/`, add a variant to `DbHandle`, and register it in
//! `registry()`. Every workload that already has a query for that
//! engine name then runs against it on the next bench invocation.
//!
//! Adding a new feature: append one entry to `workloads.yml`. No Rust
//! changes required.
//!
//! Run with:
//!   cd comparisons && cargo bench --bench comparison
//!
//! Filter to a subset (criterion --filter):
//!   cargo bench --bench comparison -- traversals/
//!
//! Override the default fixture size:
//!   LORA_VS_GRAFEO_SCALE=200 cargo bench --bench comparison

use std::hint::black_box;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::Duration;

use criterion::measurement::WallTime;
use criterion::{
    criterion_group, criterion_main, BatchSize, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};

use comparison::engine::{registry, DbHandle, EngineSpec};
use comparison::fixtures::Fixture;
use comparison::workload::{load_workloads, IterMode, ThroughputKind, Workload, WorkloadFile};

fn workloads_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("benches");
    p.push("workloads.yml");
    p
}

fn bench_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
        .sample_size(20)
        .noise_threshold(0.05)
}

/// Resolve the working size for a workload.
///
///   * an explicit `size:` on the workload wins outright (e.g.
///     `variable_length_path: 100`).
///   * otherwise the global scale comes from `LORA_VS_GRAFEO_SCALE` if
///     set, else `defaults.size` from the YAML, else 1000.
///   * the per-fixture cap in `defaults.fixture_size` is then applied
///     so chain/social workloads stay tractable at large scale settings.
fn effective_size(file: &WorkloadFile, w: &Workload) -> usize {
    if let Some(s) = w.size {
        return s;
    }
    let scale = std::env::var("LORA_VS_GRAFEO_SCALE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .or(file.defaults.size)
        .unwrap_or(1_000);
    match file.fixture_cap(w.fixture) {
        Some(cap) => scale.min(cap),
        None => scale,
    }
}

fn substitute(template: &str, size: usize) -> String {
    template
        .replace("${size}", &size.to_string())
        .replace("${half_size}", &(size / 2).to_string())
        .replace("${size_plus_1}", &(size + 1).to_string())
        .replace("${size_minus_1}", &(size.saturating_sub(1).to_string()))
}

fn apply_throughput(g: &mut BenchmarkGroup<'_, WallTime>, kind: ThroughputKind, size: usize) {
    let n = match kind {
        ThroughputKind::None => return,
        ThroughputKind::Elements => size,
        ThroughputKind::EdgesMinus1 => size.saturating_sub(1),
        ThroughputKind::EdgesMinus2 => size.saturating_sub(2),
        ThroughputKind::EdgesMinus3 => size.saturating_sub(3),
        ThroughputKind::Single => 1,
    };
    g.throughput(Throughput::Elements(n as u64));
}

fn run_one(c: &mut Criterion, w: &Workload, engine: &EngineSpec, query: &str, size: usize) {
    let group_name = format!("{}/{}", w.group, w.id);
    let mut g = c.benchmark_group(&group_name);
    apply_throughput(&mut g, w.throughput, size);

    let fixture = Fixture {
        kind: w.fixture,
        size,
    };

    // Each engine's setup + measurement is wrapped in `catch_unwind` so a
    // single (workload, engine) pair that an engine can't handle — e.g. a
    // Cypher server inheriting a Grafeo-dialect query via the alias, or a
    // function an engine doesn't implement — is skipped with a SKIP log
    // rather than aborting the whole suite. The group is always finished
    // afterwards so criterion's bookkeeping stays consistent.
    let outcome = catch_unwind(AssertUnwindSafe(|| match w.iter {
        IterMode::Construct => {
            g.bench_function(engine.name, |b| {
                b.iter(|| {
                    let h: DbHandle = (engine.fresh)();
                    black_box(h);
                });
            });
        }
        IterMode::Read => {
            let mut handle = (engine.fresh)();
            (engine.seed)(&mut handle, fixture);
            apply_indexes(engine, &handle, w);
            g.bench_function(BenchmarkId::new(engine.name, size), |b| {
                b.iter(|| (engine.execute)(&handle, query));
            });
        }
        IterMode::PerIterQuery => {
            let fresh_fn = engine.fresh;
            let seed_fn = engine.seed;
            let index_fn = engine.create_index;
            let exec_fn = engine.execute;
            let indexes: Vec<String> = lookup_indexes(engine, w);
            g.bench_function(BenchmarkId::new(engine.name, size), |b| {
                b.iter_batched(
                    || {
                        let mut h = fresh_fn();
                        seed_fn(&mut h, fixture);
                        for prop in &indexes {
                            index_fn(&h, prop);
                        }
                        h
                    },
                    |handle| {
                        exec_fn(&handle, query);
                        black_box(handle);
                    },
                    BatchSize::PerIteration,
                );
            });
        }
        IterMode::PerIterSeed => {
            let fresh_fn = engine.fresh;
            let seed_fn = engine.seed;
            g.bench_function(BenchmarkId::new(engine.name, size), |b| {
                b.iter_batched(
                    fresh_fn,
                    |mut handle| {
                        seed_fn(&mut handle, fixture);
                        black_box(handle);
                    },
                    BatchSize::PerIteration,
                );
            });
        }
    }));

    g.finish();

    if let Err(payload) = outcome {
        // The adapters panic with a descriptive `String` (or `&str`);
        // collapse it to a single capped line so the bench log stays
        // readable across the whole matrix.
        let raw = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("<non-string panic>");
        let msg: String = raw
            .lines()
            .next()
            .unwrap_or(raw)
            .chars()
            .take(200)
            .collect();
        eprintln!("SKIP {}/{} [{}]: {}", w.group, w.id, engine.name, msg);
    }
}

fn apply_indexes(engine: &EngineSpec, handle: &DbHandle, w: &Workload) {
    for p in lookup_indexes(engine, w) {
        (engine.create_index)(handle, &p);
    }
}

/// Index lookup mirrors the query lookup: prefer an entry under the
/// engine's own name, fall back to the `query_alias` (so persistent
/// variants share the in-memory engine's index list).
fn lookup_indexes(engine: &EngineSpec, w: &Workload) -> Vec<String> {
    if let Some(v) = w.indexes.get(engine.name) {
        return v.clone();
    }
    if let Some(alias) = engine.query_alias {
        if let Some(v) = w.indexes.get(alias) {
            return v.clone();
        }
    }
    Vec::new()
}

fn run_workload(c: &mut Criterion, file: &WorkloadFile, w: &Workload, engines: &[EngineSpec]) {
    let size = effective_size(file, w);
    for engine in engines {
        // A `notes` entry for this engine declares "skip, here's why" —
        // suppresses both an explicit miss and the alias fallback. The
        // report surfaces the note so the omission is visible.
        if w.notes.contains_key(engine.name) {
            continue;
        }
        let Some(template) = w
            .queries
            .get(engine.name)
            .or_else(|| engine.query_alias.and_then(|a| w.queries.get(a)))
        else {
            continue;
        };
        let q = substitute(template, size);
        run_one(c, w, engine, &q, size);
    }
}

fn run_all(c: &mut Criterion) {
    // Silence the default panic hook so the expected per-pair skips don't
    // spam backtraces; `run_one` logs a single SKIP line with the reason
    // extracted from the panic payload instead.
    std::panic::set_hook(Box::new(|_| {}));
    let file = load_workloads(&workloads_path());
    let engines = registry();
    for w in &file.workloads {
        run_workload(c, &file, w, &engines);
    }
}

criterion_group! {
    name = benches;
    config = bench_config();
    targets = run_all,
}
criterion_main!(benches);
