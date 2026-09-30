# Perf smoke benchmark

A tiny, CI-friendly Criterion suite that runs on every PR and push to `main`
to catch **obvious, large performance regressions** (2–3× slower) in core
engine paths. This is a canary, not a measurement instrument. The
[concurrency guard](#concurrency-guard) at the end of this page is its
tighter, local counterpart.

## What it is

- **Binary:** `crates/lora-database/benches/perf_smoke.rs`
- **Baseline:** `crates/lora-database/benches/perf_smoke_baseline.json`
- **Check script:** `scripts/check-perf-smoke.mjs`
- **Summary script:** `scripts/summarize-benchmarks.mjs`
- **Workflow:** `.github/workflows/perf-smoke.yml`

The smoke suite covers the main engine paths a regression is likely to
touch:

| Name | What it exercises |
|---|---|
| `perf_smoke/scan_1k` | `MATCH (n:Node) RETURN n.id` on 1 000 nodes — full scan + projection |
| `perf_smoke/filter_1k` | `MATCH (n:Node) WHERE n.value > 50 RETURN n.id` — predicate evaluation |
| `perf_smoke/traversal_chain_500` | `(:Chain)-[:NEXT]->(:Chain)` on a 500-node chain — edge iteration |
| `perf_smoke/write_batch_100` | `UNWIND list.range(1,100) CREATE (:B {...})` on a fresh DB — write path |
| `perf_smoke/stream_*` | Streaming read/write surfaces, including lazy pull and `ORDER BY` into write |
| `perf_smoke/tx_*` | Explicit transaction round-trip, read, and write paths |
| `perf_smoke/*_wal_group` | Auto-commit / batched / explicit-tx writes, updates, deletes and a scan against a WAL-backed DB with `SyncMode::GroupSync` — catches regressions on the durability path |
| `perf_smoke/write_one_wal_persistent` | One auto-commit write against a persistent WAL directory |
| `perf_smoke/wal_replay_100` | Reopening a WAL and replaying 100 committed writes |

There are 19 benches (count the entries in `perf_smoke_baseline.json`).
Each runs with a tight Criterion budget (300 ms warmup, 1.5 s
measurement, 30 samples), so measurement takes about 35 s; total workflow
runtime ≈ 3–8 min including `cargo build --release` from a warm cache.

## What it is **not**

- **Not authoritative performance numbers.** Absolute ns/iter on
  `ubuntu-latest` varies ±20–40% run-to-run. For release numbers use the
  manual `benchmarks` workflow against a release tag; see
  [Benchmarks](benchmarks.md).
- **Not a replacement for the full benchmark suites.** Use
  `query_implementations` for query-feature coverage, and `scale`,
  `realistic`, `wal`, `concurrent`, or `concurrency_guard` for deeper
  workload-specific performance work.
- **Not a tight regression gate.** The check script's default threshold
  is 3×; the baseline overrides it to 2× for the in-memory benches and
  keeps 3× for sub-µs and WAL-backed ones, where shared-runner and fsync
  variance is structurally higher. Anything tighter flakes on
  shared-runner noise.
- **Not cross-branch comparison.** The baseline is a checked-in JSON of
  approximate ns/iter, not a previous run's artifact. Simpler, far less
  flaky.

## How regression detection works

1. CI runs `cargo bench -p lora-database --bench perf_smoke
   -- --output-format bencher`.
2. `scripts/check-perf-smoke.mjs` parses the bencher output and compares
   each benchmark's mean ns/iter against the matching entry in
   `perf_smoke_baseline.json`.
3. If any bench's `current / baseline` ratio exceeds its threshold
   (default 3.0, overridden per bench in the baseline), the job fails.
4. `scripts/summarize-benchmarks.mjs` writes
   `benchmark-summary.json`, a machine-readable current-state summary
   with per-benchmark ns/iter, error, group rollups, baseline ratios,
   new benches, missing baseline entries, and regressions.
5. The raw bencher log and JSON summary are uploaded as artifacts for
   14 days.

## Running it locally

```bash
# Full pipeline: bench + regression check against the checked-in baseline.
cargo bench -p lora-database --bench perf_smoke \
    -- --output-format bencher \
  | node scripts/check-perf-smoke.mjs
```

Or piece by piece:

```bash
cargo bench -p lora-database --bench perf_smoke \
    -- --output-format bencher > bencher.log
node scripts/check-perf-smoke.mjs --input bencher.log
```

## Refreshing the baseline

Refresh deliberately, not reflexively. Reasons a refresh is appropriate:

- You've intentionally regressed a benchmark (e.g. traded scan speed for
  correctness) and the new number is the new normal.
