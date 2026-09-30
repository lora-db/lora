// One load-generator thread. run.ts starts several so the client is not
// the bottleneck, and merges their raw latencies.
//
// closed: `concurrency` workers, each sending its next request when the
//   last one answers. Measures throughput at a fixed number in flight.
// open: requests leave on a fixed schedule (`rate` per second) whether or
//   not earlier ones answered, and latency counts from the scheduled send
//   time, so a stalled server cannot hide its queue (coordinated omission).
//   Requests past `maxInflight` are dropped and counted, not queued.

import { parentPort, workerData } from "node:worker_threads";
import { Pool } from "undici";
import type { GraphSize } from "../seed.js";
import {
  operations,
  picker,
  rng,
  type OperationName,
  type Scenario,
} from "./scenarios.js";

export interface LoadgenInput {
  origin: string;
  scenario: Scenario;
  size: GraphSize;
  mode: "closed" | "open";
  /** closed: workers in this thread. open: connections in this thread. */
  concurrency: number;
  /** open: requests per second from this thread. */
  rate: number;
  warmupMs: number;
  durationMs: number;
  requestTimeoutMs: number;
  maxInflight: number;
  seed: number;
  /** none, one token for everyone, or a distinct token per request. */
  claims: "none" | "shared" | "unique";
}

export type Outcome = "ok" | "graphql" | "http" | "timeout" | "network";

export interface LoadgenOutput {
  /** Milliseconds, measured requests that answered (ok or graphql error). */
  latencies: Float64Array;
  /** The same latencies, per operation of the scenario. */
  byOperation: Partial<Record<OperationName, Float64Array>>;
  counts: Record<Outcome, number>;
  dropped: number;
  errors: string[];
}

const input = workerData as LoadgenInput;
const random = rng(input.seed);
const pick = (n: number) => Math.floor(random() * n);
const next = picker(input.scenario, random);
const pool = new Pool(input.origin, {
  connections: input.concurrency,
  pipelining: 1,
  headersTimeout: input.requestTimeoutMs,
  bodyTimeout: input.requestTimeoutMs,
});

const latencies: number[] = [];
const byOperation: Partial<Record<OperationName, number[]>> = {};
const counts: Record<Outcome, number> = {
  ok: 0,
  graphql: 0,
  http: 0,
  timeout: 0,
  network: 0,
};
const errors = new Set<string>();
let dropped = 0;
let seq = input.seed * 1_000_000;

const start = performance.now();
const measureFrom = start + input.warmupMs;
const end = measureFrom + input.durationMs;

/** A token's claims: unique ones vary per request, as sub, iat and jti do. */
function claims(): string {
  if (input.claims === "shared") {
    return JSON.stringify({ sub: "u1", roles: ["user"] });
  }
  const n = pick(1_000_000_000);
  return JSON.stringify({ sub: `u${n % 100_000}`, iat: n, roles: ["user"] });
}

/** Send one request; record it when it was due at or after measureFrom. */
async function request(due: number): Promise<void> {
  const name = next();
  const op = operations[name];
  const headers: Record<string, string> = {
    "content-type": "application/json",
  };
  if (input.claims !== "none") headers["x-bench-claims"] = claims();
  const body = JSON.stringify({
    query: op.query,
    variables: op.variables(pick, input.size, seq++),
  });
  let outcome: Outcome;
  let error = "";
  try {
    const res = await pool.request({
      path: "/graphql",
      method: "POST",
      headers,
      body,
    });
    const text = await res.body.text();
    if (res.statusCode !== 200) {
      outcome = "http";
      error = `HTTP ${res.statusCode} ${text.slice(0, 200)}`;
    } else if (text.includes('"errors":[')) {
      outcome = "graphql";
      error = text.slice(0, 200);
    } else {
      outcome = "ok";
    }
  } catch (err) {
    const code = (err as { code?: string }).code ?? "";
    outcome = /TIMEOUT/.test(code) ? "timeout" : "network";
    error = code || String(err);
  }
  if (due < measureFrom) return;
  counts[outcome]++;
  if (error) errors.add(`${name}: ${error}`);
  if (outcome === "ok" || outcome === "graphql") {
    latencies.push(performance.now() - due);
    byOperation[name] ??= [];
    byOperation[name].push(performance.now() - due);
  }
}

async function closed(): Promise<void> {
  const worker = async () => {
    while (performance.now() < end) await request(performance.now());
  };
  await Promise.all(Array.from({ length: input.concurrency }, worker));
}

async function open(): Promise<void> {
  const interval = 1000 / input.rate;
  const pending = new Set<Promise<void>>();
  let sent = 0;
  while (performance.now() < end) {
    const now = performance.now();
    while (start + sent * interval <= now) {
      const due = start + sent * interval;
      sent++;
      if (pending.size >= input.maxInflight) {
        if (due >= measureFrom) dropped++;
        continue;
      }
      const p = request(due).finally(() => pending.delete(p));
      pending.add(p);
    }
    await new Promise((resolve) => setTimeout(resolve, 1));
  }
  await Promise.all(pending);
}

await (input.mode === "closed" ? closed() : open());
await pool.close();

const output: LoadgenOutput = {
  latencies: Float64Array.from(latencies),
  byOperation: Object.fromEntries(
    Object.entries(byOperation).map(([k, v]) => [k, Float64Array.from(v)]),
  ),
  counts,
  dropped,
  errors: [...errors].slice(0, 5),
};
parentPort!.postMessage(
  output,
  [output.latencies, ...Object.values(output.byOperation)].map(
    (a) => a.buffer as ArrayBuffer,
  ),
);
