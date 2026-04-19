//! Benchmarks for advanced query features and additional function coverage.
//!
//! Run with: `cargo bench -p lora-server --bench advanced_benchmarks`
//!
//! Categories:
//!   1. union_queries — UNION / UNION ALL
//!   2. optional_match — OPTIONAL MATCH (isolated)
//!   3. list_predicates — any(), all(), none(), single()
//!   4. string_functions — substring, split, trim, toUpper, left, right, etc.
//!   5. math_functions — ceil, floor, round, sign, pow, log, trig
//!   6. type_conversion — toInteger, toFloat, toBoolean, toString, valueType
//!   7. list_functions — head, tail, last, reverse, range, size
//!   8. path_functions — nodes(), relationships(), length() on paths
//!   9. regex_matching — =~ operator
//!  10. with_piping — multipart queries with WITH chaining
//!  11. recommendation — realistic recommendation/e-commerce workloads

mod fixtures;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use fixtures::*;
use lora_database::{ExecuteOptions, ResultFormat};
use std::hint::black_box;
use std::time::Duration;

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

/// Same timing rationale as `engine_benchmarks::bench_config`: default
/// Criterion timing was an order of magnitude more than needed.
fn bench_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_millis(2_000))
        .sample_size(50)
}

// ===================================================================
// 1. UNION — UNION / UNION ALL
// ===================================================================

