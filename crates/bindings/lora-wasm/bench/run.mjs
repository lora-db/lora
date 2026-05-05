// WASM binding microbench, mirrors lora-node's bench harness.
//
// We run the four shared workloads twice: once through the new
// binary-buffer path (the default after the executeBuffer wiring)
// and once via the legacy structured-clone JS-object path. The
// `legacy_*` variants call the underlying WASM `execute()` (not
// `executeBuffer()`) so we can read the gain in isolation.

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
  const db = await createDatabase({ runtime: "main-thread" });
  await setup(db);
  for (let i = 0; i < WARMUP; i++) await run(db);
  const samples = [];
  let rowsObserved = 0;
  for (let i = 0; i < ITERS; i++) {
    const { ns, out } = await timed(() => run(db));
    samples.push(ns);
    rowsObserved = (out.rows ?? out).length;
  }
  samples.sort((a, b) => a - b);
  if (expectedRows >= 0 && rowsObserved !== expectedRows) {
    throw new Error(`[${name}] expected ${expectedRows} rows, got ${rowsObserved}`);
  }
  if (expectedRows < 0) expectedRows = rowsObserved;
  const median = pct(samples, 0.5);
  const p99 = pct(samples, 0.99);
  await db.dispose();
  return {
    name,
    rows: expectedRows,
    iters: ITERS,
    medianNs: median,
    p99Ns: p99,
    nsPerRow: median / Math.max(1, expectedRows),
    rowsPerSec: expectedRows / (median / 1e9),
  };
}

async function seedNodes(db, n) {
  for (let i = 0; i < n; i++) await db.execute(`CREATE (:Node {id: ${i}, value: ${i % 100}})`);
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

const results = [];

results.push(
  await bench(
    "point_read_10",
    (db) => seedNodes(db, 10_000),
    (db) => db.execute("MATCH (n:Node) WHERE n.id < 10 RETURN n.id, n.value"),
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
  const cols = 50, rowCount = 1_000;
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

console.log(
  ["workload", "rows", "median µs", "p99 µs", "ns/row", "rows/s"]
    .map((s) => s.padEnd(20))
    .join(""),
);
const fmt = (n) =>
  n >= 1_000_000 ? `${(n / 1_000_000).toFixed(2)} M` :
  n >= 1_000     ? `${(n / 1_000).toFixed(1)} K` :
  n.toFixed(1);
for (const r of results) {
  console.log(
    [
      r.name.padEnd(20),
      String(r.rows).padEnd(20),
      (r.medianNs / 1000).toFixed(1).padEnd(20),
      (r.p99Ns / 1000).toFixed(1).padEnd(20),
      r.nsPerRow.toFixed(0).padEnd(20),
      fmt(r.rowsPerSec).padEnd(20),
    ].join(""),
  );
}

writeFileSync(
  resolve(HERE, "last.json"),
  JSON.stringify({ when: new Date().toISOString(), results }, null, 2),
);
