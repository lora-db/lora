//! Query-implementation coverage benchmarks.
//!
//! This target mirrors the `crates/lora-database/tests/*.rs` query feature
//! areas with representative performance cases. It is the first place to add a
//! benchmark when a tested query-language feature gets a new implementation.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p lora-database --bench query_implementations
//! ```

mod fixtures;

use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use fixtures::*;
use lora_database::{parse_query, ExecuteOptions, LoraValue, ResultFormat};
use lora_store::LoraBinary;

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

fn int_param(name: &str, value: i64) -> BTreeMap<String, LoraValue> {
    BTreeMap::from([(name.to_string(), LoraValue::Int(value))])
}

fn string_param(name: &str, value: &str) -> BTreeMap<String, LoraValue> {
    BTreeMap::from([(name.to_string(), LoraValue::String(value.into()))])
}

fn binary_value() -> LoraValue {
    LoraValue::Binary(LoraBinary::from_segments(vec![
        vec![0, 1, 2, 3],
        vec![250, 251, 252, 253, 254, 255],
    ]))
}

fn run(db: &BenchDb, query: &str) {
    black_box(db.service.execute(query, opts()).unwrap());
}

fn run_params(db: &BenchDb, query: &str, params: BTreeMap<String, LoraValue>) {
    black_box(
        db.service
            .execute_with_params(query, opts(), params)
            .unwrap(),
    );
}

fn bench_parser_explain_profile(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/parser_explain_profile");
    group.throughput(Throughput::Elements(1));

    for (name, query) in [
        ("parse_match_return", "MATCH (n) RETURN n"),
        (
            "parse_relationship_varlen",
            "MATCH (a)-[:FOLLOWS*1..3]->(b) RETURN a, b",
        ),
        (
            "parse_write_pipeline",
            "MATCH (n) SET n.name = 'Alice' RETURN n.name AS name",
        ),
        (
            "parse_merge_on_match_on_create",
            "MERGE (n:User {name: 'Alice'}) ON MATCH SET n.age = 30 ON CREATE SET n:New",
        ),
        (
            "parse_union",
            "MATCH (a:A) RETURN a.name AS name UNION ALL MATCH (b:B) RETURN b.name AS name",
        ),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| {
                black_box(parse_query(black_box(query)).unwrap());
            });
        });
    }

    let db = build_org_graph();
    group.bench_function("explain_match_filter", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .explain(
                        "MATCH (p:Person) WHERE p.age > $age RETURN p.name AS name",
                        Some(int_param("age", 30)),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("profile_read_pipeline", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .profile(
                        "MATCH (p:Person) WHERE p.dept = 'Engineering' RETURN p.name AS name",
                        None,
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("profile_create_executes", |b| {
        b.iter_batched(
            BenchDb::new,
            |db| {
                black_box(
                    db.service
                        .profile("CREATE (:Profiled {n: 1}) RETURN 1 AS one", None)
                        .unwrap(),
                );
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_match_and_paths(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/match_paths");

    let nodes = build_node_graph(Scale::SMALL);
    group.throughput(Throughput::Elements(Scale::SMALL as u64));
    for (name, query) in [
        ("match_all_nodes", "MATCH (n:Node) RETURN n.id"),
        (
            "match_property_pattern",
            "MATCH (n:Node {value: 42}) RETURN n.id",
        ),
        (
            "match_multiple_labels",
            "MATCH (n:Node) WHERE n.value = 42 RETURN n.name",
        ),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| run(&nodes, query));
        });
    }

    let chain = build_chain(1_000);
    group.throughput(Throughput::Elements(999));
    for (name, query) in [
        (
            "directed_single_hop",
            "MATCH (a:Chain)-[:NEXT]->(b:Chain) RETURN a.idx, b.idx",
        ),
        (
            "varlen_1_5",
            "MATCH (a:Chain {idx:0})-[:NEXT*1..5]->(b) RETURN b.idx",
        ),
        (
            "varlen_unbounded",
            "MATCH (a:Chain {idx:0})-[:NEXT*]->(b) RETURN count(b) AS count",
        ),
        (
            "path_materialization",
            "MATCH p = (a:Chain {idx:0})-[:NEXT*1..3]->(b:Chain) RETURN value.size(p) AS len, path.nodes(p) AS ns",
        ),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| run(&chain, query));
        });
    }

    let cycle = build_cycle(100);
    group.throughput(Throughput::Elements(10));
    group.bench_function("varlen_cycle_terminates", |b| {
        b.iter(|| {
            run(
                &cycle,
                "MATCH (a:Ring {idx:0})-[:LOOP*1..10]->(b) RETURN b.idx",
            );
        });
    });

    group.finish();
}

fn bench_filter_projection_ordering(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/filter_projection_ordering");
    let db = build_node_graph(Scale::SMALL);
    group.throughput(Throughput::Elements(Scale::SMALL as u64));

    for (name, query) in [
        (
            "where_boolean_logic",
            "MATCH (n:Node) WHERE (n.value > 20 AND n.value < 60) OR n.id = 999 RETURN n.id",
        ),
        (
            "where_string_predicate",
            "MATCH (n:Node) WHERE n.name STARTS WITH 'node_5' RETURN n.id",
        ),
        (
            "projection_property_alias",
            "MATCH (n:Node) RETURN n.name AS name, n.value + 5 AS adjusted",
        ),
        ("projection_star", "MATCH (n:Node) RETURN * LIMIT 25"),
        (
            "ordering_multi_key",
            "MATCH (n:Node) RETURN n.id AS id, n.value AS value ORDER BY n.value DESC, n.id ASC",
        ),
        (
            "ordering_distinct_skip_limit",
            "MATCH (n:Node) RETURN DISTINCT n.value AS value ORDER BY value SKIP 10 LIMIT 25",
        ),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| run(&db, query));
        });
    }

    group.bench_function("parameter_scalar_filter", |b| {
        b.iter(|| {
            run_params(
                &db,
                "MATCH (n:Node) WHERE n.value = $value RETURN n.name AS name",
                int_param("value", 42),
            );
        });
    });

    group.bench_function("parameter_reused", |b| {
        b.iter(|| {
            run_params(
                &db,
                "MATCH (n:Node) WHERE n.name = $name OR n.name = $name RETURN n.id",
                string_param("name", "node_42"),
            );
        });
    });

    group.finish();
}

