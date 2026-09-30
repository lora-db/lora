## Performance Benchmarks

This page explains how to get current benchmark numbers for LoraDB. It
does not list numbers: a table checked into the repo goes stale with the
next engine change, and absolute timings depend on the host. Measure the
version you care about, on hardware you care about.

> ⚙️ **Note** — The Criterion suites characterise the single-process, in-memory core. They are single-query measurements and assume the whole graph fits in RAM. For distributed throughput, write-heavy concurrency, or multi-tenant isolation, see the [LoraDB managed platform](https://loradb.com).

### Numbers for a release: the `benchmarks` workflow

`.github/workflows/benchmarks.yml` is dispatched manually (Actions →
**benchmarks** → *Run workflow*) with an existing release tag:

- It checks out the tag, verifies every manifest carries that version, and
  runs every registered `lora-database` bench on `ubuntu-latest`:
  `cargo bench --locked -p lora-database --benches -- --output-format bencher`.
- `scripts/summarize-benchmarks.mjs` turns the bencher log into
  `benchmark-summary.json`: per-benchmark ns/iter and error, group rollups,
  fastest/slowest lists, runner metadata, and baseline comparisons when a
  baseline is supplied.
- The archive (`lora-server-<tag>-benchmarks.tar.gz`: the raw log, the
  summary, Criterion's HTML reports and `estimates.json`) and the summary
  JSON are uploaded as a workflow artifact for 30 days and, unless
  `attach_to_release` is unchecked, attached to the GitHub Release.

Shared runners add noise: treat the numbers as a trend across releases,
not as absolute figures.

### Numbers on your machine

```bash
# Every registered database bench (slow: tens of minutes)
cargo bench -p lora-database --benches

# One suite
cargo bench -p lora-database --bench query_implementations
cargo bench -p lora-database --bench index_acceleration

# Summarize a run the way the workflow does
cargo bench -p lora-database --benches -- --output-format bencher > bench.log
node scripts/summarize-benchmarks.mjs --input bench.log --output summary.json
```

The suites, as registered in `crates/lora-database/Cargo.toml` (see
`crates/lora-database/benches/README.md`):

| Suite | Use it for |
|---|---|
| `query_implementations` | Query-language coverage. New query-feature work adds benches here. |
| `index_acceleration` | Index work: seeks, range scans, index-ordered reads. |
| `scale` | The same query families at 10 000 and 50 000 nodes. |
| `realistic` | Domain-shaped workloads that combine several operators. |
| `wal` | Durability and recovery overhead. |
| `concurrent` | Concurrent read/write behaviour. |
| `concurrency_guard` | Same-machine before/after gate for concurrency work; see [Perf smoke](perf-smoke.md#concurrency-guard). |
| `perf_smoke` | The CI canary; see [Perf smoke](perf-smoke.md). |
| `memory` | Retained heap per element, gated by the `memory-bench` workflow. |
| `engine`, `advanced`, `temporal_spatial` | Older deep-dive suites, kept for comparison. |

For a quick answer without Criterion, the probes in
`crates/lora-database/examples/` (`perf_probe`, `heap_probe`,
`keyset_probe`, …) print latency or heap for a few shapes and compare
cleanly across branches.

### Continuous gates

Two workflows run on PRs and pushes to `main` that touch the engine
(both are path-filtered) and fail on large regressions only:

- `perf-smoke` — `perf_smoke` against a checked-in baseline. See
  [Perf smoke](perf-smoke.md).
- `memory-bench` — the `memory` bench against
  `crates/lora-database/benches/memory_baseline.json`, checked by
  `scripts/check-mem-bench.mjs`.

### Reading the results

- **Throughput units.** Criterion's throughput unit depends on the
  benchmark group. Scan, filter and aggregation groups count nodes scanned
  per query, so ops/sec reads as *nodes processed per second*. Traversal
  groups count edges or path destinations (*edges/sec*, *paths/sec*).
  Write groups and realistic workloads typically count one full query per
  iteration (*queries per second*).
- **Microbenchmark vs. workload.** The *functions*, *parse_compile*,
  *temporal_creation* and *spatial_creation* groups stabilise in a few µs
  and measure the constant-factor cost of the evaluator and planner. The
  *realistic*, *recommendation*, *scale_social* and *shortest_path* groups
  are whole-query workloads.
- **All benches are in-memory** unless they open a WAL explicitly (`wal`,
  the `*_wal_*` cases in `perf_smoke`). The live store is a slot-indexed
  `InMemoryGraph` held as an `Arc` behind a `RwLock`
  (`crates/lora-database/src/live_store.rs`); there is no buffer pool.
- **Single-query measurements.** Apart from `concurrent`,
  `concurrency_guard`'s mixed case and the WAL group-sync cases, benches
  run one query at a time; they do not measure concurrent throughput.
- **Variable-length hop cap.** Unbounded `*` paths are capped at
  `MAX_VAR_LEN_HOPS = 100`, so an unbounded chain bench measures at most
  100 hops.
- **Compare like with like.** Compare two runs from the same machine and
  session, or two workflow runs, never a laptop run with a CI one.

## Next steps

- Understand the current bottlenecks: [Performance Notes](notes.md)
- The CI canary and the concurrency guard: [Perf smoke](perf-smoke.md)
- See how the executor and storage fit together: [Data Flow](../architecture/data-flow.md), [Graph Engine](../architecture/graph-engine.md)
- If you're hitting write-lock contention or need persistent scale, check the [LoraDB managed platform](https://loradb.com)
