# Node binding microbench

Times four read-shaped workloads through `@loradb/lora-node` to surface
the cost of result serialization across the FFI boundary.

## Running

```sh
npm run build:native        # rebuild the .node addon
npm run build:ts            # rebuild the JS entry under dist/
npm run bench               # ~60s
```

`bench/run.mjs` writes a JSON sidecar at `bench/last.json` so before/after
diffs are mechanical. Capture a baseline before changing the binding code:

```sh
npm run bench
cp bench/last.json bench/before.json

# ... change code, rebuild ...

npm run bench
diff <(jq '.results' bench/before.json) <(jq '.results' bench/last.json)
```

## Workloads

| name              | shape                       | what it measures                          |
| ----------------- | --------------------------- | ----------------------------------------- |
| `point_read_10`   | 10 rows × 2 cols            | FFI overhead with negligible row work     |
| `medium_scan_10k` | 10 000 rows × 1 col         | bulk read path; per-row allocation cost   |
| `wide_row_1k_x_50`| 1 000 rows × 50 cols        | per-cell + per-column-name overhead       |
| `nested_list_1k`  | 1 000 rows, list of 10 ints | nested `LoraValue` → JS conversion        |

## Native baseline

For an apples-to-apples comparison against the engine without any binding
overhead, run the `lora-database` criterion suite:

```sh
cargo bench -p lora-database --bench perf_smoke
```

The Phase 1 exit criterion is ≥2× speedup on `medium_scan_10k` against a
baseline captured on the same machine in the same session — relative
deltas matter more than the absolute numbers.
