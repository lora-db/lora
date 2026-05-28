# Graph DB Comparison Report

Engines: `lora`, `kuzu`, `grafeo`, `surrealdb`, `memgraph`, `neo4j`, `helixdb` · 82 workloads across 12 groups.

## Summary

_Each engine column shows the geometric-mean slowdown of that engine vs the group winner across every workload they share._

| Group      | Workloads | Winner   |         lora |             kuzu |           grafeo |         surrealdb |         memgraph |            neo4j |           helixdb |
| ---------- | --------: | -------- | -----------: | ---------------: | ---------------: | ----------------: | ---------------: | ---------------: | ----------------: |
| setup      |         1 | grafeo   | 2.70× slower |   700.36× slower |          fastest |                 — |   904.27× slower |  1768.78× slower |   1246.77× slower |
| writes     |         9 | grafeo   | 0.61× slower |     1.79× slower |          fastest |     28.33× slower |     3.71× slower |     7.75× slower |    124.50× slower |
| scans      |         6 | lora     |      fastest |     6.37× slower |     5.43× slower |    120.66× slower |    27.54× slower |    24.12× slower |    287.24× slower |
| predicates |        12 | lora     |      fastest |     1.14× slower |     1.83× slower |     21.32× slower |     4.77× slower |     3.78× slower |     20.86× slower |
| strings    |         5 | kuzu     | 1.22× slower |          fastest |     1.84× slower |     34.09× slower |    11.30× slower |     4.19× slower |                 — |
| numerics   |         6 | kuzu     | 1.08× slower |          fastest |     1.87× slower |     39.86× slower |    10.71× slower |     4.00× slower |     20.40× slower |
| aggregates |         9 | lora     |      fastest |     3.18× slower |     1.89× slower |     44.74× slower |     6.25× slower |     6.94× slower |     35.49× slower |
| pipeline   |         9 | lora     |      fastest |     1.27× slower |     1.30× slower |     36.83× slower |     4.97× slower |     3.07× slower |     17.23× slower |
| lists      |         3 | lora     |      fastest |    11.33× slower |     3.21× slower |     32.28× slower |    41.56× slower |    42.23× slower |     15.01× slower |
| sort       |         3 | lora     |      fastest |     1.41× slower |     1.60× slower |     31.76× slower |     4.28× slower |     3.48× slower |     16.44× slower |
| traversals |        15 | lora     |      fastest |    20.35× slower |     5.88× slower |    110.35× slower |    22.42× slower |    17.77× slower |    132.79× slower |
| patterns   |         4 | lora     |      fastest |     2.80× slower |     2.45× slower |    190.07× slower |     7.59× slower |     4.96× slower |     22.24× slower |
| **total**  |    **82** | **lora** |  **fastest** | **3.30× slower** | **2.30× slower** | **48.58× slower** | **9.75× slower** | **7.99× slower** | **57.20× slower** |

## setup _(1)_

| Workload        | Size |            lora |              kuzu |     grafeo |          memgraph |              neo4j |            helixdb | Winner |
| --------------- | ---: | --------------: | ----------------: | ---------: | ----------------: | -----------------: | -----------------: | ------ |
| construct_empty |    – | 4.06 µs (2.70×) | 1.05 ms (700.36×) | 1.50 µs ⭐ | 1.36 ms (904.27×) | 2.66 ms (1768.78×) | 1.87 ms (1246.77×) | grafeo |

## writes _(9)_

