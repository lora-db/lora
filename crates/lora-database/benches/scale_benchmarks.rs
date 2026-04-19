//! Large-scale benchmarks to measure scaling behaviour at 10k–50k nodes.
//!
//! Run with: `cargo bench -p lora-server --bench scale_benchmarks`
//!
//! These benchmarks use longer measurement times and fewer samples because
//! each iteration is more expensive.  They are designed to reveal O(n) vs
//! O(n²) behaviour and memory-pressure effects that are invisible at small
//! scales.
//!
//! Categories:
//!   1. scale_match — full scan and filtered match at MEDIUM/LARGE
//!   2. scale_traversal — hop traversal and variable-length at scale
//!   3. scale_aggregation — grouping and aggregation at scale
//!   4. scale_ordering — sorting large result sets
//!   5. scale_write — batch insert throughput at scale
//!   6. scale_social — social graph workloads at 2k–3k

mod fixtures;

use std::hint::black_box;
use criterion::{
    criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput,
};
use lora_database::{ExecuteOptions, ResultFormat};
use fixtures::*;
use std::time::Duration;

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

/// Scale benchmarks do more work per iteration, so we allow a slightly longer
/// measurement window than engine benchmarks, but still well under the
/// Criterion default.
fn scale_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_millis(2_500))
        .sample_size(15)
}

// ===================================================================
// 1. SCALE MATCH — full scan and filtered match at large scales
// ===================================================================

