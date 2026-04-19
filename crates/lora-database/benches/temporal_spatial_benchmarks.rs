//! Benchmarks for temporal operations, spatial operations, and shortest paths.
//!
//! Run with: `cargo bench -p lora-server --bench temporal_spatial_benchmarks`
//!
//! Categories:
//!   1. temporal_creation — date/time/datetime/duration constructor performance
//!   2. temporal_filtering — filtering graph data by temporal predicates
//!   3. temporal_arithmetic — date/duration arithmetic operations
//!   4. spatial_creation — point constructor performance
//!   5. spatial_distance — distance calculations (cartesian & geographic)
//!   6. spatial_filtering — filtering by spatial predicates
//!   7. shortest_path — shortestPath and allShortestPaths

mod fixtures;

use std::hint::black_box;
use criterion::{
    criterion_group, criterion_main, BenchmarkId, Criterion, Throughput,
};
use lora_database::{ExecuteOptions, ResultFormat};
use fixtures::*;
use std::time::Duration;

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

/// Same trimming as the other bench binaries: defaults cost ~8s per bench for
/// no extra signal.
fn bench_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_millis(2_000))
        .sample_size(50)
}

// ===================================================================
// 1. TEMPORAL CREATION — date/time/datetime/duration constructors
// ===================================================================

