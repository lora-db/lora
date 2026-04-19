//! Comprehensive benchmarks for the Lora engine.
//!
//! Run with: `cargo bench -p lora-server`
//!
//! Categories:
//!   1. match_queries — basic MATCH, WHERE, RETURN
//!   2. traversal — single-hop, multi-hop, variable-length
//!   3. filtering — property predicates, boolean logic, parameters
//!   4. aggregation — count, sum, collect, grouping
//!   5. ordering — ORDER BY, DISTINCT, SKIP/LIMIT
//!   6. write_operations — CREATE, MERGE, SET, DELETE
//!   7. functions — string, math, type functions
//!   8. realistic — social, org, dependency workloads

mod fixtures;

use criterion::{
    criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput,
};
use lora_database::{ExecuteOptions, ResultFormat};
use fixtures::*;
use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Duration;

/// Shared execute options (Rows format, the cheapest output path).
fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

/// Criterion defaults (3s warmup + 5s measurement × 100 samples) produced an
/// ~8s minimum per benchmark. For this engine that was an order of magnitude
/// more than needed: most queries stabilise in well under 1s of measurement.
/// This config keeps enough samples for stable timing but trims the per-bench
/// budget to ~2.5s.
fn bench_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_millis(2_000))
        .sample_size(50)
}

// ===================================================================
// 1. MATCH — basic query execution
// ===================================================================

fn bench_match_queries(c: &mut Criterion) {
    let mut group = c.benchmark_group("match");

    // Throughput unit for this group: *nodes scanned per query*. That makes
    // the `thrpt:` line read as "nodes processed per second", which is the
    // most comparable figure across scan benchmarks.

    // Build each size once, reuse across both parametric benchmarks.
    let dbs: Vec<(usize, BenchDb)> = [Scale::TINY, Scale::SMALL, Scale::MEDIUM]
        .iter()
        .map(|&s| (s, build_node_graph(s)))
        .collect();

    // --- match_all_nodes at different scales ---
    for (size, db) in &dbs {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::new("all_nodes", size), size, |b, _| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute("MATCH (n:Node) RETURN n.id", opts())
                        .unwrap(),
                );
            });
        });
    }

    // --- match with property equality filter ---
    for (size, db) in &dbs {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(
            BenchmarkId::new("property_eq_filter", size),
            size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute("MATCH (n:Node) WHERE n.value = 42 RETURN n.id", opts())
                            .unwrap(),
                    );
                });
            },
        );
    }

    // The remaining 1k benches all share the SMALL fixture — each query
    // scans SMALL nodes per iteration.
    let db_small = dbs
        .iter()
        .find(|(s, _)| *s == Scale::SMALL)
        .map(|(_, d)| d)
        .unwrap();

    group.throughput(Throughput::Elements(Scale::SMALL as u64));

    group.bench_function("range_filter_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute(
                        "MATCH (n:Node) WHERE n.value >= 20 AND n.value < 40 RETURN n.id",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("starts_with_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute(
                        "MATCH (n:Node) WHERE n.name STARTS WITH 'node_5' RETURN n.id",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("return_property_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute("MATCH (n:Node) RETURN n.name", opts())
                    .unwrap(),
            );
        });
    });

    group.bench_function("count_only_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute("MATCH (n:Node) RETURN count(n) AS c", opts())
                    .unwrap(),
            );
        });
    });

    group.finish();
}

// ===================================================================
// 2. TRAVERSAL — single-hop, multi-hop, variable-length
// ===================================================================

