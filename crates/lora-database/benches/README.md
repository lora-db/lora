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
