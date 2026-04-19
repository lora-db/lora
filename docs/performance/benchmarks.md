## Performance Benchmarks

All numbers below come from `cargo bench` (Criterion) on the `engine_benchmarks`, `advanced_benchmarks`, and `scale_benchmarks` binaries under `crates/lora-database/benches/`. Run on Apple Silicon (`aarch64-apple-darwin`) in release mode on 2026-04-17. Throughput has been converted from Criterion's `Kelem/s` / `Melem/s` output into fully expanded numbers.

> ⚙️ **Note** — These numbers characterise the single-process, in-memory core. They are per-core (executor holds a global mutex) and assume the whole graph fits in RAM. For distributed throughput, read-heavy concurrency, or multi-tenant isolation, see the [LoraDB managed platform](https://loradb.com).

Reproduce with:

```bash
cargo bench --bench engine_benchmarks
cargo bench --bench advanced_benchmarks
cargo bench --bench scale_benchmarks
cargo bench --bench temporal_spatial_benchmarks
```

### Summary (representative numbers)

- **Simple scan, 1 000 nodes:** ~3 500 000 nodes/sec projection, ~16 800 000 nodes/sec when only `count(*)` is projected
- **Full-scan, 50 000 nodes:** ~7 400 000 nodes/sec (6.78 ms per query)
- **Single-hop traversal, 1 000 edges:** ~1 900 000 edges/sec (chain) / ~3 800 000 edges/sec (star)
- **Grouped aggregation, 1 000 rows:** ~3 300 000 rows/sec (`GROUP BY`), ~2 200 000 rows/sec (4× aggregators)
- **Sort, 1 000 rows single key:** ~1 170 000 rows/sec
- **Single-entity writes:** ~85 000 – 160 000 ops/sec; batch `CREATE` via `UNWIND`: ~900 000 nodes/sec
- **Parse-only, simple `MATCH`:** ~282 000 parses/sec; full parse+compile+execute: ~124 000 queries/sec
- **Realistic 500-person social friend-of-friend:** ~2 500 queries/sec (399 µs per query)

> **Note on "elements":** Criterion's throughput unit depends on the benchmark group. For scan / filter / aggregation groups it counts nodes scanned per query (so ops/sec reads as *nodes processed per second*). For traversal groups it counts edges or path destinations (so ops/sec reads as *edges/sec* or *paths/sec*). For write groups and realistic workloads it typically counts one full query per iteration (so ops/sec reads as *queries per second*).

### 1. MATCH — basic query execution

Fixture: `build_node_graph(N)` builds `N` `(:Node {id, name, value})` nodes.

| Benchmark | Dataset | Mean time | Throughput | Unit | Notes |
|---|---|---|---|---|---|
| `match/all_nodes/100` | 100 nodes | 33.37 µs | 2,996,780 | nodes/sec | `MATCH (n:Node) RETURN n.id` |
| `match/all_nodes/1000` | 1 000 nodes | 285.13 µs | 3,507,211 | nodes/sec | same query, larger fixture |
| `match/all_nodes/10000` | 10 000 nodes | 3.94 ms | 2,536,932 | nodes/sec | sub-linear scaling |
| `match/property_eq_filter/100` | 100 nodes | 37.24 µs | 2,685,361 | nodes/sec | `WHERE n.value = 42` |
| `match/property_eq_filter/1000` | 1 000 nodes | 292.71 µs | 3,416,388 | nodes/sec | |
| `match/property_eq_filter/10000` | 10 000 nodes | 3.38 ms | 2,958,781 | nodes/sec | |
| `match/range_filter_1k` | 1 000 nodes | 460.47 µs | 2,171,701 | nodes/sec | `WHERE n.value >= 20 AND n.value < 40` |
| `match/starts_with_1k` | 1 000 nodes | 334.08 µs | 2,993,292 | nodes/sec | `WHERE n.name STARTS WITH 'node_5'` |
| `match/return_property_1k` | 1 000 nodes | 313.35 µs | 3,191,283 | nodes/sec | projection of a single property |
| `match/count_only_1k` | 1 000 nodes | 71.66 µs | 13,954,170 | nodes/sec | `RETURN count(n)` — skips projection |

### 2. TRAVERSAL — single-hop, multi-hop, variable-length

Fixtures: `build_chain(N)` (linear `NEXT` chain), `build_star(N)` (hub with `N` arms), `build_tree(depth, branch)`, `build_cycle(N)` (ring), `build_social_graph(N, fanout)`.

| Benchmark | Dataset | Mean time | Throughput | Unit | Notes |
|---|---|---|---|---|---|
| `traversal/single_hop_chain/100` | chain of 100 | 58.30 µs | 1,698,064 | edges/sec | `(a:Chain)-[:NEXT]->(b:Chain)` |
| `traversal/single_hop_chain/500` | chain of 500 | 258.84 µs | 1,927,833 | edges/sec | |
| `traversal/single_hop_chain/1000` | chain of 1 000 | 527.42 µs | 1,894,126 | edges/sec | |
| `traversal/three_hop_chain_500` | chain of 500 | 79.88 µs | 12,519 | queries/sec | anchored 3-hop walk from `idx:0` |
| `traversal/varlen_1_5_chain/100` | chain of 100 | 23.79 µs | 210,148 | paths/sec | `[:NEXT*1..5]` → 5 destinations |
| `traversal/varlen_1_5_chain/500` | chain of 500 | 78.34 µs | 63,822 | paths/sec | |
| `traversal/varlen_1_5_chain/1000` | chain of 1 000 | 151.95 µs | 32,905 | paths/sec | |
| `traversal/varlen_unbounded_chain/100` | chain of 100 | 58.39 µs | 1,695,425 | hops/sec | `[:NEXT*]`, hop cap 100 |
| `traversal/varlen_unbounded_chain/500` | chain of 500 | 119.40 µs | 837,541 | hops/sec | |
| `traversal/star_fan_out/100` | star, 100 leaves | 36.60 µs | 2,732,260 | edges/sec | `(:Hub)-[:ARM]->(:Leaf)` |
| `traversal/star_fan_out/500` | star, 500 leaves | 135.65 µs | 3,685,978 | edges/sec | |
| `traversal/star_fan_out/1000` | star, 1 000 leaves | 262.64 µs | 3,807,470 | edges/sec | |
| `traversal/cycle_varlen_bounded/50` | ring of 50 | 18.48 µs | 541,080 | paths/sec | `[:LOOP*1..10]`, tests rel dedup |
| `traversal/cycle_varlen_bounded/100` | ring of 100 | 24.11 µs | 414,763 | paths/sec | |
| `traversal/cycle_varlen_bounded/500` | ring of 500 | 67.53 µs | 148,077 | paths/sec | |
| `traversal/tree_depth4_branch3_traverse` | 120 descendants | 44.92 µs | 2,671,518 | paths/sec | `[:CHILD*1..4]` |
| `traversal/tree_depth3_branch5_traverse` | 155 descendants | 55.63 µs | 2,786,462 | paths/sec | |
| `traversal/social_2hop_500_nodes` | 500 Persons | 179.63 µs | 5,567 | queries/sec | `(a)-[:KNOWS]->(b)-[:KNOWS]->(c) DISTINCT` |

### 3. FILTERING — predicates, boolean logic, parameters

All filter benches use the 1 000-node fixture (throughput counts nodes scanned per query).

| Benchmark | Mean time | Throughput | Unit | Notes |
|---|---|---|---|---|
| `filtering/bool_and_1k` | 478.76 µs | 2,088,727 | nodes/sec | `WHERE n.value > 20 AND n.value < 60` |
| `filtering/bool_or_1k` | 565.16 µs | 1,769,415 | nodes/sec | `WHERE n.value = 10 OR … OR …` |
| `filtering/bool_not_1k` | 363.47 µs | 2,751,283 | nodes/sec | `WHERE NOT n.value > 50` |
| `filtering/in_list_1k` | 353.87 µs | 2,825,933 | nodes/sec | `WHERE n.value IN [10,20,30,40,50]` |
| `filtering/parameterized_eq_1k` | 296.26 µs | 3,375,431 | nodes/sec | `WHERE n.value = $val` |
| `filtering/compound_predicate_1k` | 648.94 µs | 1,540,983 | nodes/sec | `(A AND B) OR C` |
| `filtering/high_selectivity_1k` | 201.90 µs | 4,952,885 | nodes/sec | `WHERE n.id = 500` (1 row out) |
| `filtering/low_selectivity_1k` | 422.90 µs | 2,364,648 | nodes/sec | `WHERE n.value >= 0` (all rows) |
| `filtering/rel_property_filter_200` | 289.28 µs | 2,074,148 | edges/sec | `WHERE k.strength > 5` over ~600 KNOWS |

### 4. AGGREGATION — count, sum, collect, grouping

| Benchmark | Dataset | Mean time | Throughput | Unit | Notes |
|---|---|---|---|---|---|
| `aggregation/count_star/100` | 100 nodes | 9.97 µs | 10,031,764 | rows/sec | `RETURN count(*)` |
| `aggregation/count_star/1000` | 1 000 nodes | 59.52 µs | 16,801,625 | rows/sec | |
| `aggregation/count_star/10000` | 10 000 nodes | 728.58 µs | 13,725,347 | rows/sec | |
| `aggregation/count_filtered_1k` | 1 000 nodes | 208.23 µs | 4,802,394 | rows/sec | `WHERE n.value > 50 → count(n)` |
| `aggregation/count_distinct_1k` | 1 000 nodes | 210.70 µs | 4,746,151 | rows/sec | `count(DISTINCT n.value)` |
| `aggregation/multi_agg_1k` | 1 000 nodes | 446.82 µs | 2,238,046 | rows/sec | `count`, `min`, `max`, `sum` together |
| `aggregation/group_by_low_card_1k` | 1 000 nodes | 301.99 µs | 3,311,359 | rows/sec | grouped by `n.value` |
| `aggregation/group_collect_1k` | 1 000 nodes | 415.09 µs | 2,409,126 | rows/sec | grouped `collect(n.id)` |
| `aggregation/collect_100` | 100 nodes | 24.78 µs | 4,034,830 | rows/sec | single-list `collect(n.name)` |
| `aggregation/agg_after_traversal_200` | ~600 edges | 250.09 µs | 2,399,124 | edges/sec | per-person `count(friends)` |
| `aggregation/having_pattern_200` | ~600 edges | 288.87 µs | 2,077,025 | edges/sec | `WITH … count(f) … WHERE cnt > 2` |

### 5. ORDERING — ORDER BY, DISTINCT, SKIP/LIMIT

| Benchmark | Dataset | Mean time | Throughput | Unit | Notes |
|---|---|---|---|---|---|
| `ordering/order_by_single/100` | 100 rows | 49.80 µs | 2,007,870 | rows/sec | `ORDER BY n.value ASC` |
| `ordering/order_by_single/1000` | 1 000 rows | 853.55 µs | 1,171,582 | rows/sec | |
| `ordering/order_limit_top10_1k` | 1 000 rows | 745.02 µs | 1,342,241 | rows/sec | `ORDER BY … DESC LIMIT 10` |
| `ordering/order_multi_key_1k` | 1 000 rows | 1.27 ms | 786,269 | rows/sec | two sort keys |
| `ordering/distinct_1k` | 1 000 rows | 318.96 µs | 3,135,233 | rows/sec | `DISTINCT n.value` |
| `ordering/skip_limit_pagination_1k` | 1 000 rows | 311.75 µs | 3,207,677 | rows/sec | `ORDER BY … SKIP 500 LIMIT 50` |

### 6. WRITE operations — CREATE, MERGE, SET, DELETE

Single-entity write benches use `iter_batched` (fresh DB per iteration). Throughput is reported as *operations per second* (1 op per iter). Batch writes count nodes created per query.

| Benchmark | Mean time | Throughput | Unit | Notes |
|---|---|---|---|---|
| `write/create_single_node` | 8.60 µs | 116,331 | ops/sec | `CREATE (:Bench {…})` on empty DB |
| `write/create_node_and_rel` | 11.82 µs | 84,630 | ops/sec | `MATCH (a), (b) CREATE (a)-[:REL]->(b)` |
| `write/merge_create_new` | 6.80 µs | 147,094 | ops/sec | `MERGE (:Singleton {…})`, create path |
| `write/merge_match_existing` | 11.94 µs | 83,757 | ops/sec | `MERGE … ON MATCH SET` (existing row) |
| `write/set_property` | 7.33 µs | 136,453 | ops/sec | `MATCH (n:Target) SET n.val = 42` |
| `write/delete_node` | 6.37 µs | 156,954 | ops/sec | `MATCH (n:Temp) DELETE n` |
| `write/detach_delete` | 7.56 µs | 132,275 | ops/sec | hub + 5 leaves, `DETACH DELETE` |
| `write/batch_create_unwind/10` | 25.42 µs | 393,414 | nodes/sec | `UNWIND range(1, N) CREATE …` |
| `write/batch_create_unwind/50` | 67.69 µs | 738,714 | nodes/sec | |
| `write/batch_create_unwind/100` | 122.57 µs | 815,830 | nodes/sec | |
| `write/batch_create_unwind/500` | 554.79 µs | 901,238 | nodes/sec | steady state for `UNWIND` insertion |

### 7. FUNCTIONS — strings, math, types, paths

| Benchmark | Mean time | Throughput | Unit | Notes |
|---|---|---|---|---|
| `functions/string_toLower` | 4.29 µs | 232,963 | evals/sec | `RETURN toLower('HELLO WORLD')` |
| `functions/string_replace` | 6.35 µs | 157,541 | evals/sec | `RETURN replace(…)` |
| `functions/toLower_on_100_nodes` | 42.44 µs | 2,356,490 | evals/sec | `toLower(n.name)` over 100 nodes |
| `functions/math_abs_sqrt` | 6.38 µs | 156,820 | evals/sec | `RETURN abs(-42), sqrt(144)` |
| `functions/math_on_100_nodes` | 56.93 µs | 1,756,502 | evals/sec | `abs(n.value - 50)` over 100 rows |
| `functions/labels_keys_type` | 17.63 µs | 340,303 | evals/sec | `labels(p), keys(p), type(r)` over 6 rows |
| `functions/case_expression_100` | 43.46 µs | 2,301,150 | evals/sec | `CASE WHEN … THEN …` over 100 rows |
| `functions/coalesce` | 6.97 µs | 143,405 | evals/sec | `coalesce(null, null, 42, 99)` |
| `functions/list_comprehension` | 19.55 µs | 5,113,910 | elems/sec | `[x IN range(1,100) WHERE … | x*x]` |
| `functions/reduce_sum` | 12.37 µs | 8,086,668 | elems/sec | `reduce(acc, x IN range(1,100) | acc+x)` |
| `string_functions/toUpper` | 4.18 µs | 239,031 | evals/sec | |
| `string_functions/trim` | 4.10 µs | 244,158 | evals/sec | |
| `string_functions/substring` | 6.10 µs | 163,856 | evals/sec | |
| `string_functions/split` | 5.40 µs | 185,220 | evals/sec | `split('a,b,c,d,e', ',')` |
| `string_functions/string_pipeline_100_nodes` | 101.71 µs | 983,227 | evals/sec | `toUpper` + `substring` + `size` per row |
| `math_functions/trig_sin_cos_tan` | 8.67 µs | 115,350 | evals/sec | three trig calls combined |
| `math_functions/math_pipeline_100_nodes` | 115.63 µs | 864,856 | evals/sec | `ceil`, `sqrt`, `sign` per row |
| `type_conversion/toInteger_from_string` | 4.17 µs | 239,970 | evals/sec | |
| `type_conversion/conversions_on_100_nodes` | 63.46 µs | 1,575,683 | evals/sec | `toString(n.id), toFloat(n.value)` per row |
| `list_functions/range_generation` | 21.86 µs | 45,749,369 | elems/sec | `range(1, 1000)` |
| `list_functions/reverse_list` | 7.62 µs | 13,115,177 | elems/sec | `reverse(range(1, 100))` |
| `list_functions/size_of_list` | 13.25 µs | 37,730,804 | elems/sec | `size(range(1, 500))` |
| `list_predicates/any_in_list` | 13.24 µs | 377,727 | evals/sec | `any(x IN list WHERE x > 3)` |
| `list_predicates/all_in_list` | 12.85 µs | 311,281 | evals/sec | `all(x IN list WHERE x%2 = 0)` |
| `list_predicates/reduce_sum/500` | 23.26 µs | 21,496,659 | elems/sec | `reduce(…, range(1, 500) | …)` |
| `path_functions/nodes_on_path_chain_100` | 32.72 µs | 152,816 | paths/sec | `nodes(p), length(p)` |
| `path_functions/relationships_on_path_chain_100` | 31.65 µs | 157,992 | paths/sec | `relationships(p)` |
| `path_functions/path_extract_social_200` | 79.01 µs | 632,819 | paths/sec | `size(nodes(p))` over 50 paths |
| `regex/regex_simple_literal` | 43.86 µs | 22,802 | evals/sec | `'…' =~ '.*World.*'` |
| `regex/regex_filter_1k` | 33.53 ms | 29,822 | nodes/sec | `WHERE n.name =~ 'node_[5-9].*'` |
| `regex/regex_complex_pattern_1k` | 9.78 ms | 102,282 | nodes/sec | `=~ 'node_[0-9]{2,3}'` |

### 8. UNION / OPTIONAL MATCH / WITH

| Benchmark | Mean time | Throughput | Unit | Notes |
|---|---|---|---|---|
| `union/union_two_queries_1k` | 782.65 µs | 2,555,421 | rows/sec | dedup `UNION` of two `MATCH` branches |
| `union/union_all_two_queries_1k` | 655.19 µs | 3,052,529 | rows/sec | `UNION ALL` (no dedup) |
| `union/union_different_labels` | 15.83 µs | 568,461 | rows/sec | `Person` ∪ `City` |
| `union/union_triple` | 26.35 µs | 417,528 | rows/sec | three-way `UNION ALL` |
| `optional_match/optional_sparse_match` | 17.35 µs | 345,731 | rows/sec | 6 rows, mostly null |
| `optional_match/optional_mostly_matched_200` | 1.10 ms | 182,660 | rows/sec | 200 rows, mostly matched |
| `optional_match/double_optional_match` | 26.44 µs | 226,953 | rows/sec | two `OPTIONAL MATCH` clauses |
| `optional_match/optional_match_scale/200` | 773.03 µs | 258,724 | rows/sec | with filter + `collect` |
| `optional_match/optional_match_scale/500` | 3.11 ms | 160,834 | rows/sec | |
| `with_piping/with_passthrough_1k` | 555.86 µs | 1,799,003 | rows/sec | `WITH n.id AS id, n.value AS val` |
| `with_piping/with_top_n_pattern_1k` | 841.38 µs | 1,188,526 | rows/sec | `WITH n ORDER BY … LIMIT 50` |
| `with_piping/with_agg_then_match_200` | 343.22 µs | 2,330,849 | edges/sec | aggregate → filter → order |
| `with_piping/with_triple_chain_200` | 442.49 µs | 1,807,935 | edges/sec | three chained `WITH` stages |

### 9. Parse / compile overhead

Isolates query planning cost; `full_compile` includes parse + analyze + compile + execute against the 12-node org fixture.

| Benchmark | Mean time | Throughput | Unit |
|---|---|---|---|
| `parse_compile/parse/simple_match` | 3.55 µs | 282,073 | parses/sec |
| `parse_compile/parse/match_where` | 8.06 µs | 124,062 | parses/sec |
| `parse_compile/parse/multi_hop` | 5.56 µs | 179,765 | parses/sec |
| `parse_compile/parse/aggregation` | 6.01 µs | 166,320 | parses/sec |
| `parse_compile/parse/complex` | 15.06 µs | 66,410 | parses/sec |
| `parse_compile/full_compile/simple_match` | 8.05 µs | 124,240 | queries/sec |
| `parse_compile/full_compile/match_where` | 13.18 µs | 75,875 | queries/sec |
| `parse_compile/full_compile/multi_hop` | 10.90 µs | 91,770 | queries/sec |
| `parse_compile/full_compile/aggregation` | 11.23 µs | 89,046 | queries/sec |
| `parse_compile/full_compile/complex` | 31.21 µs | 32,044 | queries/sec |

### 10. Realistic workloads

One complete query per iteration.

| Benchmark | Dataset | Mean time | Throughput | Unit |
|---|---|---|---|---|
| `realistic/org_manager_subordinates` | 12-node org | 14.52 µs | 68,848 | queries/sec |
| `realistic/org_dept_headcount` | 12-node org | 12.96 µs | 77,178 | queries/sec |
| `realistic/org_people_per_city` | 12-node org | 17.43 µs | 57,371 | queries/sec |
| `realistic/org_multi_hop_mgr_project` | 12-node org | 46.02 µs | 21,728 | queries/sec |
| `realistic/social_friend_of_friend_500` | 500 Persons | 399.46 µs | 2,503 | queries/sec |
| `realistic/social_mutual_friends_500` | 500 Persons | 402.24 µs | 2,486 | queries/sec |
| `realistic/social_common_city_friends_500` | 500 Persons | 601.65 µs | 1,662 | queries/sec |
| `realistic/social_influence_score_500` | 500 Persons | 5.71 ms | 175 | queries/sec |
| `realistic/social_degree_distribution_1k` | 1 000 Persons | 859.56 µs | 1,163 | queries/sec |
| `realistic/pipeline_filter_agg_sort_500` | 500 Persons | 777.48 µs | 1,286 | queries/sec |
| `realistic/unwind_aggregate` | 3 Data nodes | 20.72 µs | 48,262 | queries/sec |
| `recommendation/user_purchases_200u_100p` | 200 users × 100 products | 244.87 µs | 4,084 | queries/sec |
| `recommendation/common_buyers_200u_100p` | same | 275.77 µs | 3,626 | queries/sec |
| `recommendation/collab_filter_200u_100p` | same | 317.40 µs | 3,151 | queries/sec |
| `recommendation/avg_rating_200u_100p` | same | 308.37 µs | 3,243 | queries/sec |
| `recommendation/spending_by_tier_200u_100p` | same | 624.96 µs | 1,600 | queries/sec |
| `recommendation/similar_products_200u_100p` | same | 227.23 µs | 4,401 | queries/sec |
| `recommendation/top_categories_400u_150p` | 400 users × 150 products | 1.15 ms | 872 | queries/sec |

### 11. Scale — 10 000 / 50 000 nodes

Fewer samples (15), longer measurement window. `Scale::MEDIUM` = 10 000, `Scale::LARGE` = 50 000.

#### 11a. Scan & filter at scale

| Benchmark | Mean time | Throughput | Unit |
|---|---|---|---|
| `scale_match/full_scan/10000` | 812.89 µs | 12,301,749 | nodes/sec |
| `scale_match/full_scan/50000` | 6.78 ms | 7,372,275 | nodes/sec |
| `scale_match/property_eq/10000` | 3.52 ms | 2,843,932 | nodes/sec |
| `scale_match/property_eq/50000` | 22.41 ms | 2,231,640 | nodes/sec |
| `scale_match/range_filter/10000` | 5.70 ms | 1,755,593 | nodes/sec |
| `scale_match/range_filter/50000` | 37.75 ms | 1,324,695 | nodes/sec |
| `scale_match/starts_with/10000` | 3.84 ms | 2,607,756 | nodes/sec |
| `scale_match/starts_with/50000` | 23.41 ms | 2,135,821 | nodes/sec |
| `scale_match/return_multi_props/10000` | 3.82 ms | 2,617,050 | nodes/sec |
| `scale_match/return_multi_props/50000` | 22.31 ms | 2,240,705 | nodes/sec |

#### 11b. Aggregation at scale

| Benchmark | Mean time | Throughput | Unit |
|---|---|---|---|
| `scale_aggregation/count_star/10000` | 760.37 µs | 13,151,493 | rows/sec |
| `scale_aggregation/count_star/50000` | 5.42 ms | 9,220,574 | rows/sec |
| `scale_aggregation/group_by_100_groups/10000` | 3.83 ms | 2,612,598 | rows/sec |
| `scale_aggregation/group_by_100_groups/50000` | 26.06 ms | 1,918,934 | rows/sec |
| `scale_aggregation/multi_aggregate/10000` | 5.92 ms | 1,688,419 | rows/sec |
| `scale_aggregation/multi_aggregate/50000` | 46.98 ms | 1,064,193 | rows/sec |
| `scale_aggregation/count_distinct/10000` | 2.89 ms | 3,463,149 | rows/sec |
| `scale_aggregation/count_distinct/50000` | 22.40 ms | 2,231,919 | rows/sec |

#### 11c. Ordering at scale

| Benchmark | Mean time | Throughput | Unit |
|---|---|---|---|
| `scale_ordering/order_by_single/10000` | 15.30 ms | 653,471 | rows/sec |
| `scale_ordering/order_by_single/50000` | 108.51 ms | 460,784 | rows/sec |
| `scale_ordering/order_limit_top10/10000` | 14.82 ms | 674,902 | rows/sec |
| `scale_ordering/order_limit_top10/50000` | 88.79 ms | 563,149 | rows/sec |
| `scale_ordering/distinct/10000` | 3.82 ms | 2,615,030 | rows/sec |
| `scale_ordering/distinct/50000` | 21.95 ms | 2,277,676 | rows/sec |
| `scale_ordering/order_multi_key/10000` | 35.03 ms | 285,477 | rows/sec |
| `scale_ordering/order_multi_key/50000` | 230.09 ms | 217,304 | rows/sec |

#### 11d. Traversal at scale

| Benchmark | Dataset | Mean time | Throughput | Unit |
|---|---|---|---|---|
| `scale_traversal/single_hop_chain/2000` | chain of 2 000 | 447.54 µs | 4,468,877 | edges/sec |
| `scale_traversal/single_hop_chain/5000` | chain of 5 000 | 1.29 ms | 3,882,595 | edges/sec |
| `scale_traversal/varlen_1_5_chain/5000` | chain of 5 000 | 670.69 µs | 7,455 | paths/sec |
| `scale_traversal/star_fan_out/5000` | star, 5 000 leaves | 566.82 µs | 8,821,134 | edges/sec |
| `scale_traversal/tree_depth5_branch3` | 363 descendants | 116.84 µs | 3,106,825 | paths/sec |
| `scale_traversal/tree_depth3_branch10` | 1 110 descendants | 311.49 µs | 3,563,559 | paths/sec |

#### 11e. Write at scale

| Benchmark | Mean time | Throughput | Unit | Notes |
|---|---|---|---|---|
| `scale_write/batch_create_unwind/1000` | 1.33 ms | 753,513 | nodes/sec | plain `UNWIND … CREATE` |
| `scale_write/batch_create_unwind/5000` | 7.00 ms | 713,852 | nodes/sec | |
| `scale_write/batch_create_chain/500` | 86.41 ms | 5,786 | edges/sec | `MATCH`-then-`CREATE` per edge (O(n²)) |
| `scale_write/batch_create_chain/1000` | 340.51 ms | 2,937 | edges/sec | same pattern, larger dataset |

#### 11f. Social workloads at scale

| Benchmark | Dataset | Mean time | Throughput | Unit |
|---|---|---|---|---|
| `scale_social/friend_of_friend_2k` | 2 000 Persons | 1.20 ms | 1,669,935 | persons/sec |
| `scale_social/friend_of_friend_3k` | 3 000 Persons | 1.82 ms | 1,644,881 | persons/sec |
| `scale_social/degree_distribution_2k` | 2 000 Persons | 2.09 ms | 958,061 | persons/sec |
| `scale_social/degree_distribution_3k` | 3 000 Persons | 3.56 ms | 843,018 | persons/sec |
| `scale_social/mutual_friends_2k` | 2 000 Persons | 1.16 ms | 1,722,862 | persons/sec |
| `scale_social/city_friend_count_2k` | 2 000 Persons | 3.91 ms | 511,961 | persons/sec |

### 12. Temporal types — Date / Time / DateTime / Duration

| Benchmark | Mean time | Throughput | Unit | Notes |
|---|---|---|---|---|
| `temporal_creation/date_from_string` | 4.42 µs | 226,201 | evals/sec | `date('2024-01-15')` |
| `temporal_creation/date_from_map` | 7.78 µs | 128,562 | evals/sec | `date({year: 2024, month: 1, day: 15})` |
| `temporal_creation/time_from_string` | 4.42 µs | 226,125 | evals/sec | `time('14:30:00')` |
| `temporal_creation/datetime_from_string` | 4.54 µs | 220,450 | evals/sec | `datetime('2024-01-15T14:30:00Z')` |
| `temporal_creation/datetime_from_map` | 10.41 µs | 96,097 | evals/sec | |
| `temporal_creation/duration_from_string` | 4.40 µs | 227,177 | evals/sec | `duration('P1Y2M3DT4H5M6S')` |
| `temporal_creation/duration_from_map` | 9.07 µs | 110,258 | evals/sec | |
| `temporal_creation/multi_temporal_creation` | 12.49 µs | 320,173 | evals/sec | 4 constructors combined |
| `temporal_creation/date_component_access` | 14.76 µs | 338,846 | evals/sec | `.year`, `.month`, etc. on `date` |
| `temporal_creation/datetime_component_access` | 16.32 µs | 367,716 | evals/sec | 6-component access on `datetime` |
| `temporal_filtering/date_greater_than/100` | 108.40 µs | 922,490 | rows/sec | `WHERE n.created > date('…')` |
| `temporal_filtering/date_greater_than/500` | 488.24 µs | 1,024,088 | rows/sec | |
| `temporal_filtering/date_greater_than/1000` | 973.74 µs | 1,026,969 | rows/sec | |
| `temporal_filtering/date_equality_500` | 331.31 µs | 1,509,166 | rows/sec | |
| `temporal_filtering/date_range_500` | 708.59 µs | 705,631 | rows/sec | both-sided range predicate |
| `temporal_filtering/order_by_date_500` | 670.43 µs | 745,794 | rows/sec | `ORDER BY date_prop` |
| `temporal_filtering/group_by_priority_500` | 200.83 µs | 2,489,624 | rows/sec | |
| `temporal_filtering/date_component_inline` | 39.76 µs | 704,304 | rows/sec | `WHERE date(n.x).year = 2024` |
| `temporal_arithmetic/date_plus_duration` | 6.47 µs | 154,660 | evals/sec | `date + duration` |
| `temporal_arithmetic/date_minus_duration` | 6.47 µs | 154,621 | evals/sec | |
| `temporal_arithmetic/duration_add` | 6.29 µs | 158,898 | evals/sec | `duration + duration` |
| `temporal_arithmetic/duration_between` | 8.11 µs | 123,293 | evals/sec | |
| `temporal_arithmetic/date_arithmetic_on_graph_200` | 169.39 µs | 1,180,681 | rows/sec | `RETURN n.date + dur` over 200 rows |
| `temporal_arithmetic/datetime_plus_duration_200` | 173.74 µs | 1,151,177 | rows/sec | |

### 13. Spatial — Point / distance

| Benchmark | Mean time | Throughput | Unit | Notes |
|---|---|---|---|---|
| `spatial_creation/point_cartesian` | 6.89 µs | 145,204 | evals/sec | `point({x, y})` — SRID 7203 |
| `spatial_creation/point_geographic` | 6.86 µs | 145,760 | evals/sec | `point({latitude, longitude})` — SRID 4326 |
| `spatial_creation/multi_point_creation` | 16.26 µs | 184,551 | evals/sec | 3 points in one query |
| `spatial_creation/point_component_access` | 13.96 µs | 214,847 | evals/sec | `.x`, `.y`, `.crs` |
| `spatial_distance/distance_cartesian` | 12.94 µs | 77,254 | evals/sec | Euclidean distance |
| `spatial_distance/distance_geographic` | 13.27 µs | 75,361 | evals/sec | Haversine distance |
| `spatial_distance/pairwise_distance_graph/100` | 152.74 µs | 654,696 | rows/sec | distance per row, 100 nodes |
| `spatial_distance/pairwise_distance_graph/500` | 713.35 µs | 700,916 | rows/sec | 500 nodes |
| `spatial_distance/geo_distance_graph_200` | 293.54 µs | 681,340 | rows/sec | geographic distance over 200 rows |
| `spatial_filtering/distance_threshold_200` | 234.30 µs | 853,596 | rows/sec | `WHERE distance(a, b) < threshold` |
| `spatial_filtering/nearest_sorted_200` | 166.16 µs | 1,203,693 | rows/sec | `ORDER BY distance` |
| `spatial_filtering/category_distance_filter_500` | 705.17 µs | 709,053 | rows/sec | compound spatial + label predicate |

### 14. Shortest path — shortestPath() / allShortestPaths()

One query per iteration; throughput reads as *queries per second*.

| Benchmark | Dataset | Mean time | Throughput | Unit |
|---|---|---|---|---|
| `shortest_path/shortest_chain/100` | chain of 100 | 66.61 µs | 15,013 | queries/sec |
| `shortest_path/shortest_chain/500` | chain of 500 | 111.66 µs | 8,956 | queries/sec |
| `shortest_path/shortest_tree_depth4_branch3` | 120-node tree | 57.93 µs | 17,263 | queries/sec |
| `shortest_path/shortest_social_bounded/100` | 100 Persons | 284.70 µs | 3,513 | queries/sec |
| `shortest_path/shortest_social_bounded/200` | 200 Persons | 299.89 µs | 3,335 | queries/sec |
| `shortest_path/all_shortest_social_100` | 100 Persons | 283.68 µs | 3,525 | queries/sec |
| `shortest_path/shortest_dep_graph_100` | 100-node DAG | 555.24 µs | 1,801 | queries/sec |

### Notes & caveats

- **All benches are in-memory.** The store is a `BTreeMap`-backed graph held behind an `Arc<Mutex>`. Numbers will not translate directly to any persistent engine — there is no I/O, no buffer pool, no WAL.
- **No indexes.** Every `WHERE` predicate scans the label set linearly. `property_eq` and `starts_with` are O(N); an index layer would shift these to O(log N).
- **Single-threaded executor.** Each query holds the store mutex for its duration; all numbers are per-core and not a measurement of concurrent throughput.
- **Microbenchmark vs. workload.** The *functions*, *parse_compile*, *temporal_creation*, and *spatial_creation* tables are microbenchmarks that stabilise in a few µs; they measure constant-factor cost of the evaluator and planner. The *realistic*, *recommendation*, *scale_social*, and *shortest_path* tables are representative whole-query workloads.
- **Variable-length path hop cap.** Unbounded `*` paths are capped at `MAX_VAR_LEN_HOPS = 100`; the `varlen_unbounded_chain/500` number reflects that cap.
- **Regex is the slow outlier.** `regex/regex_filter_1k` runs at ~30 000 nodes/sec — two orders of magnitude below a boolean predicate. Regex compilation happens per query.

## Next steps

- Understand the current bottlenecks: [Performance Notes](notes.md)
- See how the executor and storage fit together: [Data Flow](../architecture/data-flow.md), [Graph Engine](../architecture/graph-engine.md)
- If you're hitting the single-mutex ceiling or need persistent scale, check the [LoraDB managed platform](https://loradb.com)