fn bench_aggregation_with_union_unwind(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/aggregation_with_union_unwind");
    let db = build_node_graph(Scale::SMALL);
    group.throughput(Throughput::Elements(Scale::SMALL as u64));

    for (name, query) in [
        ("count_star", "MATCH (n:Node) RETURN count(*) AS count"),
        (
            "multi_aggregate",
            "MATCH (n:Node) RETURN count(n) AS count, min(n.value) AS min, max(n.value) AS max, sum(n.value) AS sum",
        ),
        (
            "group_collect",
            "MATCH (n:Node) RETURN n.value AS bucket, collect(n.id) AS ids ORDER BY bucket",
        ),
        (
            "with_filter_aggregate",
            "MATCH (n:Node) WITH n.value AS bucket, count(*) AS count WHERE count > 5 RETURN bucket, count ORDER BY bucket",
        ),
        (
            "with_top_n_pipeline",
            "MATCH (n:Node) WITH n ORDER BY n.value DESC LIMIT 50 RETURN n.id AS id, n.value AS value",
        ),
        (
            "unwind_literal_pipeline",
            "UNWIND list.range(1, 1000) AS i WITH i WHERE i % 2 = 0 RETURN count(i) AS even",
        ),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| run(&db, query));
        });
    }

    group.throughput(Throughput::Elements((Scale::SMALL * 2) as u64));
    group.bench_function("union_distinct", |b| {
        b.iter(|| {
            run(
                &db,
                "MATCH (n:Node) WHERE n.value < 30 RETURN n.id AS id \
                 UNION \
                 MATCH (n:Node) WHERE n.value > 70 RETURN n.id AS id",
            );
        });
    });

    group.bench_function("union_all", |b| {
        b.iter(|| {
            run(
                &db,
                "MATCH (n:Node) WHERE n.value < 30 RETURN n.id AS id \
                 UNION ALL \
                 MATCH (n:Node) WHERE n.value > 70 RETURN n.id AS id",
            );
        });
    });

    let social = build_social_graph(200, 4);
    group.throughput(Throughput::Elements(200));
    group.bench_function("optional_match_sparse", |b| {
        b.iter(|| {
            run(
                &social,
                "MATCH (p:Person) OPTIONAL MATCH (p)-[:KNOWS]->(f:Person) RETURN p.id, count(f) AS friends",
            );
        });
    });

    group.finish();
}