fn bench_scale_match(c: &mut Criterion) {
    let mut group = c.benchmark_group("scale_match");

    for &size in &[Scale::MEDIUM, Scale::LARGE] {
        let db = build_node_graph(size);
        group.throughput(Throughput::Elements(size as u64));

        // --- full scan ---
        group.bench_with_input(
            BenchmarkId::new("full_scan", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute("MATCH (n:Node) RETURN count(n) AS c", opts())
                            .unwrap(),
                    );
                });
            },
        );

        // --- property equality (high selectivity) ---
        group.bench_with_input(
            BenchmarkId::new("property_eq", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (n:Node) WHERE n.value = 42 RETURN n.id",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );

        // --- range filter (medium selectivity) ---
        group.bench_with_input(
            BenchmarkId::new("range_filter", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (n:Node) WHERE n.value >= 40 AND n.value < 60 RETURN n.id",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );

        // --- string predicate ---
        group.bench_with_input(
            BenchmarkId::new("starts_with", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (n:Node) WHERE n.name STARTS WITH 'node_5' RETURN n.id",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );

        // --- return multiple properties ---
        group.bench_with_input(
            BenchmarkId::new("return_multi_props", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (n:Node) RETURN n.id, n.name, n.value LIMIT 1000",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );
    }

    group.finish();
}

// ===================================================================
// 2. SCALE TRAVERSAL — hop traversal at scale
// ===================================================================

fn bench_scale_traversal(c: &mut Criterion) {
    let mut group = c.benchmark_group("scale_traversal");

    // One chain per size, reused by both single_hop and varlen benches where
    // sizes overlap. Previously each loop rebuilt the chain independently.
    let chains: Vec<(usize, BenchDb)> = [2_000usize, 5_000]
        .iter()
        .map(|&s| (s, build_chain(s)))
        .collect();

    // --- single hop on large chain ---
    for (size, db) in &chains {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::new("single_hop_chain", size), size, |b, _| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (a:Chain)-[:NEXT]->(b:Chain) RETURN count(*) AS cnt",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- variable-length on large chain ---
    for (size, db) in &chains {
        group.bench_with_input(BenchmarkId::new("varlen_1_5_chain", size), size, |b, _| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (a:Chain {idx:0})-[:NEXT*1..5]->(b) RETURN count(b) AS cnt",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- large star fan-out (10k was slow to build with limited added signal
    // over 5k; a single 5k tier is enough to see linear fan-out behaviour). ---
    {
        let size = 5_000usize;
        let db = build_star(size);
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("star_fan_out", size), &size, |b, _| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (:Hub)-[:ARM]->(l:Leaf) RETURN count(l) AS cnt",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- deep tree traversal (depth 5 / branch 3 = 364 nodes, *1..5 path
    // expansion still exercises recursive traversal; the previous depth=6
    // variant (1093 nodes, *1..6) added cost without changing the shape). ---
    {
        let db = build_tree(5, 3);
        // depth=5, branch=3 → 3+9+27+81+243 = 363 descendants.
        group.throughput(Throughput::Elements(363));
        group.bench_function("tree_depth5_branch3", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (root:Tree {id:0})-[:CHILD*1..5]->(n) RETURN count(n) AS cnt",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- wide tree (depth=3, branch=10 → 1111 nodes). Kept as-is: this is
    // the one bench exercising high branching factor at a meaningful size. ---
    {
        let db = build_tree(3, 10);
        // depth=3, branch=10 → 10+100+1000 = 1110 descendants.
        group.throughput(Throughput::Elements(1110));
        group.bench_function("tree_depth3_branch10", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (root:Tree {id:0})-[:CHILD*1..3]->(n) RETURN count(n) AS cnt",
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
// 3. SCALE AGGREGATION — grouping and aggregation at scale
// ===================================================================

fn bench_scale_aggregation(c: &mut Criterion) {
    let mut group = c.benchmark_group("scale_aggregation");

    for &size in &[Scale::MEDIUM, Scale::LARGE] {
        let db = build_node_graph(size);
        group.throughput(Throughput::Elements(size as u64));

        // --- count(*) ---
        group.bench_with_input(
            BenchmarkId::new("count_star", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute("MATCH (n:Node) RETURN count(*) AS c", opts())
                            .unwrap(),
                    );
                });
            },
        );

        // --- group by (100 groups) ---
        group.bench_with_input(
            BenchmarkId::new("group_by_100_groups", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (n:Node) RETURN n.value AS grp, count(n) AS cnt",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );

        // --- multi aggregate ---
        group.bench_with_input(
            BenchmarkId::new("multi_aggregate", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (n:Node) \
                                 RETURN count(n) AS cnt, min(n.value) AS lo, \
                                        max(n.value) AS hi, sum(n.value) AS total",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );

        // --- count DISTINCT ---
        group.bench_with_input(
            BenchmarkId::new("count_distinct", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (n:Node) RETURN count(DISTINCT n.value) AS c",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );
    }

    group.finish();
}

// ===================================================================
// 4. SCALE ORDERING — sorting large result sets
// ===================================================================

fn bench_scale_ordering(c: &mut Criterion) {
    let mut group = c.benchmark_group("scale_ordering");

    for &size in &[Scale::MEDIUM, Scale::LARGE] {
        let db = build_node_graph(size);
        group.throughput(Throughput::Elements(size as u64));

        // --- ORDER BY single key ---
        group.bench_with_input(
            BenchmarkId::new("order_by_single", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (n:Node) RETURN n.id ORDER BY n.value ASC",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );

        // --- ORDER BY + LIMIT (top-N should be cheaper) ---
        group.bench_with_input(
            BenchmarkId::new("order_limit_top10", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (n:Node) RETURN n.id, n.value ORDER BY n.value DESC LIMIT 10",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );

        // --- DISTINCT ---
        group.bench_with_input(
            BenchmarkId::new("distinct", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (n:Node) RETURN DISTINCT n.value",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );

        // --- ORDER BY multi-key ---
        group.bench_with_input(
            BenchmarkId::new("order_multi_key", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (n:Node) RETURN n.id, n.value ORDER BY n.value ASC, n.id DESC",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );
    }

    group.finish();
}

// ===================================================================
// 5. SCALE WRITE — batch insert throughput at scale
// ===================================================================

fn bench_scale_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("scale_write");
    group.sample_size(10);

    // --- batch CREATE via UNWIND at various sizes ---
    // Dropped the 10k tier: it roughly doubled the group runtime without
    // revealing any new behaviour relative to the 5k tier.
    for &size in &[1000usize, 5000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::new("batch_create_unwind", size),
            &size,
            |b, &size| {
                let q = format!(
                    "UNWIND range(1, {size}) AS i CREATE (:Batch {{id: i, val: i * 2, name: 'item_' + toString(i)}})"
                );
                b.iter_batched(
                    BenchDb::new,
                    |db| {
                        black_box(db.service.execute(&q, opts()).unwrap());
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    // --- batch CREATE + relationship ---
    // Dropped the 5k tier here; the MATCH-then-CREATE edge pass is the
    // expensive part, and the 1k tier already shows its shape.
    for &size in &[500usize, 1000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::new("batch_create_chain", size),
            &size,
            |b, &size| {
                b.iter_batched(
                    BenchDb::new,
                    |db| {
                        // Create nodes
                        let q = format!(
                            "UNWIND range(0, {}) AS i CREATE (:W {{idx: i}})",
                            size - 1
                        );
                        black_box(db.service.execute(&q, opts()).unwrap());
                        // Create chain edges
                        let q2 = format!(
                            "UNWIND range(0, {}) AS i \
                             MATCH (a:W {{idx: i}}), (b:W {{idx: i + 1}}) \
                             CREATE (a)-[:LINK]->(b)",
                            size - 2
                        );
                        black_box(db.service.execute(&q2, opts()).unwrap());
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

// ===================================================================
// 6. SCALE SOCIAL — social graph workloads at 5k–10k
// ===================================================================

fn bench_scale_social(c: &mut Criterion) {
    let mut group = c.benchmark_group("scale_social");
    group.sample_size(10);

    // --- 2k social graph ---
    // Scanning the 2k Person set dominates these queries; report ops/sec as
    // "persons touched per second".
    {
        let db = build_social_graph(2000, 4);
        group.throughput(Throughput::Elements(2000));

        group.bench_function("friend_of_friend_2k", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (a:Person {id:0})-[:KNOWS]->(b)-[:KNOWS]->(c) \
                             WHERE c.id <> 0 \
                             RETURN DISTINCT c.id LIMIT 50",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });

        group.bench_function("degree_distribution_2k", |b| {
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

        group.bench_function("city_friend_count_2k", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (p:Person)-[:KNOWS]->(f:Person) \
                             RETURN p.city AS city, count(DISTINCT f) AS total_friends \
                             ORDER BY total_friends DESC",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });

        group.bench_function("mutual_friends_2k", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (a:Person {id:0})-[:KNOWS]->(m:Person)<-[:KNOWS]-(b:Person {id:100}) \
                             RETURN m.id, m.name",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- 3k social graph (down from 5k; 5k graph build dominated this group's
    // runtime and two queries at 3k still show the scaling trend vs. 2k). ---
    {
        let db = build_social_graph(3_000, 4);
        group.throughput(Throughput::Elements(3_000));

        group.bench_function("friend_of_friend_3k", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (a:Person {id:0})-[:KNOWS]->(b)-[:KNOWS]->(c) \
                             WHERE c.id <> 0 \
                             RETURN DISTINCT c.id LIMIT 50",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });

        group.bench_function("degree_distribution_3k", |b| {
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

    group.finish();
}

// ===================================================================
// Criterion harness
// ===================================================================

criterion_group! {
    name = benches;
    config = scale_config();
    targets =
        bench_scale_match,
        bench_scale_traversal,
        bench_scale_aggregation,
        bench_scale_ordering,
        bench_scale_write,
        bench_scale_social,
}
criterion_main!(benches);