fn bench_union(c: &mut Criterion) {
    let mut group = c.benchmark_group("union");

    // Throughput unit: *rows feeding the UNION per query*. For the 1k benches
    // that is Scale::SMALL nodes scanned per branch × 2 branches.
    group.throughput(Throughput::Elements((Scale::SMALL as u64) * 2));

    // --- simple UNION (deduplicating) ---
    {
        let db = build_node_graph(Scale::SMALL);
        group.bench_function("union_two_queries_1k", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (n:Node) WHERE n.value < 30 RETURN n.id AS id \
                             UNION \
                             MATCH (n:Node) WHERE n.value > 70 RETURN n.id AS id",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- UNION ALL (no dedup) ---
    {
        let db = build_node_graph(Scale::SMALL);
        group.bench_function("union_all_two_queries_1k", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (n:Node) WHERE n.value < 30 RETURN n.id AS id \
                             UNION ALL \
                             MATCH (n:Node) WHERE n.value > 70 RETURN n.id AS id",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- UNION with different labels ---
    // Org fixture has 6 Person + 3 City + 2 Project = 11 rows unioned.
    {
        let db = build_org_graph();
        group.throughput(Throughput::Elements(9));
        group.bench_function("union_different_labels", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (p:Person) RETURN p.name AS name \
                             UNION \
                             MATCH (c:City) RETURN c.name AS name",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- triple UNION ---
    // 6 Person + 3 City + 2 Project = 11 rows across 3 branches.
    {
        let db = build_org_graph();
        group.throughput(Throughput::Elements(11));
        group.bench_function("union_triple", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (p:Person) RETURN p.name AS name, 'person' AS kind \
                             UNION ALL \
                             MATCH (c:City) RETURN c.name AS name, 'city' AS kind \
                             UNION ALL \
                             MATCH (pr:Project) RETURN pr.name AS name, 'project' AS kind",
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
// 2. OPTIONAL MATCH — isolated benchmarks
// ===================================================================

fn bench_optional_match(c: &mut Criterion) {
    let mut group = c.benchmark_group("optional_match");

    // Throughput unit: *rows produced by the outer MATCH*. OPTIONAL MATCH
    // scales with that outer cardinality.

    // --- OPTIONAL MATCH where most match ---
    // 200 Person nodes in the outer MATCH.
    {
        let db = build_social_graph(200, 4);
        group.throughput(Throughput::Elements(200));
        group.bench_function("optional_mostly_matched_200", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (p:Person) \
                             OPTIONAL MATCH (p)-[:KNOWS]->(f:Person) \
                             RETURN p.id, count(f) AS friends",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- OPTIONAL MATCH where many are null ---
    // 6 Person nodes from the org graph.
    {
        let db = build_org_graph();
        group.throughput(Throughput::Elements(6));
        group.bench_function("optional_sparse_match", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (p:Person) \
                             OPTIONAL MATCH (p)-[:MANAGES]->(sub:Person) \
                             RETURN p.name, sub.name",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- multiple OPTIONAL MATCH ---
    {
        let db = build_org_graph();
        group.throughput(Throughput::Elements(6));
        group.bench_function("double_optional_match", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (p:Person) \
                             OPTIONAL MATCH (p)-[:MANAGES]->(sub:Person) \
                             OPTIONAL MATCH (p)-[:ASSIGNED_TO]->(pr:Project) \
                             RETURN p.name, sub.name, pr.name",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- OPTIONAL MATCH at scale ---
    // Dropped the 1k tier: OPTIONAL MATCH over KNOWS gets expensive per-iter
    // and the 200/500 pair already captures the scaling trend.
    for &size in &[200usize, 500] {
        let db = build_social_graph(size, 4);
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::new("optional_match_scale", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(
                        db.service
                            .execute(
                                "MATCH (p:Person) \
                                 OPTIONAL MATCH (p)-[:KNOWS]->(f:Person) \
                                 WHERE f.age > 40 \
                                 RETURN p.id, collect(f.id) AS older_friends",
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
// 3. LIST PREDICATES — any(), all(), none(), single()
// ===================================================================

fn bench_list_predicates(c: &mut Criterion) {
    let mut group = c.benchmark_group("list_predicates");

    // Throughput unit: *list elements evaluated per iteration*. The small
    // literal-list benches use the list length; graph-backed benches use
    // the row count.

    // --- any() predicate ---  5 items
    group.throughput(Throughput::Elements(5));
    group.bench_function("any_in_list", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "WITH [1, 2, 3, 4, 5] AS nums \
                         RETURN any(x IN nums WHERE x > 3) AS has_large",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- all() predicate --- 4 items
    group.throughput(Throughput::Elements(4));
    group.bench_function("all_in_list", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "WITH [2, 4, 6, 8] AS nums \
                         RETURN all(x IN nums WHERE x % 2 = 0) AS all_even",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- none() predicate --- 4 items
    group.throughput(Throughput::Elements(4));
    group.bench_function("none_in_list", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "WITH [1, 3, 5, 7] AS nums \
                         RETURN none(x IN nums WHERE x % 2 = 0) AS no_even",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- single() predicate --- 5 items
    group.throughput(Throughput::Elements(5));
    group.bench_function("single_in_list", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "WITH [1, 2, 3, 4, 5] AS nums \
                         RETURN single(x IN nums WHERE x = 3) AS exactly_one",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- list predicates on graph data ---
    // Aggregation runs over 200 Person nodes.
    {
        let db = build_social_graph(200, 4);
        group.throughput(Throughput::Elements(200));
        group.bench_function("any_on_graph_200", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (p:Person)-[:KNOWS]->(f:Person) \
                             WITH p, collect(f.age) AS friend_ages \
                             WHERE any(a IN friend_ages WHERE a > 50) \
                             RETURN p.id, p.name",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- list comprehension with filter --- 200 elements iterated
    group.throughput(Throughput::Elements(200));
    group.bench_function("comprehension_filter_transform", |b| {
        let db = BenchDb::new();
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "WITH range(1, 200) AS nums \
                         RETURN [x IN nums WHERE x % 3 = 0 | x * x] AS squares",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- reduce over large list ---
    // 100/500 are enough to show the linear trend; 1k doubled the runtime
    // without adding a new data point.
    for &size in &[100usize, 500] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("reduce_sum", size), &size, |b, &size| {
            let db = BenchDb::new();
            let q = format!("RETURN reduce(acc = 0, x IN range(1, {size}) | acc + x) AS total");
            b.iter(|| {
                black_box(db.service.execute(&q, opts()).unwrap());
            });
        });
    }

    group.finish();
}

// ===================================================================
// 4. STRING FUNCTIONS — extended coverage
// ===================================================================

fn bench_string_functions(c: &mut Criterion) {
    let mut group = c.benchmark_group("string_functions");

    // Throughput unit: *one function-evaluation query per iteration*. The
    // `*_on_graph` bench at the end overrides this with the row count.
    group.throughput(Throughput::Elements(1));

    let db = BenchDb::new();

    group.bench_function("toUpper", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN toUpper('hello world') AS r", opts())
                    .unwrap(),
            );
        });
    });

    group.bench_function("trim", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN trim('  hello  ') AS r", opts())
                    .unwrap(),
            );
        });
    });

    group.bench_function("ltrim_rtrim", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN ltrim('  hello') AS l, rtrim('hello  ') AS r",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("substring", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN substring('hello world', 6, 5) AS r", opts())
                    .unwrap(),
            );
        });
    });

    group.bench_function("split", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN split('a,b,c,d,e', ',') AS parts", opts())
                    .unwrap(),
            );
        });
    });

    group.bench_function("left_right", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN left('hello world', 5) AS l, right('hello world', 5) AS r",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("reverse_string", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN reverse('abcdefghij') AS r", opts())
                    .unwrap(),
            );
        });
    });

    group.bench_function("char_length", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN char_length('hello world') AS len", opts())
                    .unwrap(),
            );
        });
    });

    // --- string functions on graph data --- 100 function evaluations/query
    {
        let db_graph = build_node_graph(Scale::TINY);
        group.throughput(Throughput::Elements(Scale::TINY as u64));
        group.bench_function("string_pipeline_100_nodes", |b| {
            b.iter(|| {
                black_box(
                    db_graph
                        .service
                        .execute(
                            "MATCH (n:Node) \
                             RETURN toUpper(n.name) AS upper, \
                                    substring(n.name, 0, 4) AS prefix, \
                                    size(n.name) AS len",
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
// 5. MATH FUNCTIONS — extended coverage including trig
// ===================================================================

fn bench_math_functions(c: &mut Criterion) {
    let mut group = c.benchmark_group("math_functions");

    // Throughput unit: *one function-evaluation query per iteration*. The
    // `*_on_graph` bench overrides this with the row count.
    group.throughput(Throughput::Elements(1));

    let db = BenchDb::new();

    // --- rounding functions ---
    group.bench_function("ceil_floor_round", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN ceil(3.14) AS c, floor(3.14) AS f, round(3.5) AS r",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("sign", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN sign(-42) AS neg, sign(0) AS zero, sign(42) AS pos",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("exp_log", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN exp(1) AS e, log(10) AS ln, log10(100) AS lg",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- trig functions ---
    group.bench_function("trig_sin_cos_tan", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN sin(1.0) AS s, cos(1.0) AS c, tan(0.5) AS t", opts())
                    .unwrap(),
            );
        });
    });

    group.bench_function("trig_inverse", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN asin(0.5) AS as, acos(0.5) AS ac, atan(1.0) AS at, atan2(1.0, 1.0) AS at2",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("degrees_radians", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN degrees(pi()) AS deg, radians(180) AS rad, pi() AS pi, e() AS euler",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- math on graph data --- 100 function evaluations/query
    {
        let db_graph = build_node_graph(Scale::TINY);
        group.throughput(Throughput::Elements(Scale::TINY as u64));
        group.bench_function("math_pipeline_100_nodes", |b| {
            b.iter(|| {
                black_box(
                    db_graph
                        .service
                        .execute(
                            "MATCH (n:Node) \
                             RETURN n.id, \
                                    ceil(toFloat(n.value) / 3.0) AS bucket, \
                                    sqrt(toFloat(n.value)) AS root, \
                                    sign(n.value - 50) AS half",
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
// 6. TYPE CONVERSION — toInteger, toFloat, toBoolean, toString, valueType
// ===================================================================

fn bench_type_conversion(c: &mut Criterion) {
    let mut group = c.benchmark_group("type_conversion");

    // Throughput unit: *one conversion query per iteration* for scalar
    // benches, 100 rows for the graph bench.
    group.throughput(Throughput::Elements(1));

    let db = BenchDb::new();

    group.bench_function("toInteger_from_string", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN toInteger('42') AS i", opts())
                    .unwrap(),
            );
        });
    });

    group.bench_function("toFloat_from_string", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN toFloat('3.14') AS f", opts())
                    .unwrap(),
            );
        });
    });

    group.bench_function("toBoolean_from_string", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN toBoolean('true') AS b", opts())
                    .unwrap(),
            );
        });
    });

    group.bench_function("toString_from_int", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN toString(42) AS s", opts())
                    .unwrap(),
            );
        });
    });

    group.bench_function("valueType", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN valueType(42) AS t1, valueType('hello') AS t2, \
                                valueType(3.14) AS t3, valueType(true) AS t4",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // --- type conversion on graph data --- 100 conversions/query
    {
        let db_graph = build_node_graph(Scale::TINY);
        group.throughput(Throughput::Elements(Scale::TINY as u64));
        group.bench_function("conversions_on_100_nodes", |b| {
            b.iter(|| {
                black_box(
                    db_graph
                        .service
                        .execute(
                            "MATCH (n:Node) \
                             RETURN toString(n.id) AS sid, \
                                    toFloat(n.value) AS fval",
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
// 7. LIST FUNCTIONS — head, tail, last, reverse, range, size
// ===================================================================

fn bench_list_functions(c: &mut Criterion) {
    let mut group = c.benchmark_group("list_functions");

    // Throughput unit: *list elements touched per iteration*.

    let db = BenchDb::new();

    // head/tail/last over 5 elements.
    group.throughput(Throughput::Elements(5));
    group.bench_function("head_tail_last", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "WITH [1, 2, 3, 4, 5] AS lst \
                         RETURN head(lst) AS h, tail(lst) AS t, last(lst) AS l",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // reverse of 100-element list.
    group.throughput(Throughput::Elements(100));
    group.bench_function("reverse_list", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN reverse(range(1, 100)) AS rev", opts())
                    .unwrap(),
            );
        });
    });

    // range generates 1000 elements (twice, size() is O(1) on Vec).
    group.throughput(Throughput::Elements(1000));
    group.bench_function("range_generation", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "RETURN range(1, 1000) AS nums, size(range(1, 1000)) AS len",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // 0..=100 step 3 → 34 elements.
    group.throughput(Throughput::Elements(34));
    group.bench_function("range_with_step", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute("RETURN range(0, 100, 3) AS nums", opts())
                    .unwrap(),
            );
        });
    });

    // size() of a 500-element list (generation dominates cost).
    group.throughput(Throughput::Elements(500));
    group.bench_function("size_of_list", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute("WITH range(1, 500) AS lst RETURN size(lst) AS s", opts())
                    .unwrap(),
            );
        });
    });

    // --- nested list operations --- 50 elements built then reversed.
    group.throughput(Throughput::Elements(50));
    group.bench_function("nested_list_ops", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "WITH range(1, 50) AS nums \
                         RETURN head(tail(reverse(nums))) AS second_to_last, \
                                size(nums) AS len",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.finish();
}