| Workload           | Size |              lora |               kuzu |             grafeo |          surrealdb |           memgraph |              neo4j |               helixdb | Winner |
| ------------------ | ---: | ----------------: | -----------------: | -----------------: | -----------------: | -----------------: | -----------------: | --------------------: | ------ |
| bulk_edges         |  200 |      606.63 µs ⭐ |    2.01 ms (3.31×) |  19.86 ms (32.74×) |                  — |    3.18 ms (5.24×) |    2.65 ms (4.37×) |   309.68 ms (510.49×) | lora   |
| bulk_set_match     | 1000 | 552.39 µs (1.84×) |       300.03 µs ⭐ |  315.88 µs (1.05×) |   6.15 ms (20.48×) |  726.92 µs (2.42×) |  916.11 µs (3.05×) |      3.57 ms (11.90×) | kuzu   |
| delete_node        | 1000 | 355.70 µs (1.47×) |  598.35 µs (2.48×) |       241.43 µs ⭐ |   5.56 ms (23.04×) |  483.58 µs (2.00×) |    1.65 ms (6.84×) |   151.74 ms (628.54×) | grafeo |
| merge_create       | 1000 | 112.57 µs (1.25×) |  670.68 µs (7.45×) |        90.02 µs ⭐ |                  — |  677.96 µs (7.53×) |   2.48 ms (27.56×) |                     — | grafeo |
| merge_existing     | 1000 |  23.95 µs (2.89×) | 162.06 µs (19.55×) |         8.29 µs ⭐ |                  — | 304.94 µs (36.78×) | 647.72 µs (78.13×) |                     — | grafeo |
| set_multiple_props | 1000 |       21.15 µs ⭐ |  169.72 µs (8.03×) |  182.77 µs (8.64×) |  5.54 ms (262.09×) | 313.89 µs (14.84×) | 571.61 µs (27.03×) |     2.37 ms (112.05×) | lora   |
| update_set         | 1000 |       18.01 µs ⭐ | 188.62 µs (10.47×) | 208.61 µs (11.58×) |  6.04 ms (335.30×) | 315.49 µs (17.52×) | 732.47 µs (40.68×) |     2.91 ms (161.53×) | lora   |
| write_bulk         | 1000 |   1.46 ms (1.93×) |    2.76 ms (3.65×) |       756.21 µs ⭐ |  26.66 ms (35.26×) |    3.13 ms (4.14×) |    6.34 ms (8.38×) |  771.22 ms (1019.86×) | grafeo |
| write_single       | 1000 |  14.90 µs (2.20×) |                  — |         6.78 µs ⭐ | 252.56 µs (37.27×) | 413.52 µs (61.02×) |  1.30 ms (192.29×) | 153.86 ms (22704.20×) | grafeo |

**Notes**

- `write_single` · **kuzu** — Omitted: Kuzu requires `CREATE NODE TABLE` before inserts; the empty fixture has no schema and adding one would change the iteration's measured cost.
- `merge_existing` · **helixdb** — Omitted: HelixDB has no MERGE/upsert; emulating it needs conditional var_as_if branching.
- `merge_existing` · **surrealdb** — Omitted: SurrealDB's UPSERT semantics diverge from Cypher MERGE on which fields are matched vs set.
- `merge_create` · **helixdb** — Omitted: HelixDB has no MERGE/upsert (see merge_existing).
- `merge_create` · **surrealdb** — Omitted: see merge_existing — UPSERT semantics differ.
- `bulk_edges` · **surrealdb** — Omitted: UNWIND-driven bulk RELATE requires scripted FOR loops; not a like-for-like comparison.

## scans _(6)_

| Workload             | Size |              lora |                kuzu |              grafeo |          surrealdb |            memgraph |                neo4j |            helixdb | Winner |
| -------------------- | ---: | ----------------: | ------------------: | ------------------: | -----------------: | ------------------: | -------------------: | -----------------: | ------ |
| distinct             | 1000 |      198.38 µs ⭐ |   448.06 µs (2.26×) |   246.37 µs (1.24×) |                  — |   738.04 µs (3.72×) |    566.24 µs (2.85×) |                  — | lora   |
| lookup_by_id         | 1000 |      716.64 ns ⭐ | 116.07 µs (161.96×) | 170.03 µs (237.26×) | 5.26 ms (7340.09×) | 305.39 µs (426.14×) |  601.36 µs (839.14×) | 2.30 ms (3204.88×) | lora   |
| lookup_by_id_indexed | 1000 |      684.00 ns ⭐ | 116.18 µs (169.85×) |   22.73 µs (33.23×) |  32.35 µs (47.30×) | 310.02 µs (453.25×) | 747.01 µs (1092.12×) | 2.62 ms (3830.95×) | lora   |
| range_filter         | 1000 | 199.70 µs (1.04×) |        192.68 µs ⭐ |   214.22 µs (1.11×) |   9.29 ms (48.24×) |     1.22 ms (6.31×) |    589.16 µs (3.06×) |                  — | kuzu   |
| scan_filtered        | 1000 |      149.08 µs ⭐ |   158.49 µs (1.06×) |   211.95 µs (1.42×) |   5.88 ms (39.46×) |     1.15 ms (7.73×) |    718.45 µs (4.82×) |   3.82 ms (25.64×) | lora   |
| scan_label           | 1000 |      124.96 µs ⭐ |   131.34 µs (1.05×) |   213.71 µs (1.71×) |   5.01 ms (40.10×) |    1.61 ms (12.89×) |    661.84 µs (5.30×) |   2.70 ms (21.62×) | lora   |

**Notes**