- You've intentionally improved a benchmark meaningfully and want future
  regressions to be caught relative to the new floor.
- CI hardware or the toolchain changed and every bench moved together.

```bash
# Locally: run the bench, then --update rewrites the baseline JSON in place.
cargo bench -p lora-database --bench perf_smoke \
    -- --output-format bencher \
  | node scripts/check-perf-smoke.mjs --update
```

Commit the change to `perf_smoke_baseline.json` in a dedicated PR and
note **why** in the commit message. Per-bench `threshold` overrides
(for genuinely noisy cases) are preserved across `--update`.

## Tuning knobs

- `--threshold <n>` on the check script overrides the default multiplier.
- `benchmarks["<name>"].threshold` in the baseline JSON overrides the
  default for a single bench.
- If a bench becomes chronically flaky, prefer widening its threshold
  over removing it — a 5× gate on a noisy bench still catches a
  catastrophic regression.

## Residual limitations

- Absolute ns values are meaningful only relative to their own baseline,
  which holds CI-measured numbers (last realigned in `dd6d9aef`; the date
  is in the baseline's `_meta.last_updated`).
- `ubuntu-latest` runner variance means a single flake is possible;
  re-running the job resolves it in practice, and a real regression
  reproduces.
- The bench cases are a sample, not a spec — a regression that only
  affects, say, temporal arithmetic will not be caught here. The full
  `benchmarks` workflow exists for that.

## Concurrency guard

Use this guard while implementing concurrent reads, concurrent writes, WAL
commit changes, and concurrent file syncs. It compares two runs from the same
machine/session, so it can use a much tighter threshold than the `perf_smoke`
gate. The phase-by-phase plan it guards is
[`docs/design/concurrency-implementation-plan.md`](../design/concurrency-implementation-plan.md).

- **Binary:** `crates/lora-database/benches/concurrency_guard.rs`
- **Check script:** `scripts/check-bench-delta.mjs`

### Run it for each phase

```bash
cargo bench -p lora-database --bench concurrency_guard \
    -- --output-format bencher > /tmp/lora-before.bencher

# make one implementation step

cargo bench -p lora-database --bench concurrency_guard \
    -- --output-format bencher > /tmp/lora-after.bencher

node scripts/check-bench-delta.mjs \
    --baseline /tmp/lora-before.bencher \
    --current /tmp/lora-after.bencher \
    --threshold 1.15
```

The default threshold is `1.15`, meaning a benchmark may be at most 15 percent
slower than the baseline run. For noisy filesystem work, rerun once before
assuming a regression is real.

### What it covers

- `read_scan_1k`: snapshot read query on 1,000 nodes.
- `read_scan_50k`: unlabelled scan projecting one property on 50,000 nodes.
- `read_label_project_50k`: label scan projecting three properties.
- `read_scan_filter_project_50k`: filter that keeps every row, then project.
- `read_scan_filter_half_project_50k`: filter that keeps about half the rows.
- `read_map_projection_50k`: map projection (`n { .id, .name, .value }`).
- `stream_pull_one_1k`: live stream open, pull one row, drop.
- `write_create_one_steady`: auto-commit create on a long-lived database.
- `write_set_existing_1k`: auto-commit update of an existing record.
- `tx_roundtrip_empty`: explicit read-write transaction fixed cost.
- `tx_write_create_one`: explicit write transaction with one commit.
- `mixed_4_readers_1_writer`: coarse mixed read/write thread pressure.
- `wal_group_sync_create_delete_one`: WAL encode/flush-buffer path plus
  GroupSync background fsync coordination.
- `wal_group_create_delete_one`: legacy GroupSync guard name retained for
  older benchmark comparisons.

### Interpreting results

Treat this as a phase gate, not a release benchmark. A failure means "pause and
understand this before stacking more concurrency work on top." If the slowdown
is intentional, capture it in the phase notes and use the new run as the next
phase's baseline.
