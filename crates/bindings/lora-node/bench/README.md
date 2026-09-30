# Node binding microbench

Times four read-shaped workloads through `@loradb/lora-node` to surface
the cost of result serialization across the FFI boundary, plus an empty
result (the fixed round-trip floor) and a native-only variant of the
10k scan.

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
| `ffi_overhead_0_rows` | empty result            | the promise round-trip floor              |
| `medium_scan_10k::native_only` | 10 000 rows, no JS decode | native encode cost without the decoder |

## Wire format

`execute()` and the per-statement results inside `transaction()` ship a
single binary `Buffer` to JS instead of building a JS object tree on the
main thread; the TS wrapper decodes it once into the public
`{ columns, rows }` shape. The format (little-endian, `LR1\0` magic,
column header, then row-major tagged cells) is documented at the top of
`crates/bindings/lora-binding-buffer/src/lib.rs`, which holds the shared
encoder for the Node, WASM and FFI bindings. The one reader is
`crates/bindings/shared-ts/decode.ts`, synced into `ts/decode.ts` by
`sync:types`; keep the two byte-compatible.

This is an internal contract. `db.execute()` still resolves with
`{ columns, rows }` and `db.transaction()` with `Array<{ columns, rows }>`;
the `Buffer` only appears in `native.d.ts`, which is not part of the
public package surface. `db.stream()` and `db.streamRows()` do not use
the buffer.

## Optimizations applied

1. **Direct napi value construction.** Removed the `serde_json::Value`
   middle layer: `Task::compute` returns owned Rust data and
   `Task::resolve` builds JS values directly. Was 2–3 walks over every
   cell, now 1.

2. **Column-key interning.** Column names are created once as
   `JsString` and reused via `set_property` for each row, instead of
   `set_named_property(&str, …)` doing one `CString::new` per cell.

3. **Bulk-buffer encoding (the big one).** See "Wire format" above.
   Per-cell napi calls dominated the old path; one buffer transfer
   replaces all of them. V8 walks contiguous bytes far faster than
   napi can hand individual values across.

4. **Compact i32 tag.** Ints that fit in `i32` go on the wire as a
   1-byte tag + 4-byte payload (instead of 8). Shrinks the buffer by
   ~40% on graph-id-heavy results and lets the JS decoder use
   `getInt32` instead of `BigInt64Array`-backed `Number(BigInt)`.

5. **Per-shape row factory.** The decoder caches a `new Function`-built
   factory keyed by the column-name fingerprint. The factory body
   contains *static* property assignments, so V8 sees a fixed object
   literal shape and shares one hidden class across every row of the
   query — dynamic `row[colName] = …` would force each row through its
   own bootstrap.

6. **Skip the RowArrays projection.** The encoder iterates `Row`
   entries directly instead of going through
   `lora_executor::value::row_to_array`, which does an O(C) linear scan
   per column for every row — quadratic in column count. This was the
   biggest win on `wide_row_1k_x_50` (~4.7× at the time).

7. **`Row::iter()` not `iter_named()` in the body.** Names are only
   needed once for the header; each row body uses the cheaper iterator
   that doesn't allocate a `Cow<str>` per cell.

8. **Batched primitive writes.** Tag + payload go through a single
   `extend_from_slice` of a stack-allocated `[u8; 5]` (i32) or
   `[u8; 9]` (i64 / f64). The Vec then sees one bounds check and one
   memcpy instead of two pushes — meaningful on tall tables.

Together these took `medium_scan_10k` from ~5.8 ms to ~2.6 ms on Apple
Silicon; after them, most of the time is engine execution rather than
the binding. Re-measure with the steps above rather than trusting those
figures.

## Native baseline

For an apples-to-apples comparison against the engine without any binding
overhead, run the `lora-database` criterion suite:

```sh
cargo bench -p lora-database --bench perf_smoke
```

The Phase 1 exit criterion is ≥2× speedup on `medium_scan_10k` against a
baseline captured on the same machine in the same session — relative
deltas matter more than the absolute numbers.