- `distinct` · **helixdb** — Omitted: HelixDB's dedup is node-level; there is no SELECT DISTINCT <property>.
- `distinct` · **surrealdb** — Omitted: `value` is a reserved word in SurrealQL's SELECT VALUE clause; no clean equivalent.

## predicates _(12)_

| Workload              | Size |              lora |              kuzu |            grafeo |         surrealdb |          memgraph |             neo4j |          helixdb | Winner    |
| --------------------- | ---: | ----------------: | ----------------: | ----------------: | ----------------: | ----------------: | ----------------: | ---------------: | --------- |
| where_compound_and_or | 1000 |      213.07 µs ⭐ | 230.83 µs (1.08×) | 646.50 µs (3.03×) | 10.23 ms (48.02×) | 997.22 µs (4.68×) | 584.96 µs (2.75×) | 3.23 ms (15.17×) | lora      |
| where_contains        | 1000 |      145.04 µs ⭐ | 166.24 µs (1.15×) | 255.53 µs (1.76×) |  4.96 ms (34.20×) | 638.95 µs (4.41×) | 597.31 µs (4.12×) | 3.02 ms (20.84×) | lora      |
| where_ends_with       | 1000 |      143.60 µs ⭐ | 167.41 µs (1.17×) | 236.22 µs (1.64×) |  5.06 ms (35.23×) | 726.66 µs (5.06×) | 581.26 µs (4.05×) | 2.99 ms (20.80×) | lora      |
| where_id_in_range     | 1000 | 142.71 µs (2.02×) | 187.97 µs (2.66×) |       70.55 µs ⭐ | 7.54 ms (106.85×) | 433.95 µs (6.15×) | 663.12 µs (9.40×) | 3.13 ms (44.34×) | grafeo    |
| where_in_list         | 1000 |      162.42 µs ⭐ | 196.06 µs (1.21×) | 270.30 µs (1.66×) |  5.59 ms (34.40×) | 606.63 µs (3.74×) | 627.53 µs (3.86×) | 2.92 ms (17.99×) | lora      |
| where_modulo_eq       | 1000 |      127.32 µs ⭐ | 167.83 µs (1.32×) | 250.88 µs (1.97×) | 128.48 µs (1.01×) | 717.29 µs (5.63×) | 557.09 µs (4.38×) | 3.01 ms (23.66×) | lora      |
| where_not             | 1000 | 166.05 µs (1.01×) |      164.07 µs ⭐ | 340.52 µs (2.08×) |  8.27 ms (50.40×) |   1.24 ms (7.57×) | 695.07 µs (4.24×) | 4.64 ms (28.27×) | kuzu      |
| where_or              | 1000 |      147.16 µs ⭐ | 185.19 µs (1.26×) | 450.08 µs (3.06×) |  8.43 ms (57.26×) | 627.82 µs (4.27×) | 603.10 µs (4.10×) | 3.29 ms (22.34×) | lora      |
| where_starts_with     | 1000 |      146.91 µs ⭐ | 168.08 µs (1.14×) | 249.22 µs (1.70×) |  5.12 ms (34.86×) | 727.06 µs (4.95×) | 596.02 µs (4.06×) | 3.29 ms (22.38×) | lora      |
| where_string_gte      | 1000 | 180.97 µs (1.07×) |      168.40 µs ⭐ | 247.34 µs (1.47×) |  6.05 ms (35.91×) |   1.20 ms (7.10×) | 612.78 µs (3.64×) | 4.11 ms (24.43×) | kuzu      |
| where_subexpr         | 1000 | 227.59 µs (1.69×) | 194.85 µs (1.45×) | 589.01 µs (4.37×) |      134.71 µs ⭐ |  1.76 ms (13.08×) | 584.16 µs (4.34×) | 4.73 ms (35.11×) | surrealdb |
| where_two_props       | 1000 |      152.69 µs ⭐ | 205.41 µs (1.35×) | 404.15 µs (2.65×) |  6.46 ms (42.31×) | 390.30 µs (2.56×) | 589.64 µs (3.86×) | 2.56 ms (16.77×) | lora      |

## strings _(5)_

