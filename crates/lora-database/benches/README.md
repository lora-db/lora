# lora-database Benchmarks

Benchmarks are split by intent:

| Target | Purpose |
| --- | --- |
| `query_implementations` | Coverage-oriented query-language suite. Add representative benches here when a tested query implementation changes or lands. |
| `scale` | Same query families across larger graph sizes. |
| `realistic` | End-to-end domain-shaped workloads that combine several operators. |
| `perf_smoke` | Short CI canary for large regressions. |
| `wal` | Durability and recovery overhead. |
| `concurrent` | Concurrent read/write workload behavior. |
| `concurrency_guard` | Focused guardrail suite for snapshot, OCC, and WAL concurrency changes. |
| `engine`, `advanced`, `temporal_spatial` | Older deep-dive suites kept for historical comparison and detailed performance docs. Prefer `query_implementations` for new query-feature coverage. |

Run the coverage suite:

```bash
cargo bench -p lora-database --bench query_implementations
```

Run every registered database benchmark:

```bash
cargo bench -p lora-database --benches
```

## Probes (`examples/`)

Targeted measurements that are quicker to read than a Criterion run and
compare cleanly across branches (build each branch, run the same probe):

| Example | Measures |
| --- | --- |
| `heap_probe` | Real retained heap per node / relationship / index entry via a counting allocator (`MemoryReport` is an estimate). `-- --festimap` runs the 25k-node / 100k-relationship shape from the Festimap report |
| `perf_probe` | Mean latency of common query shapes; `-- <scenario> <seconds>` loops one scenario for an external sampler, `-- --stream` compares `execute()` with `stream()` |
| `optional_probe` | Anchored `OPTIONAL MATCH` vs the equivalent pattern comprehension (target: within 3x) |
| `keyset_probe` | `WHERE n.key > $after ORDER BY n.key LIMIT 20` latency at 20k and 1M nodes (target: under 1 ms p50, flat in label size) |
| `bulk_probe` | Bulk-load time with no schema vs a uniqueness constraint, range, or full-text index declared first |

```bash
cargo run --release -p lora-database --example heap_probe
```