// ===================================================================
// 8. PATH FUNCTIONS — nodes(), relationships(), length() on paths
// ===================================================================

fn bench_path_functions(c: &mut Criterion) {
    let mut group = c.benchmark_group("path_functions");

    // Throughput unit: *paths produced per iteration*.

    // --- nodes() on path --- bounded *1..5 → 5 paths from idx=0
    {
        let db = build_chain(100);
        group.throughput(Throughput::Elements(5));
        group.bench_function("nodes_on_path_chain_100", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH p = (a:Chain {idx:0})-[:NEXT*1..5]->(b) \
                             RETURN nodes(p) AS path_nodes, length(p) AS len",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- relationships() on path ---
    {
        let db = build_chain(100);
        group.throughput(Throughput::Elements(5));
        group.bench_function("relationships_on_path_chain_100", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH p = (a:Chain {idx:0})-[:NEXT*1..5]->(b) \
                             RETURN relationships(p) AS rels, length(p) AS len",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- path extraction on social graph --- LIMIT 50 caps result paths.
    {
        let db = build_social_graph(200, 4);
        group.throughput(Throughput::Elements(50));
        group.bench_function("path_extract_social_200", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH p = (a:Person {id:0})-[:KNOWS*1..3]->(b) \
                             RETURN length(p) AS len, size(nodes(p)) AS node_count \
                             LIMIT 50",
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
// 9. REGEX MATCHING — =~ operator
// ===================================================================

fn bench_regex_matching(c: &mut Criterion) {
    let mut group = c.benchmark_group("regex");

    // Throughput unit: *regex matches evaluated per iteration*. Literal bench
    // evaluates exactly one; graph benches evaluate one per scanned node.
    group.throughput(Throughput::Elements(1));

    let db_empty = BenchDb::new();
    let db_small = build_node_graph(Scale::SMALL);

    group.bench_function("regex_simple_literal", |b| {
        b.iter(|| {
            black_box(
                db_empty
                    .service
                    .execute(
                        "WITH 'Hello World 123' AS s \
                         RETURN s =~ '.*World.*' AS matches",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.throughput(Throughput::Elements(Scale::SMALL as u64));
    group.bench_function("regex_filter_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute(
                        "MATCH (n:Node) WHERE n.name =~ 'node_[5-9].*' RETURN n.id",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.throughput(Throughput::Elements(Scale::SMALL as u64));
    group.bench_function("regex_complex_pattern_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute(
                        "MATCH (n:Node) WHERE n.name =~ 'node_[0-9]{2,3}' RETURN n.id, n.name",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.finish();
}

// ===================================================================
// 10. WITH PIPING — multi-part queries with WITH chaining
// ===================================================================

fn bench_with_piping(c: &mut Criterion) {
    let mut group = c.benchmark_group("with_piping");

    // Throughput unit: *rows flowing through the WITH pipeline per query*.

    let db_small = build_node_graph(Scale::SMALL);
    let db_social = build_social_graph(200, 4);

    group.throughput(Throughput::Elements(Scale::SMALL as u64));
    group.bench_function("with_passthrough_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute(
                        "MATCH (n:Node) \
                         WITH n.id AS id, n.value AS val \
                         RETURN id, val",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // 200 Person nodes × ~4 KNOWS = 800 edges feeding the aggregator.
    group.throughput(Throughput::Elements(800));
    group.bench_function("with_agg_then_match_200", |b| {
        b.iter(|| {
            black_box(
                db_social
                    .service
                    .execute(
                        "MATCH (p:Person)-[:KNOWS]->(f:Person) \
                         WITH p, count(f) AS friend_count \
                         WHERE friend_count > 2 \
                         RETURN p.name, friend_count \
                         ORDER BY friend_count DESC LIMIT 10",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.throughput(Throughput::Elements(800));
    group.bench_function("with_triple_chain_200", |b| {
        b.iter(|| {
            black_box(
                db_social
                    .service
                    .execute(
                        "MATCH (p:Person)-[:KNOWS]->(f:Person) \
                         WITH p, count(f) AS cnt \
                         WITH p.city AS city, sum(cnt) AS total_friends \
                         RETURN city, total_friends \
                         ORDER BY total_friends DESC",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.throughput(Throughput::Elements(Scale::SMALL as u64));
    group.bench_function("with_top_n_pattern_1k", |b| {
        b.iter(|| {
            black_box(
                db_small
                    .service
                    .execute(
                        "MATCH (n:Node) \
                         WITH n ORDER BY n.value DESC LIMIT 50 \
                         RETURN n.id, n.name, n.value",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.finish();
}

// ===================================================================
// 11. RECOMMENDATION — realistic e-commerce workloads
// ===================================================================

fn bench_recommendation(c: &mut Criterion) {
    let mut group = c.benchmark_group("recommendation");
    group.sample_size(40);

    // Throughput unit: *one realistic recommendation query per iteration*.
    // These combine joins/aggregations/sorts over the 200×100 fixture, so
    // "queries per second" is the most honest figure.
    group.throughput(Throughput::Elements(1));

    // Previously this group built `build_recommendation_graph(200, 100)` six
    // separate times — each one a multi-pass UNWIND sequence that dominated
    // the group's setup cost. Build once and reuse.
    let db = build_recommendation_graph(200, 100);

    group.bench_function("user_purchases_200u_100p", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "MATCH (u:User {id: 0})-[:BOUGHT]->(p:Product) \
                         RETURN p.name, p.price, p.category",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("common_buyers_200u_100p", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "MATCH (u1:User {id: 0})-[:BOUGHT]->(p:Product)<-[:BOUGHT]-(u2:User) \
                         WHERE u2.id <> 0 \
                         RETURN DISTINCT u2.id, u2.name",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("collab_filter_200u_100p", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "MATCH (u1:User {id: 0})-[:BOUGHT]->(p:Product)<-[:BOUGHT]-(u2:User)-[:BOUGHT]->(rec:Product) \
                         WHERE u2.id <> 0 \
                         RETURN rec.name, rec.category, count(DISTINCT u2) AS score \
                         ORDER BY score DESC LIMIT 10",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("avg_rating_200u_100p", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "MATCH (u:User)-[r:REVIEWED]->(p:Product) \
                         RETURN p.name, p.category, avg(r.rating) AS avg_rating, count(r) AS reviews \
                         ORDER BY avg_rating DESC LIMIT 20",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("spending_by_tier_200u_100p", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "MATCH (u:User)-[b:BOUGHT]->(p:Product) \
                         RETURN u.tier, count(DISTINCT u) AS users, \
                                sum(p.price * b.quantity) AS total_spend \
                         ORDER BY total_spend DESC",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("similar_products_200u_100p", |b| {
        b.iter(|| {
            black_box(
                db.service
                    .execute(
                        "MATCH (p:Product {id: 0})-[:SIMILAR_TO]->(similar:Product) \
                         RETURN similar.name, similar.price, similar.category",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    // Larger dataset retained for top_categories — this is the one bench that
    // benefits from the extra breadth. Kept at a single build.
    let db_large = build_recommendation_graph(400, 150);
    group.bench_function("top_categories_400u_150p", |b| {
        b.iter(|| {
            black_box(
                db_large
                    .service
                    .execute(
                        "MATCH (u:User)-[b:BOUGHT]->(p:Product) \
                         RETURN p.category, \
                                count(b) AS purchases, \
                                sum(p.price * b.quantity) AS revenue \
                         ORDER BY revenue DESC",
                        opts(),
                    )
                    .unwrap(),
            );
        });
    });

    group.finish();
}

// ===================================================================
// Criterion harness
// ===================================================================

criterion_group! {
    name = benches;
    config = bench_config();
    targets =
        bench_union,
        bench_optional_match,
        bench_list_predicates,
        bench_string_functions,
        bench_math_functions,
        bench_type_conversion,
        bench_list_functions,
        bench_path_functions,
        bench_regex_matching,
        bench_with_piping,
        bench_recommendation,
}
criterion_main!(benches);