| Workload         | Size |              lora |         kuzu |            grafeo |        surrealdb |         memgraph |             neo4j | Winner |
| ---------------- | ---: | ----------------: | -----------: | ----------------: | ---------------: | ---------------: | ----------------: | ------ |
| string_concat    | 1000 | 186.55 µs (1.27×) | 147.24 µs ⭐ | 283.23 µs (1.92×) | 5.86 ms (39.77×) | 1.68 ms (11.38×) | 577.43 µs (3.92×) | kuzu   |
| string_size      | 1000 | 172.17 µs (1.10×) | 156.38 µs ⭐ | 251.12 µs (1.61×) | 5.23 ms (33.47×) | 1.70 ms (10.90×) | 567.80 µs (3.63×) | kuzu   |
| string_substring | 1000 | 210.20 µs (1.21×) | 173.57 µs ⭐ | 314.23 µs (1.81×) | 5.58 ms (32.15×) | 2.06 ms (11.88×) | 877.36 µs (5.05×) | kuzu   |
| string_to_lower  | 1000 | 201.00 µs (1.33×) | 151.22 µs ⭐ | 297.66 µs (1.97×) | 5.03 ms (33.29×) | 1.72 ms (11.36×) | 684.21 µs (4.52×) | kuzu   |
| string_to_upper  | 1000 | 185.41 µs (1.22×) | 151.96 µs ⭐ | 292.95 µs (1.93×) | 4.91 ms (32.34×) | 1.67 ms (11.02×) | 603.96 µs (3.97×) | kuzu   |

**Notes**

- `string_to_upper` · **helixdb** — Omitted: HelixDB's DSL has no scalar string functions (upper/lower/substring/length/concat) — only property filters and graph traversal.
- `string_to_lower` · **helixdb** — Omitted: no scalar string functions in the DSL (see string_to_upper).
- `string_substring` · **helixdb** — Omitted: no scalar string functions in the DSL (see string_to_upper).
- `string_size` · **helixdb** — Omitted: no scalar string functions in the DSL (see string_to_upper).
- `string_concat` · **helixdb** — Omitted: no scalar string functions in the DSL (see string_to_upper).

## numerics _(6)_

| Workload       | Size |              lora |              kuzu |            grafeo |        surrealdb |         memgraph |             neo4j |          helixdb | Winner |
| -------------- | ---: | ----------------: | ----------------: | ----------------: | ---------------: | ---------------: | ----------------: | ---------------: | ------ |
| numeric_abs    | 1000 | 174.85 µs (1.13×) |      154.99 µs ⭐ | 272.48 µs (1.76×) | 5.86 ms (37.81×) | 1.64 ms (10.59×) | 636.05 µs (4.10×) |                — | kuzu   |
| numeric_ceil   | 1000 | 171.69 µs (1.10×) |      155.40 µs ⭐ | 269.13 µs (1.73×) | 5.81 ms (37.36×) | 1.78 ms (11.46×) | 634.65 µs (4.08×) |                — | kuzu   |
| numeric_floor  | 1000 | 175.48 µs (1.10×) |      159.62 µs ⭐ | 326.82 µs (2.05×) | 5.99 ms (37.52×) | 1.64 ms (10.29×) | 609.30 µs (3.82×) |                — | kuzu   |
| numeric_modulo | 1000 | 144.95 µs (1.02×) |      142.67 µs ⭐ | 236.39 µs (1.66×) |                — | 1.59 ms (11.17×) | 637.61 µs (4.47×) | 2.97 ms (20.79×) | kuzu   |
| numeric_pow    | 1000 | 166.22 µs (1.16×) |      143.34 µs ⭐ | 394.61 µs (2.75×) | 8.42 ms (58.75×) | 1.68 ms (11.73×) | 641.94 µs (4.48×) | 2.87 ms (20.00×) | kuzu   |
| numeric_round  | 1000 |      180.68 µs ⭐ | 181.69 µs (1.01×) | 269.75 µs (1.49×) | 5.87 ms (32.48×) |  1.68 ms (9.30×) | 581.78 µs (3.22×) |                — | lora   |

**Notes**

- `numeric_abs` · **helixdb** — Omitted: HelixDB's Expr supports +,-,*,/,% but no abs/floor/ceil/round.
- `numeric_modulo` · **surrealdb** — Omitted: SurrealQL parser rejects bare `%` inside SELECT projections (same parse limit as grouped_aggregation).
- `numeric_floor` · **helixdb** — Omitted: HelixDB's Expr supports +,-,*,/,% but no abs/floor/ceil/round.
- `numeric_ceil` · **helixdb** — Omitted: HelixDB's Expr supports +,-,*,/,% but no abs/floor/ceil/round.
- `numeric_round` · **helixdb** — Omitted: HelixDB's Expr supports +,-,*,/,% but no abs/floor/ceil/round.

## aggregates _(9)_

