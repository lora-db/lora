---
title: Performance and Benchmark Reports
sidebar_label: Performance
description: How to read LoraDB benchmark results, inspect the CI benchmark-summary.json artifact, and compare current database performance against the smoke baseline.
---

# Performance and Benchmark Reports

LoraDB benchmarks are meant to answer two different questions:

| Question | Source | Use it for |
|---|---|---|
| Is this build obviously slower? | `perf-smoke` CI workflow | Pull request review and regression triage |
| What does this release look like overall? | Manual `benchmarks` workflow | Release notes, dashboards, and capacity planning experiments |

The shared output is `benchmark-summary.json`: a compact JSON file
generated from Criterion's bencher output. It gives you a table-friendly
view of the current database state without scraping terminal logs.

:::note

CI runners are noisy. Treat these numbers as a trend signal for the same
benchmark on the same workflow, not as a hardware-independent promise.

:::

## Current Snapshot

Representative numbers below come from the Criterion benchmark suite
used for LoraDB performance tracking. Throughput units are benchmark
specific: scans count nodes, traversals count edges or paths, writes
count entities or full write operations.

| Area | Benchmark | Dataset | Mean time | Throughput | Unit | What it tells you |
|---|---|---:|---:|---:|---|---|
| Scan | `match/all_nodes/1000` | 1,000 nodes | 285.13 µs | 3,507,211 | nodes/sec | Full label scan plus projection |
| Count | `aggregation/count_star/1000` | 1,000 rows | 59.52 µs | 16,801,625 | rows/sec | Fast aggregate path without row projection |
| Traversal | `traversal/single_hop_chain/1000` | 1,000-node chain | 527.42 µs | 1,894,126 | edges/sec | Basic relationship iteration |
| Traversal | `traversal/tree_depth3_branch5_traverse` | 155 descendants | 55.63 µs | 2,786,462 | paths/sec | Bounded tree expansion |
| Ordering | `ordering/order_by_single/1000` | 1,000 rows | 853.55 µs | 1,171,582 | rows/sec | Sort cost for a single key |
| Write | `write/create_single_node` | fresh DB | 8.60 µs | 116,331 | ops/sec | Single-node write overhead |
| Write | `write/batch_create_unwind/500` | 500 creates | 554.79 µs | 901,238 | nodes/sec | Batched `UNWIND` insertion |
| Workload | `realistic/social_friend_of_friend_500` | 500 persons | 399.46 µs | 2,503 | queries/sec | Representative two-hop social query |

## Live Smoke Report

The PR-facing `perf-smoke` workflow produces a smaller table with a
baseline comparison. The table below is loaded from
<a href="/benchmarks/perf-smoke-summary.json"><code>/benchmarks/perf-smoke-summary.json</code></a>
on the live website.

<BenchmarkSummary src="/benchmarks/perf-smoke-summary.json" />

The smoke gate fails only when a benchmark is slower than its configured
threshold. The default threshold is `3.0x`, because shared CI machines
vary enough that a tight gate would be noisy instead of useful.

## JSON Shape

The summary artifact is designed to be boring to consume. The useful
top-level fields are:

| Field | Type | Purpose |
|---|---|---|
| `schema_version` | number | Incremented only when the artifact contract changes |
| `generated_at` | string | ISO timestamp for the summary generation time |
| `suite` | string | Workflow or explicit suite name, such as `perf-smoke` |
| `source.github` | object | Repository, SHA, ref, workflow, run id, and actor when produced in GitHub Actions |
| `source.runner` | object | OS, CPU architecture, and CPU count |
| `summary` | object | Benchmark count, group count, fastest and slowest benchmarks |
| `baseline` | object | Compared, ok, regressed, new, and missing benchmark counts |
| `groups` | array | Per-group rollups: count, fastest, median, p95, slowest, average |
| `benchmarks` | array | One row per benchmark, ready for table rendering |

Each `benchmarks[]` row contains:

| Field | Meaning |
|---|---|
| `name` | Full Criterion name, for example `perf_smoke/scan_1k` |
| `group` | First path segment, for example `perf_smoke` |
| `case` | Remaining path segments |
| `ns_per_iter` | Current mean time in nanoseconds per iteration |
| `error_ns` | Criterion reported error in nanoseconds |
| `relative_error_pct` | `error_ns / ns_per_iter * 100` |
| `iterations_per_second` | Derived `1_000_000_000 / ns_per_iter` |
| `baseline.status` | `ok`, `regressed`, or `new` when a baseline is supplied |
| `baseline.ratio` | `current / baseline` |
| `baseline.threshold` | Maximum allowed ratio for the smoke gate |

## Render A Table

After downloading a workflow artifact, render the benchmark rows into a
developer-readable table with `jq`:

```bash
jq -r '
  ["benchmark","current_ns","baseline_ns","ratio","status"],
  (.benchmarks[] | [
    .name,
    .ns_per_iter,
    (.baseline.ns_per_iter // ""),
    (.baseline.ratio // ""),
    (.baseline.status // "")
  ])
  | @tsv
' benchmark-summary.json
```

For Markdown:

```bash
jq -r '
  "| Benchmark | Current ns | Baseline ns | Ratio | Status |",
  "|---|---:|---:|---:|---|",
  (.benchmarks[] |
    "| `\(.name)` | \(.ns_per_iter) | \(.baseline.ns_per_iter // "") | \(.baseline.ratio // "") | \(.baseline.status // "") |"
  )
' benchmark-summary.json
```

## Reproduce Locally

Run the smoke suite and produce the same JSON shape:

```bash
cargo bench -p lora-database --bench perf_smoke \
    -- --output-format bencher > bencher.log

node scripts/summarize-benchmarks.mjs \
  --input bencher.log \
  --output benchmark-summary.json \
  --baseline crates/lora-database/benches/perf_smoke_baseline.json \
  --suite perf-smoke-local

node scripts/check-perf-smoke.mjs --input bencher.log
```

Run the full suite when you want a release-level snapshot:

```bash
cargo bench --locked -p lora-database --benches \
    -- --output-format bencher > benchmarks.log

node scripts/summarize-benchmarks.mjs \
  --input benchmarks.log \
  --output benchmark-summary.json \
  --suite release-benchmarks
```

## Reading Regressions

| Signal | Meaning | Next step |
|---|---|---|
| `baseline.regressed_count > 0` | One or more current timings exceeded threshold | Inspect the named rows in `baseline.regressions` |
| `baseline.new_count > 0` | A benchmark exists in the run but not the baseline | Add it deliberately with a baseline refresh |
| `baseline.missing_count > 0` | A baseline entry was not produced | Check for renamed or deleted benchmark cases |
| High `relative_error_pct` | Criterion saw noisy measurements | Re-run before drawing conclusions |
| One slow outlier in CI | Shared runner noise is possible | Re-run the job, then compare trend across runs |

## See Also

- [Queries → Paths](./queries/paths#performance) for path-query cost notes.
- [Limitations](./limitations#concurrency) for the current single-process
  concurrency model.
- [WAL and Checkpoints](./wal) for persistence behavior, which can affect
  write-heavy measurements when WAL is enabled.
