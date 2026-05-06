# Concurrency Performance Guard

Use this guard while implementing concurrent reads, concurrent writes, WAL
commit changes, and concurrent file syncs. It compares two runs from the same
machine/session, so it can use a much tighter threshold than the broad CI
`perf_smoke` gate.

The phase-by-phase implementation plan lives in
[`docs/design/concurrency-implementation-plan.md`](../design/concurrency-implementation-plan.md).

## Run It For Each Phase

```bash
cargo bench -p lora-database --bench concurrency_guard_benchmarks \
    -- --output-format bencher > /tmp/lora-before.bencher

# make one implementation step

cargo bench -p lora-database --bench concurrency_guard_benchmarks \
    -- --output-format bencher > /tmp/lora-after.bencher

node scripts/check-bench-delta.mjs \
    --baseline /tmp/lora-before.bencher \
    --current /tmp/lora-after.bencher \
    --threshold 1.15
```

The default threshold is `1.15`, meaning a benchmark may be at most 15 percent
slower than the baseline run. For noisy filesystem work, rerun once before
assuming a regression is real.

## What It Covers

- `read_scan_1k`: snapshot read query on 1,000 nodes.
- `stream_pull_one_1k`: live stream open, pull one row, drop.
- `write_create_one_steady`: auto-commit create on a long-lived database.
- `write_set_existing_1k`: auto-commit update of an existing record.
- `tx_roundtrip_empty`: explicit read-write transaction fixed cost.
- `tx_write_create_one`: explicit write transaction with one commit.
- `mixed_4_readers_1_writer`: coarse mixed read/write thread pressure.
- `wal_none_create_delete_one`: WAL encode/flush-buffer path without fsync.
- `wal_group_create_delete_one`: cooperative Group-mode WAL path.

## Interpreting Results

Treat this as a phase gate, not a release benchmark. A failure means "pause and
understand this before stacking more concurrency work on top." If the slowdown
is intentional, capture it in the phase notes and use the new run as the next
phase's baseline.