| Workload                 | Size |             lora |               kuzu |            grafeo |         surrealdb |           memgraph |              neo4j |          helixdb | Winner |
| ------------------------ | ---: | ---------------: | -----------------: | ----------------: | ----------------: | -----------------: | -----------------: | ---------------: | ------ |
| aggregate_avg            | 1000 |      81.43 µs ⭐ |  253.54 µs (3.11×) | 202.26 µs (2.48×) |  6.22 ms (76.38×) |  562.93 µs (6.91×) |  606.76 µs (7.45×) | 3.20 ms (39.29×) | lora   |
| aggregate_collect        | 1000 |      78.90 µs ⭐ |  275.13 µs (3.49×) | 214.58 µs (2.72×) |  6.79 ms (86.03×) |  599.26 µs (7.60×) |  619.24 µs (7.85×) |                — | lora   |
| aggregate_count          | 1000 | 59.50 µs (2.77×) | 250.00 µs (11.66×) |       21.44 µs ⭐ | 209.05 µs (9.75×) | 381.02 µs (17.77×) | 588.34 µs (27.44×) |                — | grafeo |
| aggregate_count_distinct | 1000 |     104.05 µs ⭐ |  441.70 µs (4.24×) | 213.10 µs (2.05×) |                 — |  540.27 µs (5.19×) |  635.68 µs (6.11×) |                — | lora   |
| aggregate_max            | 1000 |      79.97 µs ⭐ |  263.33 µs (3.29×) | 200.95 µs (2.51×) |  6.08 ms (75.98×) |  572.73 µs (7.16×) |  619.56 µs (7.75×) | 2.86 ms (35.74×) | lora   |
| aggregate_min            | 1000 |      79.67 µs ⭐ |  258.01 µs (3.24×) | 197.17 µs (2.47×) |  5.95 ms (74.66×) |  552.13 µs (6.93×) |  563.60 µs (7.07×) | 2.49 ms (31.30×) | lora   |
| aggregate_sum            | 1000 |      78.09 µs ⭐ |  263.06 µs (3.37×) | 199.65 µs (2.56×) |  6.23 ms (79.76×) |  524.56 µs (6.72×) |  568.81 µs (7.28×) | 2.82 ms (36.08×) | lora   |
| grouped_aggregation      | 1000 |     154.46 µs ⭐ |  549.19 µs (3.56×) | 269.45 µs (1.74×) |                 — |  751.51 µs (4.87×) |  969.01 µs (6.27×) |                — | lora   |
| top_k                    | 1000 |     186.50 µs ⭐ |  248.58 µs (1.33×) | 424.61 µs (2.28×) |  6.40 ms (34.33×) |  955.44 µs (5.12×) |  792.15 µs (4.25×) |                — | lora   |

**Notes**

- `aggregate_count` · **helixdb** — Omitted: the HelixDB enterprise-dev image rejects `count()` dynamic queries with `rate limit exceeded` (its other 9 workloads run fine). The count handler is wired in helixdb.rs and would run on a server without that limit.
- `aggregate_collect` · **helixdb** — Omitted: aggregate_by offers Count/Sum/Min/Max/Mean, no list collect.
- `aggregate_count_distinct` · **helixdb** — Omitted: needs count(DISTINCT); count() is rate-limited on the enterprise-dev image and value-level DISTINCT isn't exposed.
- `aggregate_count_distinct` · **surrealdb** — Omitted: count(DISTINCT) has no direct SurrealQL aggregate; requires nested SELECT + array::distinct.
- `grouped_aggregation` · **helixdb** — Omitted: group_count groups by a stored property, not a computed value % 10 key.
- `grouped_aggregation` · **surrealdb** — Omitted: SurrealQL rejects `%` inside a SELECT projection that's then used as a GROUP BY key (parse error).

## pipeline _(9)_

