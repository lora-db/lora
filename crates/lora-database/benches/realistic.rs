//! Realistic scenario benchmarks for LoraDB.
//!
//! Run with: `cargo bench -p lora-database --bench realistic`
//!
//! These workloads combine traversal, filtering, aggregation, sorting, and
//! projection over domain-shaped fixtures. Keep broad scenario queries here so
//! `engine` can stay focused on individual physical operators.

mod fixtures;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use fixtures::*;
use lora_database::{ExecuteOptions, ResultFormat};
use std::hint::black_box;
use std::time::Duration;

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn bench_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_millis(2_000))
        .sample_size(50)
}

fn bench_realistic(c: &mut Criterion) {
    let mut group = c.benchmark_group("realistic");
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

    // ---- Pipeline queries: filter -> aggregate -> sort ----
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

criterion_group! {
    name = benches;
    config = bench_config();
    targets = bench_realistic,
}
criterion_main!(benches);