fn bench_traversal(c: &mut Criterion) {
    let mut group = c.benchmark_group("traversal");

    // Throughput unit for this group: *edges visited per query*. For simple
    // pattern matches this is the number of (a)-[r]->(b) pairs produced; for
    // variable-length expansion it is the number of hops the BFS performs.

    // One chain per size, reused by single_hop / varlen_1_5 / varlen_unbounded.
    let chains: Vec<(usize, BenchDb)> = [100usize, 500, 1000]
        .iter()
        .map(|&s| (s, build_chain(s)))
        .collect();

    // --- single hop on chain ---
    // Chain of N nodes has N-1 NEXT edges; the query emits N-1 rows.
    for (size, db) in &chains {
        group.throughput(Throughput::Elements((*size - 1) as u64));
        group.bench_with_input(
            BenchmarkId::new("single_hop_chain", size),
            size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (a:Chain)-[:NEXT]->(b:Chain) RETURN a.idx, b.idx",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );
    }

    // --- multi-hop fixed chain (3 hops) ---
    // One row emitted per query (single anchored 3-hop walk).
    let chain_500 = chains.iter().find(|(s, _)| *s == 500).map(|(_, d)| d).unwrap();
    group.throughput(Throughput::Elements(1));
    group.bench_function("three_hop_chain_500", |b| {
        b.iter(|| {
            black_box(
                chain_500
                    .service
                    .execute(
                        "MATCH (a:Chain {idx:0})-[:NEXT]->(b)-[:NEXT]->(c)-[:NEXT]->(d) RETURN d.idx",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- variable-length path (bounded) on chain ---
    // NEXT*1..5 from idx=0 expands to up to 5 distinct destinations.
    for (size, db) in &chains {
        let hops = 5usize.min(size.saturating_sub(1));
        group.throughput(Throughput::Elements(hops as u64));
        group.bench_with_input(
            BenchmarkId::new("varlen_1_5_chain", size),
            size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (a:Chain {idx:0})-[:NEXT*1..5]->(b) RETURN b.idx",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );
    }

    // --- variable-length unbounded on chain (tests termination) ---
    // The engine caps hops at MAX_VAR_LEN_HOPS (100); effective hops =
    // min(size - 1, 100).
    for (size, db) in chains.iter().filter(|(s, _)| *s <= 500) {
        let hops = 100usize.min(size.saturating_sub(1));
        group.throughput(Throughput::Elements(hops as u64));
        group.bench_with_input(
            BenchmarkId::new("varlen_unbounded_chain", size),
            size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (a:Chain {idx:0})-[:NEXT*]->(b) RETURN count(b) AS cnt",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );
    }

    // --- star fan-out traversal ---
    // A hub with `size` leaves produces `size` rows.
    for &size in &[100usize, 500, 1000] {
        let db = build_star(size);
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::new("star_fan_out", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (:Hub)-[:ARM]->(l:Leaf) RETURN l.id",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );
    }

    // --- cycle traversal (tests relationship dedup) ---
    // Bounded LOOP*1..10 expands to min(10, size-1) distinct destinations.
    for &size in &[50usize, 100, 500] {
        let db = build_cycle(size);
        let hops = 10.min(size.saturating_sub(1));
        group.throughput(Throughput::Elements(hops as u64));
        group.bench_with_input(
            BenchmarkId::new("cycle_varlen_bounded", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (a:Ring {idx:0})-[:LOOP*1..10]->(b) RETURN count(b) AS cnt",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );
    }

    // --- tree traversal (branching factor) ---
    {
        // depth=4, branch=3 → 3 + 9 + 27 + 81 = 120 leaves visited
        let db = build_tree(4, 3);
        group.throughput(Throughput::Elements(120));
        group.bench_function("tree_depth4_branch3_traverse", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (root:Tree {id:0})-[:CHILD*1..4]->(leaf) RETURN count(leaf) AS cnt",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }
    {
        // depth=3, branch=5 → 5 + 25 + 125 = 155 descendants visited
        let db = build_tree(3, 5);
        group.throughput(Throughput::Elements(155));
        group.bench_function("tree_depth3_branch5_traverse", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (root:Tree {id:0})-[:CHILD*1..3]->(leaf) RETURN count(leaf) AS cnt",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- social graph: 2-hop traversal ---
    {
        // 500 Person nodes with ~4-way fan out → on the order of 16 distinct
        // 2-hop destinations per starting node. Report 1 query per iteration
        // since the final DISTINCT prevents a clean work-per-row count.
        let db = build_social_graph(500, 4);
        group.throughput(Throughput::Elements(1));
        group.bench_function("social_2hop_500_nodes", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (a:Person {id:0})-[:KNOWS]->(b)-[:KNOWS]->(c) \
                             RETURN DISTINCT c.id",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    group.finish();
}

// ===================================================================
// 3. FILTERING — property predicates, boolean logic, parameters
// ===================================================================

fn bench_filtering(c: &mut Criterion) {
    let mut group = c.benchmark_group("filtering");

    let db_1k = build_node_graph(Scale::SMALL);

    // Throughput unit: *nodes scanned per query*. All 1k benches scan the
    // SMALL (1000 node) fixture, so ops/sec is reported as "nodes filtered
    // per second".
    group.throughput(Throughput::Elements(Scale::SMALL as u64));

    // --- boolean AND ---
    group.bench_function("bool_and_1k", |b| {
        b.iter(|| {
            black_box(
                db_1k
                    .service
                    .execute(
                        "MATCH (n:Node) WHERE n.value > 20 AND n.value < 60 RETURN n.id",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- boolean OR ---
    group.bench_function("bool_or_1k", |b| {
        b.iter(|| {
            black_box(
                db_1k
                    .service
                    .execute(
                        "MATCH (n:Node) WHERE n.value = 10 OR n.value = 50 OR n.value = 90 RETURN n.id",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- NOT predicate ---
    group.bench_function("bool_not_1k", |b| {
        b.iter(|| {
            black_box(
                db_1k
                    .service
                    .execute(
                        "MATCH (n:Node) WHERE NOT n.value > 50 RETURN n.id",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- IN list ---
    group.bench_function("in_list_1k", |b| {
        b.iter(|| {
            black_box(
                db_1k
                    .service
                    .execute(
                        "MATCH (n:Node) WHERE n.value IN [10, 20, 30, 40, 50] RETURN n.id",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- parameterized query ---
    group.bench_function("parameterized_eq_1k", |b| {
        let mut params = BTreeMap::new();
        params.insert("val".to_string(), lora_database::LoraValue::Int(42));
        b.iter(|| {
            black_box(
                db_1k
                    .service
                    .execute_with_params(
                        "MATCH (n:Node) WHERE n.value = $val RETURN n.id",
                        opts(),
                        params.clone(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- complex compound predicate ---
    group.bench_function("compound_predicate_1k", |b| {
        b.iter(|| {
            black_box(
                db_1k
                    .service
                    .execute(
                        "MATCH (n:Node) WHERE (n.value > 30 AND n.value < 70) OR n.id < 10 RETURN n.id",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- high-selectivity filter (few results) ---
    group.bench_function("high_selectivity_1k", |b| {
        b.iter(|| {
            black_box(
                db_1k
                    .service
                    .execute("MATCH (n:Node) WHERE n.id = 500 RETURN n", opts())
                    .unwrap(),
            );
        });
    });

    // --- low-selectivity filter (many results) ---
    group.bench_function("low_selectivity_1k", |b| {
        b.iter(|| {
            black_box(
                db_1k
                    .service
                    .execute(
                        "MATCH (n:Node) WHERE n.value >= 0 RETURN n.id",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- relationship property filter ---
    // Social graph of 200 Person nodes with fan-out 3 → ~600 KNOWS edges
    // scanned per query.
    {
        let db = build_social_graph(200, 3);
        group.throughput(Throughput::Elements(600));
        group.bench_function("rel_property_filter_200", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (a:Person)-[k:KNOWS]->(b:Person) WHERE k.strength > 5 RETURN a.id, b.id",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    group.finish();
}

// ===================================================================
// 4. AGGREGATION — count, sum, collect, grouping
// ===================================================================

fn bench_aggregation(c: &mut Criterion) {
    let mut group = c.benchmark_group("aggregation");

    // Throughput unit: *rows aggregated per query* (= nodes scanned before
    // aggregation). Lets you read ops/sec as rows consumed by the aggregator.

    // Build each scale once; reuse across all aggregation benches.
    let dbs: Vec<(usize, BenchDb)> = [Scale::TINY, Scale::SMALL, Scale::MEDIUM]
        .iter()
        .map(|&s| (s, build_node_graph(s)))
        .collect();
    let db_tiny = dbs.iter().find(|(s, _)| *s == Scale::TINY).map(|(_, d)| d).unwrap();
    let db_small = dbs.iter().find(|(s, _)| *s == Scale::SMALL).map(|(_, d)| d).unwrap();

    // --- count(*) across scales ---
    for (size, db) in &dbs {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::new("count_star", size), size, |b, _| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute("MATCH (n:Node) RETURN count(*) AS c", opts())
                        .unwrap(),
                );
            });
        });
    }

    // All subsequent 1k benches aggregate over the SMALL (1000 node) fixture.
    group.throughput(Throughput::Elements(Scale::SMALL as u64));

    group.bench_function("count_filtered_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute(
                        "MATCH (n:Node) WHERE n.value > 50 RETURN count(n) AS c",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("group_by_low_card_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute(
                        "MATCH (n:Node) RETURN n.value AS grp, count(n) AS cnt",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("multi_agg_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute(
                        "MATCH (n:Node) RETURN count(n) AS cnt, min(n.id) AS lo, max(n.id) AS hi, sum(n.value) AS total",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // collect_100 uses the TINY fixture (100 nodes).
    group.throughput(Throughput::Elements(Scale::TINY as u64));
    group.bench_function("collect_100", |b| {
        b.iter(|| {
            black_box(
                db_tiny
                    .service
                    .execute("MATCH (n:Node) RETURN collect(n.name) AS names", opts())
                    .unwrap(),
            );
        });
    });

    // Back to SMALL for the remaining 1k benches.
    group.throughput(Throughput::Elements(Scale::SMALL as u64));
    group.bench_function("group_collect_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute(
                        "MATCH (n:Node) RETURN n.value AS grp, collect(n.id) AS ids",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("count_distinct_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute(
                        "MATCH (n:Node) RETURN count(DISTINCT n.value) AS c",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // Shared social fixture for the last two benches in this group.
    // ~600 KNOWS edges scanned per iteration.
    let db_social = build_social_graph(200, 3);
    group.throughput(Throughput::Elements(600));

    group.bench_function("agg_after_traversal_200", |b| {
        b.iter(|| {
            black_box(
                db_social
                    .service
                    .execute(
                        "MATCH (p:Person)-[:KNOWS]->(f:Person) \
                         RETURN p.id AS person, count(f) AS friends",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("having_pattern_200", |b| {
        b.iter(|| {
            black_box(
                db_social
                    .service
                    .execute(
                        "MATCH (p:Person)-[:KNOWS]->(f:Person) \
                         WITH p.id AS pid, count(f) AS cnt \
                         WHERE cnt > 2 \
                         RETURN pid, cnt",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.finish();
}

// ===================================================================
// 5. ORDERING — ORDER BY, DISTINCT, SKIP/LIMIT
// ===================================================================

fn bench_ordering(c: &mut Criterion) {
    let mut group = c.benchmark_group("ordering");

    // Throughput unit: *rows sorted/deduped per query*.

    let db_tiny = build_node_graph(Scale::TINY);
    let db_small = build_node_graph(Scale::SMALL);

    for (size, db) in [(Scale::TINY, &db_tiny), (Scale::SMALL, &db_small)] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("order_by_single", size), &size, |b, _| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute("MATCH (n:Node) RETURN n.id ORDER BY n.value ASC", opts())
                        .unwrap(),
                );
            });
        });
    }

    // Remaining benches all run against the SMALL (1000 node) fixture.
    group.throughput(Throughput::Elements(Scale::SMALL as u64));

    group.bench_function("order_limit_top10_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute(
                        "MATCH (n:Node) RETURN n.id, n.value ORDER BY n.value DESC LIMIT 10",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("distinct_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute("MATCH (n:Node) RETURN DISTINCT n.value", opts())
                    .unwrap(),
            );
        });
    });

    group.bench_function("skip_limit_pagination_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute(
                        "MATCH (n:Node) RETURN n.id ORDER BY n.id SKIP 500 LIMIT 50",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("order_multi_key_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute(
                        "MATCH (n:Node) RETURN n.id, n.value ORDER BY n.value ASC, n.id DESC",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.finish();
}

// ===================================================================
// 6. WRITE OPERATIONS — CREATE, MERGE, SET, DELETE
// ===================================================================

fn bench_write_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("write");
    // Write benchmarks create fresh databases per iteration.
    // Throughput unit: *graph entities written per iteration* (nodes or
    // relationships). Single-entity writes use Elements(1); batched writes
    // scale with batch size.
    group.throughput(Throughput::Elements(1));

    // --- CREATE single node ---
    group.bench_function("create_single_node", |b| {
        b.iter_batched(
            BenchDb::new,
            |db| {
                black_box(
                    db.service
                        .execute(
                            "CREATE (:Bench {name: 'test', value: 42})",
                            opts(),
                        )
                        .unwrap(),
                );
            },
            BatchSize::SmallInput,
        );
    });

    // --- CREATE node + relationship ---
    group.bench_function("create_node_and_rel", |b| {
        b.iter_batched(
            || {
                let db = BenchDb::new();
                db.run("CREATE (:A {id:1}), (:B {id:2})");
                db
            },
            |db| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (a:A), (b:B) CREATE (a)-[:REL {weight: 1}]->(b)",
                            opts(),
                        )
                        .unwrap(),
                );
            },
            BatchSize::SmallInput,
        );
    });

    // --- batch CREATE via UNWIND ---
    for &size in &[10usize, 50, 100, 500] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::new("batch_create_unwind", size),
            &size,
            |b, &size| {
                b.iter_batched(
                    BenchDb::new,
                    |db| {
                        let q = format!(
                            "UNWIND range(1, {size}) AS i CREATE (:Batch {{id: i, val: i * 2}})"
                        );
                        black_box(db.service.execute(&q, opts()).unwrap());
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    // --- MERGE (create path) ---
    group.bench_function("merge_create_new", |b| {
        b.iter_batched(
            BenchDb::new,
            |db| {
                black_box(
                    db.service
                        .execute("MERGE (n:Singleton {key: 'unique'})", opts())
                        .unwrap(),
                );
            },
            BatchSize::SmallInput,
        );
    });

    // --- MERGE (match existing) ---
    group.bench_function("merge_match_existing", |b| {
        b.iter_batched(
            || {
                let db = BenchDb::new();
                db.run("CREATE (:Singleton {key: 'unique', count: 0})");
                db
            },
            |db| {
                black_box(
                    db.service
                        .execute(
                            "MERGE (n:Singleton {key: 'unique'}) ON MATCH SET n.count = n.count + 1",
                            opts(),
                        )
                        .unwrap(),
                );
            },
            BatchSize::SmallInput,
        );
    });

    // --- SET property ---
    group.bench_function("set_property", |b| {
        b.iter_batched(
            || {
                let db = BenchDb::new();
                db.run("CREATE (:Target {val: 0})");
                db
            },
            |db| {
                black_box(
                    db.service
                        .execute("MATCH (n:Target) SET n.val = 42", opts())
                        .unwrap(),
                );
            },
            BatchSize::SmallInput,
        );
    });

    // --- DELETE node ---
    group.bench_function("delete_node", |b| {
        b.iter_batched(
            || {
                let db = BenchDb::new();
                db.run("CREATE (:Temp {id: 1})");
                db
            },
            |db| {
                black_box(
                    db.service
                        .execute("MATCH (n:Temp) DELETE n", opts())
                        .unwrap(),
                );
            },
            BatchSize::SmallInput,
        );
    });

    // --- DETACH DELETE ---
    group.bench_function("detach_delete", |b| {
        b.iter_batched(
            || {
                let db = BenchDb::new();
                db.run("CREATE (:Hub {id: 1})");
                db.run("UNWIND range(1,5) AS i MATCH (h:Hub) CREATE (h)-[:E]->(:Leaf {id:i})");
                db
            },
            |db| {
                black_box(
                    db.service
                        .execute("MATCH (h:Hub) DETACH DELETE h", opts())
                        .unwrap(),
                );
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ===================================================================
// 7. FUNCTIONS — string, math, type
// ===================================================================

fn bench_functions(c: &mut Criterion) {
    let mut group = c.benchmark_group("functions");

    // These microbenchmarks stabilise in a few hundred nanoseconds. Trim the
    // measurement window further so we don't spend 2s per tiny function.
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_millis(1_200));

    // Default throughput unit: *one function-evaluation query per iteration*.
    // Benches that apply a function across many rows override this with the
    // row count so the reported rate becomes "function evaluations/sec".
    group.throughput(Throughput::Elements(1));

    // Previously each no-graph bench called `BenchDb::new()` inside `b.iter()`,
    // which measured fixture setup alongside the function call. Build one DB
    // per group and reuse.
    let db_empty = BenchDb::new();
    let db_tiny = build_node_graph(Scale::TINY);
    let db_org = build_org_graph();

    // --- string functions ---
    group.bench_function("string_toLower", |b| {
        b.iter(|| {
            black_box(
                db_empty
                    .service
                    .execute("RETURN toLower('HELLO WORLD') AS r", opts())
                    .unwrap(),
            );
        });
    });

    group.bench_function("string_replace", |b| {
        b.iter(|| {
            black_box(
                db_empty
                    .service
                    .execute("RETURN replace('hello world', 'world', 'bench') AS r", opts())
                    .unwrap(),
            );
        });
    });

    // Row-scaling benches: one function evaluation per scanned row.
    group.throughput(Throughput::Elements(Scale::TINY as u64));
    group.bench_function("toLower_on_100_nodes", |b| {
        b.iter(|| {
            black_box(
                db_tiny
                    .service
                    .execute("MATCH (n:Node) RETURN toLower(n.name) AS lower", opts())
                    .unwrap(),
            );
        });
    });

    // Back to 1 op per iter for the next single-RETURN bench.
    group.throughput(Throughput::Elements(1));
    group.bench_function("math_abs_sqrt", |b| {
        b.iter(|| {
            black_box(
                db_empty
                    .service
                    .execute("RETURN abs(-42) AS a, sqrt(144) AS s", opts())
                    .unwrap(),
            );
        });
    });

    group.throughput(Throughput::Elements(Scale::TINY as u64));
    group.bench_function("math_on_100_nodes", |b| {
        b.iter(|| {
            black_box(
                db_tiny
                    .service
                    .execute(
                        "MATCH (n:Node) RETURN n.id, abs(n.value - 50) AS diff",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // Org fixture: 6 WORKS_AT edges → 6 result rows.
    group.throughput(Throughput::Elements(6));
    group.bench_function("labels_keys_type", |b| {
        b.iter(|| {
            black_box(
                db_org
                    .service
                    .execute(
                        "MATCH (p:Person)-[r:WORKS_AT]->(c:Company) \
                         RETURN labels(p) AS l, keys(p) AS k, type(r) AS t",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.throughput(Throughput::Elements(Scale::TINY as u64));
    group.bench_function("case_expression_100", |b| {
        b.iter(|| {
            black_box(
                db_tiny
                    .service
                    .execute(
                        "MATCH (n:Node) RETURN CASE WHEN n.value > 50 THEN 'high' ELSE 'low' END AS tier",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.throughput(Throughput::Elements(1));
    group.bench_function("coalesce", |b| {
        b.iter(|| {
            black_box(
                db_empty
                    .service
                    .execute("RETURN coalesce(null, null, 42, 99) AS r", opts())
                    .unwrap(),
            );
        });
    });

    // list_comprehension and reduce_sum each iterate over 100 elements.
    group.throughput(Throughput::Elements(100));
    group.bench_function("list_comprehension", |b| {
        b.iter(|| {
            black_box(
                db_empty
                    .service
                    .execute(
                        "WITH range(1, 100) AS nums RETURN [x IN nums WHERE x % 2 = 0 | x * x] AS evens",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("reduce_sum", |b| {
        b.iter(|| {
            black_box(
                db_empty
                    .service
                    .execute(
                        "RETURN reduce(acc = 0, x IN range(1, 100) | acc + x) AS total",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.finish();
}

// ===================================================================
// 8. REALISTIC WORKLOADS
// ===================================================================

fn bench_realistic(c: &mut Criterion) {
    let mut group = c.benchmark_group("realistic");
    // Realistic queries do more work per iteration, so we keep the default
    // sample count (50) but don't need extra measurement time beyond the
    // group-level default.
    group.sample_size(40);

    // Throughput unit for realistic workloads: *1 complete query per iter*.
    // These queries mix match + filter + aggregate + sort, so the meaningful
    // rate is "queries per second".
    group.throughput(Throughput::Elements(1));

    // ---- Org chart: hierarchy traversal ----
    {
        let db = build_org_graph();

        group.bench_function("org_manager_subordinates", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (m:Manager)-[:MANAGES]->(e:Person) \
                             RETURN m.name AS mgr, collect(e.name) AS team",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });

        group.bench_function("org_dept_headcount", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (p:Person) \
                             RETURN p.dept AS dept, count(p) AS cnt ORDER BY cnt DESC",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });

        group.bench_function("org_people_per_city", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
                             RETURN c.name AS city, count(p) AS residents ORDER BY residents DESC",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });

        group.bench_function("org_multi_hop_mgr_project", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (m:Manager)-[:MANAGES]->(e:Person)-[:ASSIGNED_TO]->(p:Project) \
                             RETURN m.name, e.name, p.name",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // ---- Social graph: friend-of-friend, mutual connections ----
    {
        let db = build_social_graph(500, 4);

        group.bench_function("social_friend_of_friend_500", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (a:Person {id:0})-[:KNOWS]->(b)-[:KNOWS]->(c) \
                             WHERE c.id <> 0 \
                             RETURN DISTINCT c.id ORDER BY c.id LIMIT 20",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });

        group.bench_function("social_mutual_friends_500", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (a:Person {id:0})-[:KNOWS]->(m:Person)<-[:KNOWS]-(b:Person {id:10}) \
                             RETURN m.id, m.name",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });

        group.bench_function("social_influence_score_500", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (p:Person) \
                             OPTIONAL MATCH (other:Person)-[:KNOWS]->(p) \
                             RETURN p.id, count(other) AS in_degree \
                             ORDER BY in_degree DESC LIMIT 10",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });

        group.bench_function("social_common_city_friends_500", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (a:Person {id:0})-[:KNOWS]->(f:Person) \
                             WHERE f.city = a.city \
                             RETURN f.id, f.name, f.city",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // ---- Social graph at 1k scale ----
    // Only keep the degree-distribution bench at 1k: it's the one that reveals
    // different O() behaviour than the 500-node friend-of-friend above. The
    // 1k friend_of_friend variant was redundant with the 500-node one.
    {
        let db = build_social_graph(1000, 4);

        group.bench_function("social_degree_distribution_1k", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (p:Person)-[k:KNOWS]->() \
                             RETURN p.id, count(k) AS out_degree \
                             ORDER BY out_degree DESC LIMIT 20",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // // ---- Dependency graph: reachability ----
    // {
    //     let db = build_dependency_graph(200);
    //
    //     group.bench_function("dep_direct_deps_200", |b| {
    //         b.iter(|| {
    //             black_box(
    //                 db.service
    //                     .execute(
    //                         "MATCH (p:Package {id:199})-[:DEPENDS_ON]->(dep:Package) \
    //                          RETURN dep.name",
    //                         opts(),
    //                     )
    //                     .unwrap(),
    //             );
    //         });
    //     });
    //
    //     group.bench_function("dep_transitive_deps_200", |b| {
    //         b.iter(|| {
    //             black_box(
    //                 db.service
    //                     .execute(
    //                         "MATCH (p:Package {id:199})-[:DEPENDS_ON*]->(dep:Package) \
    //                          RETURN DISTINCT dep.id ORDER BY dep.id",
    //                         opts(),
    //                     )
    //                     .unwrap(),
    //             );
    //         });
    //     });
    //
    //     group.bench_function("dep_reverse_dependents_200", |b| {
    //         b.iter(|| {
    //             black_box(
    //                 db.service
    //                     .execute(
    //                         "MATCH (consumer:Package)-[:DEPENDS_ON*]->(dep:Package {id:0}) \
    //                          RETURN DISTINCT consumer.id",
    //                         opts(),
    //                     )
    //                     .unwrap(),
    //             );
    //         });
    //     });
    // }

    // ---- Pipeline queries: filter → aggregate → sort ----
    {
        let db = build_social_graph(500, 4);

        group.bench_function("pipeline_filter_agg_sort_500", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (p:Person)-[k:KNOWS]->(f:Person) \
                             WHERE k.strength > 3 \
                             WITH p.city AS city, count(f) AS strong_friends \
                             WHERE strong_friends > 1 \
                             RETURN city, strong_friends ORDER BY strong_friends DESC",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // ---- UNWIND + aggregation ----
    {
        group.bench_function("unwind_aggregate", |b| {
            let db = BenchDb::new();
            db.run("CREATE (:Data {tags: ['a','b','c','d','e']})");
            db.run("CREATE (:Data {tags: ['b','c','f']})");
            db.run("CREATE (:Data {tags: ['a','e','g','h']})");
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (d:Data) UNWIND d.tags AS tag RETURN tag, count(tag) AS cnt ORDER BY cnt DESC",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    group.finish();
}

// ===================================================================
// 9. PARSE/COMPILE OVERHEAD — isolate query planning cost
// ===================================================================

fn bench_parse_compile(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_compile");

    // Throughput unit: *one parse or full compile + execute per iteration*.
    // Report reads as "queries planned/sec" or "queries executed/sec".
    group.throughput(Throughput::Elements(1));

    let queries = [
        ("simple_match", "MATCH (n:Node) RETURN n"),
        ("match_where", "MATCH (n:Node) WHERE n.value > 10 RETURN n.id"),
        ("multi_hop", "MATCH (a:Person)-[:KNOWS]->(b)-[:KNOWS]->(c) RETURN c"),
        (
            "aggregation",
            "MATCH (n:Node) RETURN n.value AS grp, count(n) AS cnt",
        ),
        (
            "complex",
            "MATCH (p:Person)-[r:WORKS_AT]->(c:Company) \
             WHERE p.age > 30 AND r.since < 2020 \
             WITH p.name AS name, p.dept AS dept \
             RETURN dept, collect(name) AS people ORDER BY dept",
        ),
    ];

    // Measure parse-only
    for (name, query) in &queries {
        group.bench_with_input(BenchmarkId::new("parse", *name), query, |b, q| {
            let db = BenchDb::new();
            b.iter(|| {
                black_box(db.service.parse(q).unwrap());
            });
        });
    }

    // Measure parse+analyze+compile+execute on small org graph
    // (execution cost is minimal on 12 nodes; this isolates planning overhead)
    {
        let db = build_org_graph();
        let org_queries = [
            ("simple_match", "MATCH (n:Person) RETURN n"),
            ("match_where", "MATCH (n:Person) WHERE n.age > 30 RETURN n.name"),
            ("multi_hop", "MATCH (a:Person)-[:MANAGES]->(b)-[:ASSIGNED_TO]->(c) RETURN c"),
            (
                "aggregation",
                "MATCH (n:Person) RETURN n.dept AS grp, count(n) AS cnt",
            ),
            (
                "complex",
                "MATCH (p:Person)-[r:WORKS_AT]->(c:Company) \
                 WHERE p.age > 30 AND r.since < 2020 \
                 WITH p.name AS name, p.dept AS dept \
                 RETURN dept, collect(name) AS people ORDER BY dept",
            ),
        ];
        for (name, query) in &org_queries {
            group.bench_with_input(BenchmarkId::new("full_compile", *name), query, |b, q| {
                b.iter(|| {
                    black_box(db.service.execute(q, opts()).unwrap());
                });
            });
        }
    }

    group.finish();
}

// ===================================================================
// Criterion harness
// ===================================================================

criterion_group! {
    name = benches;
    config = bench_config();
    targets =
        bench_match_queries,
        bench_traversal,
        bench_filtering,
        bench_aggregation,
        bench_ordering,
        bench_write_operations,
        bench_functions,
        bench_realistic,
        bench_parse_compile,
}
criterion_main!(benches);