| Workload                   | Size |              lora |              kuzu |            grafeo |        surrealdb |          memgraph |             neo4j |          helixdb | Winner |
| -------------------------- | ---: | ----------------: | ----------------: | ----------------: | ---------------: | ----------------: | ----------------: | ---------------: | ------ |
| case_when                  | 1000 |      173.33 µs ⭐ | 186.25 µs (1.07×) | 256.32 µs (1.48×) | 7.38 ms (42.60×) |  1.86 ms (10.75×) | 796.60 µs (4.60×) | 2.78 ms (16.05×) | lora   |
| coalesce_existing          | 1000 |      161.92 µs ⭐ | 162.41 µs (1.00×) | 260.13 µs (1.61×) | 7.51 ms (46.39×) |   1.62 ms (9.99×) | 628.36 µs (3.88×) | 3.32 ms (20.50×) | lora   |
| computed_in_return         | 1000 | 151.15 µs (1.07×) |      140.92 µs ⭐ | 241.22 µs (1.71×) | 7.35 ms (52.15×) |  1.61 ms (11.46×) | 639.09 µs (4.54×) | 2.99 ms (21.20×) | kuzu   |
| distinct_with_order        | 1000 | 508.01 µs (2.08×) | 549.89 µs (2.25×) |      243.88 µs ⭐ |                — | 692.42 µs (2.84×) | 617.76 µs (2.53×) |                — | grafeo |
| predicate_via_function     | 1000 | 238.39 µs (1.38×) |      172.38 µs ⭐ | 434.70 µs (2.52×) | 7.45 ms (43.19×) |  1.77 ms (10.29×) | 544.38 µs (3.16×) |                — | kuzu   |
| with_aggregate_then_filter | 1000 |      148.70 µs ⭐ | 475.77 µs (3.20×) | 267.49 µs (1.80×) |                — | 567.44 µs (3.82×) | 710.00 µs (4.77×) |                — | lora   |
| with_distinct_then_count   | 1000 |      202.72 µs ⭐ | 540.85 µs (2.67×) | 244.75 µs (1.21×) |                — | 597.19 µs (2.95×) | 812.09 µs (4.01×) |                — | lora   |
| with_pipeline              | 1000 |      186.41 µs ⭐ | 333.60 µs (1.79×) | 218.95 µs (1.17×) | 5.98 ms (32.10×) | 678.15 µs (3.64×) | 581.26 µs (3.12×) |                — | lora   |
| with_two_chained           | 1000 | 313.64 µs (1.41×) |      222.76 µs ⭐ | 388.95 µs (1.75×) | 8.12 ms (36.46×) |   1.22 ms (5.46×) | 608.48 µs (2.73×) | 4.25 ms (19.06×) | kuzu   |

**Notes**

- `with_pipeline` · **helixdb** — Omitted: returns count(...); count() is rate-limited on the enterprise-dev image (see aggregate_count).
- `with_distinct_then_count` · **helixdb** — Omitted: count() rate-limited and no value-level DISTINCT.
- `with_distinct_then_count` · **surrealdb** — Omitted: DISTINCT on `value` needs SELECT VALUE, where `value` is reserved (same parse limit as the `distinct` workload).
- `with_aggregate_then_filter` · **helixdb** — Omitted: group-then-having on a computed key (value % 10) isn't expressible; group_count keys on a stored property.
- `with_aggregate_then_filter` · **surrealdb** — Omitted: groups on `value % 10`; SurrealQL rejects `%` in a projection used as a GROUP BY key (same parse limit as grouped_aggregation).
- `predicate_via_function` · **helixdb** — Omitted: WHERE size(name) needs a length() scalar the DSL doesn't provide.
- `distinct_with_order` · **helixdb** — Omitted: no value-level DISTINCT (see distinct).
- `distinct_with_order` · **surrealdb** — Omitted: DISTINCT + ORDER BY on `value`, which is reserved in both SELECT VALUE and ORDER BY (see distinct, order_by_multi_key).

## lists _(3)_

| Workload             | Size |              lora |                kuzu |            grafeo |         surrealdb |            memgraph |               neo4j |          helixdb | Winner |
| -------------------- | ---: | ----------------: | ------------------: | ----------------: | ----------------: | ------------------: | ------------------: | ---------------: | ------ |
| list_in_construction | 1000 | 177.94 µs (1.05×) |        168.89 µs ⭐ | 450.42 µs (2.67×) |  6.14 ms (36.36×) |    2.05 ms (12.12×) |   733.38 µs (4.34×) | 2.67 ms (15.81×) | kuzu   |
| list_unwind_explicit | 1000 |        1.11 µs ⭐ | 165.87 µs (149.40×) |   7.11 µs (6.40×) | 33.52 µs (30.19×) | 322.60 µs (290.57×) | 689.53 µs (621.07×) |                — | lora   |
| range_function       | 1000 |       18.86 µs ⭐ |  193.49 µs (10.26×) |  38.62 µs (2.05×) |                 — |  404.85 µs (21.46×) |  555.15 µs (29.43×) |                — | lora   |

**Notes**

- `range_function` · **surrealdb** — Omitted: SurrealQL has no row-generating numeric range (no UNWIND/range equivalent); the explicit-list unwind is covered by list_unwind_explicit.

## sort _(3)_

