# Perf smoke benchmark

A tiny, CI-friendly Criterion suite that runs on every PR and push to `main`
to catch **obvious, large performance regressions** (≥3× slower) in core
engine paths. This is a canary, not a measurement instrument.

## What it is

- **Binary:** `crates/lora-database/benches/perf_smoke_benchmarks.rs`
- **Baseline:** `crates/lora-database/benches/perf_smoke_baseline.json`
- **Check script:** `scripts/check-perf-smoke.mjs`
- **Workflow:** `.github/workflows/perf-smoke.yml`

Four benchmarks, chosen to cover the main engine paths a regression is
likely to touch:

| Name | What it exercises |
|---|---|
| `perf_smoke/scan_1k` | `MATCH (n:Node) RETURN n.id` on 1 000 nodes — full scan + projection |
| `perf_smoke/filter_1k` | `MATCH (n:Node) WHERE n.value > 50 RETURN n.id` — predicate evaluation |
| `perf_smoke/traversal_chain_500` | `(:Chain)-[:NEXT]->(:Chain)` on a 500-node chain — edge iteration |
| `perf_smoke/write_batch_100` | `UNWIND range(1,100) CREATE (:B {...})` on a fresh DB — write path |

Each bench runs with a tight Criterion budget (300 ms warmup, 1.5 s
measurement, 30 samples). Total measurement time ≈ 7 s; total workflow
runtime ≈ 3–8 min including `cargo build --release` from a warm cache.

## What it is **not**

- **Not authoritative performance numbers.** Absolute ns/iter on
  `ubuntu-latest` varies ±20–40% run-to-run. For reproducible numbers use
  `docs/performance/benchmarks.md` and the manual `benchmarks` workflow
  against a release tag.
- **Not a replacement for the full benchmark suites.** `engine_benchmarks`,
  `scale_benchmarks`, `advanced_benchmarks`, and
  `temporal_spatial_benchmarks` still exist and are still the right tool
  for real performance work.
- **Not a tight regression gate.** The default threshold is 3× — a bench
  has to get *three times slower* before CI fails. Anything tighter
  flakes on shared-runner noise.
- **Not cross-branch comparison.** The baseline is a checked-in JSON of
  approximate ns/iter, not a previous run's artifact. Simpler, far less
  flaky.

## How regression detection works

1. CI runs `cargo bench -p lora-database --bench perf_smoke_benchmarks
   -- --output-format bencher`.
2. `scripts/check-perf-smoke.mjs` parses the bencher output and compares
   each benchmark's mean ns/iter against the matching entry in
   `perf_smoke_baseline.json`.
3. If any bench's `current / baseline` ratio exceeds its threshold
   (default 3.0, overridable per bench), the job fails.
4. The raw bencher log is uploaded as an artifact for 14 days.

## Running it locally

```bash
# Full pipeline: bench + regression check against the checked-in baseline.
cargo bench -p lora-database --bench perf_smoke_benchmarks \
    -- --output-format bencher \
  | node scripts/check-perf-smoke.mjs
```

Or piece by piece:

```bash
cargo bench -p lora-database --bench perf_smoke_benchmarks \
    -- --output-format bencher > bencher.log
node scripts/check-perf-smoke.mjs --input bencher.log
```

## Refreshing the baseline

Refresh deliberately, not reflexively. Reasons a refresh is appropriate:

- You've intentionally regressed a benchmark (e.g. traded scan speed for
  correctness) and the new number is the new normal.
- You've intentionally improved a benchmark meaningfully and want future
  regressions to be caught relative to the new floor.
- The baseline was seeded from rough numbers and you're replacing it
  with one real CI-measured run.

```bash
# Locally: run the bench, then --update rewrites the baseline JSON in place.
cargo bench -p lora-database --bench perf_smoke_benchmarks \
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

- Absolute ns values are meaningful only relative to their own baseline;
  the baseline was seeded from rough estimates and will need a real
  refresh on the first green CI run.
- `ubuntu-latest` runner variance means a single flake is possible;
  re-running the job resolves it in practice, and a real regression
  reproduces.
- The four bench cases are a sample, not a spec — a regression that
  only affects, say, temporal arithmetic will not be caught here. The
  full `benchmarks` workflow exists for that.