fn bench_temporal_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("temporal_creation");

    // Throughput unit: *one temporal constructor per query*. Composite
    // queries that construct several values override with their count.
    group.throughput(Throughput::Elements(1));

    // --- date from string ---
    group.bench_function("date_from_string", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN date('2024-06-15') AS d", opts())
                    .unwrap(),
            );
        });
    });

    // --- date from map ---
    group.bench_function("date_from_map", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN date({year: 2024, month: 6, day: 15}) AS d",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- time from string ---
    group.bench_function("time_from_string", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN time('14:30:00') AS t", opts())
                    .unwrap(),
            );
        });
    });

    // --- datetime from string ---
    group.bench_function("datetime_from_string", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN datetime('2024-06-15T14:30:00Z') AS dt", opts())
                    .unwrap(),
            );
        });
    });

    // --- datetime from map ---
    group.bench_function("datetime_from_map", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN datetime({year: 2024, month: 6, day: 15, hour: 14, minute: 30}) AS dt",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- duration from string ---
    group.bench_function("duration_from_string", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN duration('P1Y2M3DT4H') AS dur", opts())
                    .unwrap(),
            );
        });
    });

    // --- duration from map ---
    group.bench_function("duration_from_map", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN duration({years: 1, months: 2, days: 3, hours: 4}) AS dur",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- multiple temporal values in one query --- 4 constructors/query
    group.throughput(Throughput::Elements(4));
    group.bench_function("multi_temporal_creation", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN date('2024-01-01') AS d, \
                         time('10:30:00') AS t, \
                         datetime('2024-06-15T14:30:00Z') AS dt, \
                         duration('P30D') AS dur",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- temporal component access --- 5 property reads in date_component_access
    group.throughput(Throughput::Elements(5));
    group.bench_function("date_component_access", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "WITH date('2024-06-15') AS d \
                         RETURN d.year AS y, d.month AS m, d.day AS day, \
                                d.dayOfWeek AS dow, d.dayOfYear AS doy",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- datetime component access --- 6 property reads
    group.throughput(Throughput::Elements(6));
    group.bench_function("datetime_component_access", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "WITH datetime('2024-06-15T14:30:45Z') AS dt \
                         RETURN dt.year AS y, dt.month AS m, dt.day AS d, \
                                dt.hour AS h, dt.minute AS min, dt.second AS s",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.finish();
}

// ===================================================================
// 2. TEMPORAL FILTERING — filtering graph data by temporal predicates
// ===================================================================

fn bench_temporal_filtering(c: &mut Criterion) {
    let mut group = c.benchmark_group("temporal_filtering");

    // Throughput unit: *events filtered per query*.

    // build_temporal_graph has a heavy per-row CASE expression, so we build
    // each size once and reuse across every bench in this group.
    let temporal: Vec<(usize, BenchDb)> = [100usize, 500, 1000]
        .iter()
        .map(|&s| (s, build_temporal_graph(s)))
        .collect();
    let db_500 = temporal.iter().find(|(s, _)| *s == 500).map(|(_, d)| d).unwrap();

    // --- filter events by date comparison ---
    for (size, db) in &temporal {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::new("date_greater_than", size), size, |b, _| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (e:Event) \
                             WHERE e.event_date > date('2024-01-14') \
                             RETURN e.id, e.event_date",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // The remaining _500 benches all run on the 500-event fixture.
    group.throughput(Throughput::Elements(500));

    group.bench_function("date_range_500", |b| {
        b.iter(|| {
            black_box(
                db_500
                    .service
                    .execute(
                        "MATCH (e:Event) \
                         WHERE e.event_date >= date('2024-01-05') \
                           AND e.event_date <= date('2024-01-20') \
                         RETURN e.id, e.name",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("date_equality_500", |b| {
        b.iter(|| {
            black_box(
                db_500
                    .service
                    .execute(
                        "MATCH (e:Event) \
                         WHERE e.event_date = date('2024-01-15') \
                         RETURN e.id",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("order_by_date_500", |b| {
        b.iter(|| {
            black_box(
                db_500
                    .service
                    .execute(
                        "MATCH (e:Event) \
                         RETURN e.id, e.event_date \
                         ORDER BY e.event_date DESC LIMIT 20",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("group_by_priority_500", |b| {
        b.iter(|| {
            black_box(
                db_500
                    .service
                    .execute(
                        "MATCH (e:Event) \
                         RETURN e.priority AS prio, count(e) AS cnt \
                         ORDER BY prio",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- temporal component access on inline values --- 28 dates unwound.
    {
        let db = BenchDb::new();
        group.throughput(Throughput::Elements(28));
        group.bench_function("date_component_inline", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "UNWIND range(1, 28) AS d \
                             WITH date('2024-01-' + CASE WHEN d < 10 THEN '0' + toString(d) ELSE toString(d) END) AS dt \
                             RETURN dt.year AS y, dt.month AS m, dt.day AS day",
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
// 3. TEMPORAL ARITHMETIC — date/duration arithmetic operations
// ===================================================================

fn bench_temporal_arithmetic(c: &mut Criterion) {
    let mut group = c.benchmark_group("temporal_arithmetic");

    // Throughput unit: *one temporal arithmetic op per iteration* for scalar
    // benches; graph benches override with row count.
    group.throughput(Throughput::Elements(1));

    // --- date + duration ---
    group.bench_function("date_plus_duration", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN date('2024-01-15') + duration('P30D') AS future_date",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- date - duration ---
    group.bench_function("date_minus_duration", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN date('2024-06-15') - duration('P2M') AS past_date",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- duration between dates ---
    group.bench_function("duration_between", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN duration.between(date('2024-01-01'), date('2024-12-31')) AS span",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- duration arithmetic ---
    group.bench_function("duration_add", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN duration('P1Y') + duration('P6M') AS total",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // Share the 200-node temporal graph across the two graph-bound benches
    // in this group (previously rebuilt twice).
    let db_graph = build_temporal_graph(200);
    group.throughput(Throughput::Elements(200));

    group.bench_function("date_arithmetic_on_graph_200", |b| {
        b.iter(|| {
            black_box(
                db_graph
                    .service
                    .execute(
                        "MATCH (e:Event) \
                         RETURN e.id, e.event_date + duration('P7D') AS next_week",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("datetime_plus_duration_200", |b| {
        b.iter(|| {
            black_box(
                db_graph
                    .service
                    .execute(
                        "MATCH (e:Event) \
                         RETURN e.id, e.created_at + duration('P30D') AS expiry",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.finish();
}

// ===================================================================
// 4. SPATIAL CREATION — point constructor performance
// ===================================================================

fn bench_spatial_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("spatial_creation");

    // Throughput unit: *one point construction per iteration*.
    group.throughput(Throughput::Elements(1));

    // --- cartesian point ---
    group.bench_function("point_cartesian", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN point({x: 3.0, y: 4.0}) AS p", opts())
                    .unwrap(),
            );
        });
    });

    // --- geographic point ---
    group.bench_function("point_geographic", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN point({latitude: 48.8566, longitude: 2.3522}) AS p",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- point component access --- 3 property reads
    group.throughput(Throughput::Elements(3));
    group.bench_function("point_component_access", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "WITH point({latitude: 48.8566, longitude: 2.3522}) AS p \
                         RETURN p.latitude AS lat, p.longitude AS lon, p.srid AS srid",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- multiple point creation --- 3 constructions/query
    group.throughput(Throughput::Elements(3));
    group.bench_function("multi_point_creation", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN point({x: 1.0, y: 2.0}) AS p1, \
                                point({x: 3.0, y: 4.0}) AS p2, \
                                point({latitude: 51.5, longitude: -0.1}) AS p3",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.finish();
}

// ===================================================================
// 5. SPATIAL DISTANCE — distance calculations
// ===================================================================

fn bench_spatial_distance(c: &mut Criterion) {
    let mut group = c.benchmark_group("spatial_distance");

    // Throughput unit: *one distance calculation per iteration* for scalar
    // benches; graph benches override with the number of edges they traverse.
    group.throughput(Throughput::Elements(1));

    // --- cartesian distance ---
    group.bench_function("distance_cartesian", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN distance(point({x: 0.0, y: 0.0}), point({x: 3.0, y: 4.0})) AS d",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- geographic distance (haversine) ---
    group.bench_function("distance_geographic", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN distance(\
                           point({latitude: 48.8566, longitude: 2.3522}), \
                           point({latitude: 51.5074, longitude: -0.1278})\
                         ) AS d",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- distance on graph data ---
    for &size in &[100usize, 500] {
        let db = build_spatial_graph(size);
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::new("pairwise_distance_graph", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (a:Location)-[:CONNECTS_TO]->(b:Location) \
                                 RETURN a.id, b.id, distance(a.pos, b.pos) AS dist",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );
    }

    // --- geographic distance on graph --- 200 CONNECTS_TO edges.
    {
        let db = build_spatial_graph(200);
        group.throughput(Throughput::Elements(200));
        group.bench_function("geo_distance_graph_200", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (a:Location)-[:CONNECTS_TO]->(b:Location) \
                             RETURN a.id, b.id, distance(a.geo, b.geo) AS meters",
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
// 6. SPATIAL FILTERING — filtering by spatial predicates
// ===================================================================

fn bench_spatial_filtering(c: &mut Criterion) {
    let mut group = c.benchmark_group("spatial_filtering");

    // Throughput unit: *Location nodes scanned per query*.

    // 200-node spatial graph was previously built twice; share it.
    let db_200 = build_spatial_graph(200);
    let db_500 = build_spatial_graph(500);

    group.throughput(Throughput::Elements(200));
    group.bench_function("distance_threshold_200", |b| {
        b.iter(|| {
            black_box(
                db_200
                    .service
                    .execute(
                        "MATCH (a:Location {id: 0}), (b:Location) \
                         WHERE a <> b AND distance(a.pos, b.pos) < 20.0 \
                         RETURN b.id, b.name",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.throughput(Throughput::Elements(200));
    group.bench_function("nearest_sorted_200", |b| {
        b.iter(|| {
            black_box(
                db_200
                    .service
                    .execute(
                        "MATCH (a:Location {id: 0}), (b:Location) \
                         WHERE a <> b \
                         RETURN b.id, distance(a.pos, b.pos) AS dist \
                         ORDER BY dist ASC LIMIT 10",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.throughput(Throughput::Elements(500));
    group.bench_function("category_distance_filter_500", |b| {
        b.iter(|| {
            black_box(
                db_500
                    .service
                    .execute(
                        "MATCH (a:Location {id: 0}), (b:Location) \
                         WHERE b.category = 'restaurant' \
                           AND distance(a.pos, b.pos) < 30.0 \
                         RETURN b.id, b.name",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.finish();
}

// ===================================================================
// 7. SHORTEST PATH — shortestPath and allShortestPaths
// ===================================================================

fn bench_shortest_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("shortest_path");
    // Shortest-path iterations are by far the slowest in this binary.
    // Fewer samples at this level still give enough statistical power because
    // per-iteration variance is low on deterministic graphs.
    group.sample_size(30);
    group.measurement_time(Duration::from_millis(2_500));

    // Throughput unit: *one shortest-path query per iteration* (a single path
    // is produced). Reported as "paths/sec".
    group.throughput(Throughput::Elements(1));

    // --- shortestPath on chain (trivial: only one path) ---
    for &size in &[100usize, 500] {
        let db = build_chain(size);
        group.bench_with_input(
            BenchmarkId::new("shortest_chain", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH p = shortestPath((a:Chain {idx:0})-[:NEXT*]->(b:Chain {idx:10})) \
                                 RETURN length(p) AS len",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );
    }

    // --- shortestPath on social graph (bounded to prevent explosion) ---
    for &size in &[100usize, 200] {
        let db = build_social_graph(size, 3);
        group.bench_with_input(
            BenchmarkId::new("shortest_social_bounded", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH p = shortestPath((a:Person {id:0})-[:KNOWS*1..6]->(b:Person {id:10})) \
                                 RETURN length(p) AS len",
                                opts(),
                            )
                            .unwrap(),
                    );
                });
            },
        );
    }

    // --- allShortestPaths on social graph (bounded) ---
    {
        let db = build_social_graph(100, 3);
        group.bench_function("all_shortest_social_100", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH p = allShortestPaths((a:Person {id:0})-[:KNOWS*1..6]->(b:Person {id:10})) \
                             RETURN length(p) AS len",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- shortestPath on tree (well-defined depth) ---
    {
        let db = build_tree(4, 3);
        group.bench_function("shortest_tree_depth4_branch3", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH p = shortestPath((root:Tree {id:0})-[:CHILD*1..4]->(leaf:Tree {depth:4})) \
                             RETURN length(p) AS len",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- shortestPath on dependency graph (bounded) ---
    {
        let db = build_dependency_graph(100);
        group.bench_function("shortest_dep_graph_100", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH p = shortestPath((a:Package {id:99})-[:DEPENDS_ON*1..10]->(b:Package {id:0})) \
                             RETURN length(p) AS len",
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
    config = bench_config();
    targets =
        bench_temporal_creation,
        bench_temporal_filtering,
        bench_temporal_arithmetic,
        bench_spatial_creation,
        bench_spatial_distance,
        bench_spatial_filtering,
        bench_shortest_path,
}
criterion_main!(benches);