| Workload           | Size |         lora |              kuzu |            grafeo |        surrealdb |          memgraph |             neo4j |          helixdb | Winner |
| ------------------ | ---: | -----------: | ----------------: | ----------------: | ---------------: | ----------------: | ----------------: | ---------------: | ------ |
| order_by_id_asc    | 1000 | 167.95 µs ⭐ | 234.56 µs (1.40×) | 226.74 µs (1.35×) | 5.24 ms (31.23×) | 738.63 µs (4.40×) | 718.84 µs (4.28×) | 3.20 ms (19.06×) | lora   |
| order_by_multi_key | 1000 | 211.99 µs ⭐ | 276.69 µs (1.31×) | 442.71 µs (2.09×) |                — | 871.82 µs (4.11×) | 554.84 µs (2.62×) | 2.80 ms (13.19×) | lora   |
| skip_limit         | 1000 | 162.19 µs ⭐ | 251.28 µs (1.55×) | 233.92 µs (1.44×) | 5.24 ms (32.30×) | 704.66 µs (4.34×) | 612.55 µs (3.78×) | 2.87 ms (17.67×) | lora   |

**Notes**

- `order_by_multi_key` · **surrealdb** — Omitted: `value` is reserved in SurrealQL ORDER BY clauses (parse error).

## traversals _(15)_

| Workload                 | Size |         lora |                kuzu |            grafeo |           surrealdb |            memgraph |               neo4j |            helixdb | Winner |
| ------------------------ | ---: | -----------: | ------------------: | ----------------: | ------------------: | ------------------: | ------------------: | -----------------: | ------ |
| direct_record_traversal  |  500 | 831.70 ns ⭐ | 255.86 µs (307.64×) | 64.09 µs (77.06×) |   62.98 µs (75.73×) | 384.41 µs (462.20×) | 616.93 µs (741.77×) | 1.55 ms (1860.62×) | lora   |
| recursive_depth2         |  500 | 968.69 ns ⭐ | 499.89 µs (516.05×) | 62.29 µs (64.30×) | 110.22 µs (113.79×) | 380.49 µs (392.78×) | 601.14 µs (620.57×) | 1.69 ms (1741.68×) | lora   |
| recursive_depth3         |  500 |   1.04 µs ⭐ | 540.33 µs (519.88×) | 62.87 µs (60.49×) | 147.78 µs (142.19×) | 382.45 µs (367.97×) | 592.76 µs (570.32×) | 1.46 ms (1401.95×) | lora   |
| recursive_depth5         |  500 |   1.16 µs ⭐ | 511.05 µs (440.16×) | 62.30 µs (53.66×) | 225.94 µs (194.59×) | 426.52 µs (367.35×) | 606.31 µs (522.20×) | 1.65 ms (1424.57×) | lora   |
| relation_filter          |  500 | 117.78 µs ⭐ |   406.36 µs (3.45×) |                 — |  18.70 ms (158.81×) |   868.53 µs (7.37×) |   586.82 µs (4.98×) |   2.50 ms (21.19×) | lora   |
| traversal_count_one_hop  |  500 |  59.36 µs ⭐ |   288.00 µs (4.85×) | 141.39 µs (2.38×) |   106.91 µs (1.80×) |   394.49 µs (6.65×) |  609.51 µs (10.27×) |                  — | lora   |
| traversal_filter_one_hop |  500 | 138.99 µs ⭐ |   405.25 µs (2.92×) | 259.07 µs (1.86×) |  21.51 ms (154.73×) |     1.04 ms (7.49×) |   582.58 µs (4.19×) |   3.15 ms (22.66×) | lora   |
| traversal_one_hop        |  500 | 125.98 µs ⭐ |   360.18 µs (2.86×) | 263.65 µs (2.09×) |  25.78 ms (204.62×) |     1.14 ms (9.06×) |   570.56 µs (4.53×) |   2.59 ms (20.53×) | lora   |
| traversal_reverse        |  500 | 123.67 µs ⭐ |   365.12 µs (2.95×) | 266.69 µs (2.16×) |  24.71 ms (199.82×) |     1.10 ms (8.93×) |   591.25 µs (4.78×) |   2.62 ms (21.16×) | lora   |
| traversal_three_hop      |  500 | 236.10 µs ⭐ |     1.13 ms (4.80×) | 556.10 µs (2.36×) |  60.59 ms (256.63×) |     1.23 ms (5.21×) |   607.69 µs (2.57×) |   5.16 ms (21.86×) | lora   |
| traversal_two_hop        |  500 | 165.58 µs ⭐ |   649.69 µs (3.92×) | 411.83 µs (2.49×) |  44.18 ms (266.85×) |     1.27 ms (7.66×) |   555.27 µs (3.35×) | 64.61 ms (390.24×) | lora   |
| traversal_undirected     |  500 | 213.41 µs ⭐ |   492.51 µs (2.31×) | 494.10 µs (2.32×) |                   — |     1.76 ms (8.26×) |   637.72 µs (2.99×) |   4.20 ms (19.66×) | lora   |
| variable_length_path     |  100 |  86.24 µs ⭐ |    2.64 ms (30.62×) | 210.43 µs (2.44×) |                   — |   809.87 µs (9.39×) |   605.82 µs (7.02×) |                  — | lora   |
| varlen_2_to_5            |  100 | 123.74 µs ⭐ |    3.90 ms (31.50×) | 274.33 µs (2.22×) |                   — |   986.89 µs (7.98×) |   578.29 µs (4.67×) |                  — | lora   |
| varlen_exact_5           |  100 |  56.89 µs ⭐ |    3.86 ms (67.92×) | 140.52 µs (2.47×) |                   — |  576.13 µs (10.13×) |  586.35 µs (10.31×) |                  — | lora   |