fn bench_writes(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/writes");
    group.throughput(Throughput::Elements(1));

    group.bench_function("create_node", |b| {
        b.iter_batched(
            BenchDb::new,
            |db| run(&db, "CREATE (:Bench {id: 1, name: 'one'})"),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("create_relationship", |b| {
        b.iter_batched(
            || {
                let db = BenchDb::new();
                db.run("CREATE (:A {id: 1}), (:B {id: 2})");
                db
            },
            |db| run(&db, "MATCH (a:A), (b:B) CREATE (a)-[:REL {n: 1}]->(b)"),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("merge_create_node", |b| {
        b.iter_batched(
            BenchDb::new,
            |db| run(&db, "MERGE (:Singleton {key: 'unique'})"),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("merge_match_on_match", |b| {
        b.iter_batched(
            || {
                let db = BenchDb::new();
                db.run("CREATE (:Counter {name: 'hits', count: 0})");
                db
            },
            |db| {
                run(
                    &db,
                    "MERGE (c:Counter {name: 'hits'}) ON MATCH SET c.count = c.count + 1",
                );
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("merge_relationship", |b| {
        b.iter_batched(
            || {
                let db = BenchDb::new();
                db.run("CREATE (:A {id: 1}), (:B {id: 2})");
                db
            },
            |db| run(&db, "MATCH (a:A), (b:B) MERGE (a)-[:REL]->(b)"),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("set_remove_property", |b| {
        b.iter_batched(
            || {
                let db = BenchDb::new();
                db.run("CREATE (:Target {id: 1, old: true})");
                db
            },
            |db| run(&db, "MATCH (n:Target {id: 1}) SET n.val = 42 REMOVE n.old"),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("detach_delete", |b| {
        b.iter_batched(
            || build_star(5),
            |db| run(&db, "MATCH (h:Hub) DETACH DELETE h"),
            BatchSize::SmallInput,
        );
    });

    group.throughput(Throughput::Elements(100));
    group.bench_function("batch_create_unwind_100", |b| {
        b.iter_batched(
            BenchDb::new,
            |db| {
                run(
                    &db,
                    "UNWIND list.range(1, 100) AS i CREATE (:Batch {id: i})",
                )
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_expressions_and_functions(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/expressions_functions");
    let db = build_node_graph(100);
    group.throughput(Throughput::Elements(100));

    for (name, query) in [
        (
            "arithmetic_precedence",
            "MATCH (n:Node) RETURN n.id + n.value * 2 AS score",
        ),
        (
            "case_expression",
            "MATCH (n:Node) RETURN CASE WHEN n.value > 50 THEN 'high' ELSE 'low' END AS bucket",
        ),
        (
            "string_pipeline",
            "MATCH (n:Node) RETURN string.upper(string.slice(n.name, 0, 6)) AS prefix, value.size(n.name) AS len",
        ),
        (
            "math_pipeline",
            "MATCH (n:Node) RETURN math.ceil(type.cast(n.value, FLOAT) / 3.0) AS c, math.abs(n.value - 50) AS d",
        ),
        (
            "type_conversion",
            "MATCH (n:Node) RETURN type.cast(n.id, STRING) AS id, type.cast(n.value, FLOAT) AS value, type.of(n.name) AS typ",
        ),
        (
            "list_functions",
            "RETURN list.first(list.range(1, 100)), list.rest(list.range(1, 20)), value.reverse(list.range(1, 20)), value.size(list.range(1, 100))",
        ),
        (
            "list_predicates",
            "WITH list.range(1, 100) AS nums RETURN any(x IN nums WHERE x > 90), all(x IN nums WHERE x > 0), none(x IN nums WHERE x < 0)",
        ),
        (
            "regex_filter",
            "MATCH (n:Node) WHERE n.name =~ 'node_[5-9][0-9]' RETURN n.id",
        ),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| run(&db, query));
        });
    }

    group.finish();
}

fn bench_typed_values(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/typed_values");
    group.throughput(Throughput::Elements(1));

    let temporal = build_temporal_graph(100);
    group.bench_function("temporal_filter_duration", |b| {
        b.iter(|| {
            run(
                &temporal,
                "MATCH (e:Event) WHERE e.event_date >= '2024-01-15'::DATE RETURN e.name, temporal.between('2024-01-01'::DATE, e.event_date) AS age",
            );
        });
    });

    let spatial = build_spatial_graph(100);
    group.bench_function("spatial_distance_filter", |b| {
        b.iter(|| {
            run(
                &spatial,
                "MATCH (l:Location) WITH l, geo.distance(l.pos, {x: 0.0, y: 0.0}::POINT) AS dist WHERE dist < 100.0 RETURN l.name, dist ORDER BY dist LIMIT 10",
            );
        });
    });

    let vector_db = BenchDb::new();
    vector_db.run("CREATE (:Doc {id: 1, embedding: [1, 2, 3]::VECTOR<INTEGER>(3)})");
    group.bench_function("vector_construct_and_similarity", |b| {
        b.iter(|| {
            run(
                &vector_db,
                "MATCH (d:Doc) RETURN vector.similarity(d.embedding, [1, 2, 3]::VECTOR<INTEGER>(3)) AS score",
            );
        });
    });

    group.bench_function("vector_from_parameter_list", |b| {
        b.iter(|| {
            run_params(
                &vector_db,
                "RETURN $values::VECTOR<INTEGER8>(5) AS v",
                BTreeMap::from([(
                    "values".into(),
                    LoraValue::List(vec![
                        LoraValue::Int(1),
                        LoraValue::Int(2),
                        LoraValue::Int(3),
                        LoraValue::Int(4),
                        LoraValue::Int(5),
                    ]),
                )]),
            );
        });
    });

    let binary_db = BenchDb::new();
    binary_db.run_with_params(
        "CREATE (:Doc {id: 1, payload: $payload})",
        BTreeMap::from([("payload".into(), binary_value())]),
    );
    group.bench_function("binary_parameter_lookup", |b| {
        b.iter(|| {
            run_params(
                &binary_db,
                "MATCH (d:Doc {payload: $payload}) RETURN d.payload AS payload, type.of($payload) AS typ",
                BTreeMap::from([("payload".into(), binary_value())]),
            );
        });
    });

    group.finish();
}

fn bench_advanced_query_shapes(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/advanced_shapes");
    group.throughput(Throughput::Elements(1));

    let org = build_org_graph();
    for (name, query) in [
        (
            "org_managers_and_cities",
            "MATCH (m:Manager)-[:MANAGES]->(p:Person)-[:LIVES_IN]->(c:City) RETURN m.name, c.name, count(p) AS reports ORDER BY reports DESC",
        ),
        (
            "org_project_team_sizes",
            "MATCH (p:Person)-[:ASSIGNED_TO]->(pr:Project) RETURN pr.name, count(p) AS teamSize ORDER BY teamSize DESC",
        ),
        (
            "org_budget_rollup",
            "MATCH (p:Person)-[:ASSIGNED_TO]->(pr:Project) WITH p.dept AS dept, sum(pr.budget) AS budget RETURN dept, budget ORDER BY budget DESC",
        ),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| run(&org, query));
        });
    }

    let social = build_social_graph(500, 4);
    group.bench_function("social_friends_of_friends", |b| {
        b.iter(|| {
            run(
                &social,
                "MATCH (p:Person {id: 0})-[:KNOWS]->(:Person)-[:KNOWS]->(fof:Person) RETURN DISTINCT fof.id AS id",
            );
        });
    });

    let deps = build_dependency_graph(200);
    group.bench_function("dependency_transitive_dependencies", |b| {
        b.iter(|| {
            run(
                &deps,
                "MATCH (p:Package {name: 'pkg_0'})-[:DEPENDS_ON*1..5]->(dep:Package) RETURN DISTINCT dep.name AS dep",
            );
        });
    });

    let rec = build_recommendation_graph(100, 200);
    group.bench_function("recommend_collaborative_filtering", |b| {
        b.iter(|| {
            run(
                &rec,
                "MATCH (u:User {id: 0})-[:BOUGHT]->(p:Product)<-[:BOUGHT]-(other:User)-[:BOUGHT]->(rec:Product) RETURN rec.id AS product, count(other) AS score ORDER BY score DESC LIMIT 10",
            );
        });
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = bench_config();
    targets =
        bench_parser_explain_profile,
        bench_match_and_paths,
        bench_filter_projection_ordering,
        bench_aggregation_with_union_unwind,
        bench_writes,
        bench_expressions_and_functions,
        bench_typed_values,
        bench_advanced_query_shapes
}

criterion_main!(benches);
