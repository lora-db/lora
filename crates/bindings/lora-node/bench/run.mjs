// Node binding microbench. Pure ESM, no build step.
//
// Loads the compiled `dist/` entry point and times four read workloads
// that stress different parts of the result-serialization path:
//
//   point_read_10      — 10 rows, 2 cols   (FFI overhead dominates)
//   medium_scan_10k    — 10000 rows, 1 col (bulk read path)
//   wide_row_1k_x_50   — 1000 rows × 50 cols (per-cell + per-column-name overhead)
//   nested_list_1k     — 1000 rows, list of 10 ints (nested LoraValue conversion)
//
// Output is human-readable text plus a JSON sidecar (`bench/last.json`)
// so before/after diffs are mechanical.

import { performance } from "node:perf_hooks";
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

import { createDatabase } from "../dist/index.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const ITERS = 30;
const WARMUP = 5;

function pct(samples, p) {
  const i = Math.min(samples.length - 1, Math.floor(samples.length * p));
  return samples[i];
}

async function timed(fn) {
  const t0 = performance.now();
  const out = await fn();
  const t1 = performance.now();
  return { ns: (t1 - t0) * 1e6, out };
}

async function bench(name, setup, run, expectedRows) {
  const db = await createDatabase();
  await setup(db);

  // Warmup so JIT, libuv pool sizing, and any one-shot caches settle.
  for (let i = 0; i < WARMUP; i++) {
    await run(db);
  }

  const samples = [];
  let rowsObserved = 0;
  for (let i = 0; i < ITERS; i++) {
    const { ns, out } = await timed(() => run(db));
    samples.push(ns);
    rowsObserved = (out.rows ?? out).length;
  }
  samples.sort((a, b) => a - b);

  if (expectedRows >= 0 && rowsObserved !== expectedRows) {
    throw new Error(
      `[${name}] expected ${expectedRows} rows, got ${rowsObserved} — bench setup drift`,
    );
  }
  if (expectedRows < 0) expectedRows = rowsObserved;

  const median = pct(samples, 0.5);
  const p99 = pct(samples, 0.99);
  const min = samples[0];
  const nsPerRow = median / Math.max(1, expectedRows);
  const rowsPerSec = expectedRows / (median / 1e9);

  await db.dispose();
  return {
    name,
    rows: expectedRows,
    iters: ITERS,
    minNs: min,
    medianNs: median,
    p99Ns: p99,
    nsPerRow,
    rowsPerSec,
  };
}

// ---------------------------------------------------------------------------
// Workload setups
// ---------------------------------------------------------------------------

async function seedNodes(db, count) {
  // Per-statement creates. Slow (this is setup, not measured) but matches
  // how fixtures.rs builds graphs in the criterion benches.
  for (let i = 0; i < count; i++) {
    await db.execute(`CREATE (:Node {id: ${i}, value: ${i % 100}})`);
  }
}

async function seedWide(db, count, cols) {
  for (let i = 0; i < count; i++) {
    const props = [];
    for (let c = 0; c < cols; c++) props.push(`p${c}: ${i * cols + c}`);
    await db.execute(`CREATE (:Wide {${props.join(", ")}})`);
  }
}

async function seedNested(db, count, listLen) {
  for (let i = 0; i < count; i++) {
    const items = [];
    for (let j = 0; j < listLen; j++) items.push(i * listLen + j);
    await db.execute(`CREATE (:Nested {tags: [${items.join(", ")}]})`);
  }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

const results = [];

// 0-row baseline: isolates pure FFI / promise-resolution overhead.
results.push(
  await bench(
    "ffi_overhead_0_rows",
    () => {},
    (db) => db.execute("RETURN 1 LIMIT 0"),
    0,
  ),
);

results.push(
  await bench(
    "point_read_10",
    (db) => seedNodes(db, 10_000),
    (db) =>
      db.execute("MATCH (n:Node) WHERE n.id < 10 RETURN n.id, n.value"),
    10,
  ),
);

results.push(
  await bench(
    "medium_scan_10k",
    (db) => seedNodes(db, 10_000),
    (db) => db.execute("MATCH (n:Node) RETURN n.id"),
    10_000,
  ),
);

{
  const cols = 50;
  const rowCount = 1_000;
  const projection = Array.from({ length: cols }, (_, c) => `n.p${c}`).join(", ");
  results.push(
    await bench(
      "wide_row_1k_x_50",
      (db) => seedWide(db, rowCount, cols),
      (db) => db.execute(`MATCH (n:Wide) RETURN ${projection}`),
      rowCount,
    ),
  );
}

results.push(
  await bench(
    "nested_list_1k",
    (db) => seedNested(db, 1_000, 10),
    (db) => db.execute("MATCH (n:Nested) RETURN n.tags"),
    1_000,
  ),
);

const fmt = (n) =>
  n >= 1_000_000 ? `${(n / 1_000_000).toFixed(2)} M` :
  n >= 1_000     ? `${(n / 1_000).toFixed(1)} K` :
  n.toFixed(1);

console.log(
  ["workload", "rows", "median µs", "p99 µs", "ns/row", "rows/s"]
    .map((s) => s.padEnd(18))
    .join(""),
);
for (const r of results) {
  console.log(
    [
      r.name.padEnd(18),
      String(r.rows).padEnd(18),
      (r.medianNs / 1000).toFixed(1).padEnd(18),
      (r.p99Ns / 1000).toFixed(1).padEnd(18),
      r.nsPerRow.toFixed(0).padEnd(18),
      fmt(r.rowsPerSec).padEnd(18),
    ].join(""),
  );
}

writeFileSync(
  resolve(HERE, "last.json"),
  JSON.stringify({ when: new Date().toISOString(), results }, null, 2),
);

// ---------------------------------------------------------------------------
// Side-channel: measure the native Buffer-only path so we can see what
// share of execute() is Rust-side (engine + encode + napi transfer)
// vs JS-side (decode). Useful to know whether further wins should
// target encoder, decoder, or the engine itself.
// ---------------------------------------------------------------------------

import nativeMod from "../dist/native.js";
const { Database: NativeDatabase } = nativeMod;

async function timeNativeOnly(name, setup, query) {
  const db = new NativeDatabase();
  for (const stmt of setup) {
    await db.execute(stmt);
  }
  // warmup
  for (let i = 0; i < WARMUP; i++) await db.execute(query);
  const samples = [];
  let bufLen = 0;
  for (let i = 0; i < ITERS; i++) {
    const t0 = performance.now();
    const buf = await db.execute(query);
    const t1 = performance.now();
    samples.push((t1 - t0) * 1e6);
    bufLen = buf.length;
  }
  samples.sort((a, b) => a - b);
  db.dispose();
  return {
    name,
    bufBytes: bufLen,
    medianNs: pct(samples, 0.5),
    p99Ns: pct(samples, 0.99),
  };
}

const nativeOnly = [];
{
  const seedStmts = [];
  for (let i = 0; i < 10_000; i++) seedStmts.push(`CREATE (:Node {id: ${i}, value: ${i % 100}})`);
  nativeOnly.push(
    await timeNativeOnly(
      "medium_scan_10k::native_only",
      seedStmts,
      "MATCH (n:Node) RETURN n.id",
    ),
  );
}

console.log("");
console.log("Native-only (no JS decode):");
for (const r of nativeOnly) {
  console.log(
    `  ${r.name.padEnd(36)} median ${(r.medianNs / 1000).toFixed(1)} µs  buf ${(r.bufBytes / 1024).toFixed(1)} KiB`,
  );
}