**Notes**

- `traversal_undirected` · **surrealdb** — Omitted: SurrealDB graph edges are directional; the undirected `-[:NEXT]-` pattern has no single like-for-like arrow form (forward and reverse are covered by traversal_one_hop and traversal_reverse).
- `variable_length_path` · **helixdb** — Omitted: range-bounded variable-length expansion isn't mapped; fixed depths are benched as recursive_depth2/3/5.
- `variable_length_path` · **surrealdb** — Omitted: SurrealQL recursive traversal takes a fixed depth `@{n}` (see recursive_depth2/3/5); a `1..3` range expanded from every start node has no like-for-like form.
- `varlen_2_to_5` · **helixdb** — Omitted: range-bounded variable-length expansion isn't mapped (see variable_length_path).
- `varlen_2_to_5` · **surrealdb** — Omitted: range-bounded variable-length expansion from every start node; see variable_length_path.
- `varlen_exact_5` · **helixdb** — Omitted: range-bounded variable-length expansion isn't mapped (see variable_length_path).
- `varlen_exact_5` · **surrealdb** — Omitted: fixed-depth expansion from every start node; the anchored single-source equivalent is benched as recursive_depth5.
- `relation_filter` · **grafeo** — Omitted: grafeo's `create_edge` facade takes no edge properties, so the chain fixture has no `step` to filter on (see grafeo.rs seed_chain). memgraph/neo4j/kuzu seed `step` directly, so they do run this workload.

## patterns _(4)_

| Workload             | Size |         lora |              kuzu |            grafeo |          surrealdb |          memgraph |             neo4j |          helixdb | Winner |
| -------------------- | ---: | -----------: | ----------------: | ----------------: | -----------------: | ----------------: | ----------------: | ---------------: | ------ |
| edge_subquery_clause |  500 | 213.55 µs ⭐ | 465.09 µs (2.18×) |                 — |  18.19 ms (85.20×) |   1.18 ms (5.52×) | 605.92 µs (2.84×) | 4.12 ms (19.29×) | lora   |
| star_fanout          | 1000 | 138.96 µs ⭐ | 296.68 µs (2.13×) | 321.31 µs (2.31×) | 29.94 ms (215.43×) |  1.62 ms (11.69×) | 579.41 µs (4.17×) | 2.75 ms (19.79×) | lora   |
| star_fanout_count    | 1000 |  61.36 µs ⭐ | 290.42 µs (4.73×) | 193.38 µs (3.15×) | 20.35 ms (331.65×) | 500.19 µs (8.15×) | 590.08 µs (9.62×) |                — | lora   |
| star_fanout_filter   | 1000 | 112.53 µs ⭐ | 312.37 µs (2.78×) | 227.60 µs (2.02×) | 24.13 ms (214.42×) | 707.87 µs (6.29×) | 599.26 µs (5.33×) | 3.24 ms (28.82×) | lora   |

**Notes**

- `star_fanout_count` · **helixdb** — Omitted: the HelixDB enterprise-dev image rejects `count()` dynamic queries with `rate limit exceeded` (its other 9 workloads run fine); see aggregate_count.
- `edge_subquery_clause` · **grafeo** — Omitted: grafeo's `create_edge` facade takes no edge properties, so the social fixture has no `strength` to filter on (see grafeo.rs seed_social). memgraph/neo4j/kuzu seed `strength` directly, so they do run this workload.
