# Node binding bench

Hardware: Apple Silicon (darwin-arm64). 30 iterations per workload after
5 warmup. Median wall-clock measured around `db.execute()` from the JS
side, so numbers include the napi promise round-trip end-to-end.

## Results

| workload            | rows   | original (Phase 0) | current      | speedup    |
| ------------------- | ------ | ------------------ | ------------ | ---------- |
| `point_read_10`     | 10     | 1.16 ms            | 1.22 ms      | ~1×        |
| `medium_scan_10k`   | 10 000 | 5.79 ms            | 2.58 ms      | **2.24×**  |
| `wide_row_1k_x_50`  | 1 000  | 23.94 ms           | 5.14 ms      | **4.66×**  |
| `nested_list_1k`    | 1 000  | 1.30 ms            | 0.35 ms      | **3.66×**  |

`ffi_overhead_0_rows` (an empty result) sits at ~13 µs end-to-end —
that's the irreducible promise round-trip cost. `point_read_10` is
within noise of that floor and barely moves; meaningful work is below
the FFI overhead.

`before.json` and `last.json` hold the raw samples.

## Where the time goes now

For `medium_scan_10k` (10 000 rows × 1 int):

| stage                                 | µs     | %    |
| ------------------------------------- | ------ | ---- |
| FFI / promise round-trip              | ~13    | <1%  |
| Engine query execution                | ~1 700 | ~66% |
| Native encode + Buffer transfer       | ~750   | ~29% |
| JS-side decode                        | ~135   | ~5%  |

Confirmed by the `medium_scan_10k::native_only` bench (no JS decode):
the native pipeline is ~2 450 µs and the JS decode adds ~135 µs.

The remaining headroom is in the engine, not the binding. `cargo bench
perf_smoke::scan_1k` reports ~170 µs for a 1 000-row scan
through `ResultFormat::Rows`, so 10K rows extrapolate to ~1.7 ms — the
binding is now within ~1.06× of the engine itself for bulk reads on
the native side, and ~1.5× end-to-end including JS decode.

## Optimizations applied

1. **Direct napi value construction (Phase 1).** Removed the
   `serde_json::Value` middle layer: `Task::compute` returns owned Rust
   data and `Task::resolve` builds JS values directly. Was 2–3 walks
   over every cell, now 1.

2. **Column-key interning.** Column names are created once as
   `JsString` and reused via `set_property` for each row, instead of
   `set_named_property(&str, …)` doing one `CString::new` per cell.

3. **Bulk-buffer encoding (the big one).** `execute()` and the
   per-statement results inside `transaction()` now ship a single
   binary `Buffer` to JS instead of building a JS object tree on the
   main thread. The TS wrapper decodes it once into the public
   `{ columns, rows }` shape. See `src/encode.rs` and `ts/decode.ts`
   for the wire format.

   Per-cell napi syscalls dominated the old path; one buffer transfer
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

6. **Skip the RowArrays projection.** Engine returns `Vec<Row>`; the
   encoder iterates `Row` entries directly. Avoids
   `lora_executor::value::row_to_array`, which does an O(C) linear
   scan per column for every row — quadratic in column count, hence
   the 4.66× win on `wide_row_1k_x_50`.

7. **`Row::iter()` not `iter_named()` in the body.** Names are only
   needed once for the header; for each row body we use the cheaper
   iterator that doesn't allocate a `Cow<str>` per cell.

8. **Batched primitive writes.** Tag + payload go through a single
   `extend_from_slice` of a stack-allocated `[u8; 5]` (i32) or
   `[u8; 9]` (i64 / f64). The Vec then sees one bounds check and one
   memcpy instead of two pushes — meaningful on tall tables.

## API impact

None. `db.execute(query, params)` still resolves with `{ columns, rows }`
where `rows` is `Array<Record<string, LoraValue>>`. `db.transaction(...)`
still resolves with `Array<{columns, rows}>`. `db.stream()` and
`db.streamRows()` are untouched and still go through the row-by-row
napi path.

The native side now hands a `Buffer` to the TS layer and the layer
decodes it. That's an internal contract; the only TS-visible change is
the `native.d.ts` declaration which is not part of the public package
surface.

## Reproducing

```sh
cd crates/bindings/lora-node
npm run build
npm run bench
```

The bench writes `bench/last.json` for diffs. Capture a baseline before
changing binding code so the deltas are meaningful — absolute numbers
depend on the host.

## What's left

The binding is now bottlenecked by the engine. Further wins require:

- **Engine-level optimization** to cut the ~170 µs/1 000-row floor in
  `lora-executor`. Out of scope for the bindings crate.
- **Column-major buffer with TypedArray views** for primitive columns.
  Could shave another ~100 µs of JS decode but the JS side is only
  ~6% of total time, so the impact would be marginal.
- **`ThreadsafeFunction` streaming** so very large results don't hold
  the JS event loop while encoding finishes. Affects responsiveness
  rather than throughput.
