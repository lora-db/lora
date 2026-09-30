// Load harness: how many concurrent requests a lora-graphql server takes,
// and how latency, CPU and the event loop hold up as load grows.
//   yarn bench:load --help
//
// For each scenario it forks a fresh server (server.ts, its own
// seeded graph), ramps the load step by step with client threads
// (loadgen.ts), and reads the server's own stats after every step. A step
// that errors past --max-errors or leaves the server unresponsive ends
// that scenario's ramp. Results go to bench/load/results as JSON and
// Markdown.

import { fork, type ChildProcess } from "node:child_process";
import { mkdir, readdir, stat, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname } from "node:path";
import { availableParallelism, cpus, totalmem } from "node:os";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { Worker } from "node:worker_threads";
import { graphSize } from "../seed.js";
import type { LoadgenInput, LoadgenOutput, Outcome } from "./loadgen.js";
import { operations, scenarios, type Operation } from "./scenarios.js";
import type { ServerConfig, ServerMessage, ServerStats } from "./server.js";

const help = `Usage: yarn bench:load [options]

  --scenario LIST     comma-separated, default: byKey,traverse,page,connection,cypherField,update,mixed
                      available: ${Object.keys(scenarios).join(", ")}
  --concurrency LIST  closed-loop steps, requests in flight (default 1,8,32,128,512)
  --rate LIST         open-loop steps instead, requests per second (e.g. 500,1000,2000)
  --duration SEC      measured seconds per step (default 10)
  --warmup SEC        unmeasured seconds before each step (default 2)
  --scale N           graph size, 1 = 20k festivals, 2k users, 100k follows (default 1)
  --persist           back the database with a WAL directory
  --uv-threads N      server UV_THREADPOOL_SIZE (default: Node's, 4)
  --claims KIND       none | shared | unique: JWT claims per request (default none)
  --threads N         client threads (default 4)
  --request-timeout MS  client-side timeout per request (default 15000)
  --slo MS            p99 target for the "within SLO" summary (default 100)
  --max-errors PCT    stop ramping past this error rate (default 5)
  --out DIR           results directory (default bench/load/results)
`;

const { values: args } = parseArgs({
  args: process.argv.slice(2),
  options: {
    scenario: {
      type: "string",
      default: "byKey,traverse,page,connection,cypherField,update,mixed",
    },
    concurrency: { type: "string", default: "1,8,32,128,512" },
    rate: { type: "string" },
    duration: { type: "string", default: "10" },
    warmup: { type: "string", default: "2" },
    scale: { type: "string", default: "1" },
    persist: { type: "boolean", default: false },
    "uv-threads": { type: "string" },
    claims: { type: "string", default: "none" },
    threads: { type: "string", default: "4" },
    "request-timeout": { type: "string", default: "15000" },
    slo: { type: "string", default: "100" },
    "max-errors": { type: "string", default: "5" },
    out: { type: "string", default: "bench/load/results" },
    help: { type: "boolean", default: false },
  },
});
if (args.help) {
  console.log(help);
  process.exit(0);
}

const list = (s: string) =>
  s
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean);
const mode: "closed" | "open" = args.rate ? "open" : "closed";
const steps = list(args.rate ?? args.concurrency).map(Number);
const scale = Number(args.scale);
const size = graphSize(scale);
const clientThreads = Number(args.threads);
const requestTimeoutMs = Number(args["request-timeout"]);
const slo = Number(args.slo);
const maxErrorRate = Number(args["max-errors"]) / 100;
const scenarioNames = list(args.scenario);
for (const name of scenarioNames) {
  if (!scenarios[name]) throw new Error(`unknown scenario ${name}\n\n${help}`);
}

interface Step {
  scenario: string;
  /** Requests in flight (closed) or offered per second (open). */
  load: number;
  throughput: number;
  p50: number;
  p90: number;
  p99: number;
  p999: number;
  max: number;
  /** Per operation, when the scenario mixes several. */
  operations?: Record<string, { count: number; p50: number; p99: number }>;
  counts: Record<Outcome, number>;
  dropped: number;
  errorRate: number;
  server: ServerStats | null;
  errors: string[];
}

// When bundled by main.mjs, server.js and loadgen.js sit next to this file.
function entryDir(): URL {
  return new URL(".", import.meta.url);
}

class Server {
  readonly #child: ChildProcess;
  #waiting: ((message: ServerMessage) => void) | undefined;

  private constructor(child: ChildProcess) {
    this.#child = child;
    child.on("message", (message: ServerMessage) => this.#waiting?.(message));
  }

  static async start(config: ServerConfig) {
    const env: NodeJS.ProcessEnv = {
      ...process.env,
      NODE_ENV: "production",
      LOAD_SERVER_CONFIG: JSON.stringify(config),
    };
    if (args["uv-threads"]) env.UV_THREADPOOL_SIZE = args["uv-threads"];
    const child = fork(fileURLToPath(new URL("server.js", entryDir())), {
      env,
      stdio: ["ignore", "inherit", "inherit", "ipc"],
    });
    const server = new Server(child);
    const ready = await server.#next(10 * 60_000);
    if (ready?.type !== "ready") throw new Error("server did not start");
    return { server, port: ready.port, seedMs: ready.seedMs };
  }

  /** The server's stats since the last call, or null when it does not answer. */
  async stats(timeoutMs = 10_000): Promise<ServerStats | null> {
    this.#child.send({ type: "stats" });
    const message = await this.#next(timeoutMs);
    return message?.type === "stats" ? message.stats : null;
  }

  /** Ask the server to exit; kill it when it cannot (a stalled pool never joins its threads). */
  async stop(): Promise<void> {
    const exited = new Promise<void>((resolve) => {
      if (this.#child.exitCode !== null) resolve();
      else this.#child.once("exit", () => resolve());
    });
    if (this.#child.connected) this.#child.disconnect();
    const timer = setTimeout(() => this.#child.kill("SIGKILL"), 3000);
    await exited;
    clearTimeout(timer);
  }

  #next(timeoutMs: number): Promise<ServerMessage | undefined> {
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        this.#waiting = undefined;
        resolve(undefined);
      }, timeoutMs);
      this.#waiting = (message) => {
        clearTimeout(timer);
        this.#waiting = undefined;
        resolve(message);
      };
    });
  }
}

/** Split `total` into `parts` near-equal integers, dropping zeros. */
function split(total: number, parts: number): number[] {
  const out = Array.from(
    { length: parts },
    (_, i) => Math.floor(total / parts) + (i < total % parts ? 1 : 0),
  );
  return out.filter((n) => n > 0);
}

async function step(
  scenario: string,
  port: number,
  server: Server,
  load: number,
): Promise<Step> {
  await server.stats(); // reset the server's counters
  const shares =
    mode === "closed"
      ? split(load, Math.min(clientThreads, load))
      : split(load, clientThreads);
  const durationMs = Number(args.duration) * 1000;
  const outputs = await Promise.all(
    shares.map((share, i) => {
      const input: LoadgenInput = {
        origin: `http://127.0.0.1:${port}`,
        scenario: scenarios[scenario]!,
        size,
        mode,
        concurrency: mode === "closed" ? share : 128,
        rate: share,
        warmupMs: Number(args.warmup) * 1000,
        durationMs,
        requestTimeoutMs,
        maxInflight: 2048,
        seed: i + 1,
        claims: args.claims as LoadgenInput["claims"],
      };
      return new Promise<LoadgenOutput>((resolve, reject) => {
        const worker = new Worker(new URL("loadgen.js", entryDir()), {
          workerData: input,
        });
        worker.once("message", resolve);
        worker.once("error", reject);
      });
    }),
  );
  const serverStats = await server.stats();

  const latencies = merge(outputs.map((o) => o.latencies));
  latencies.sort();
  const at = (q: number) => quantile(latencies, q);
  const names = Object.keys(scenarios[scenario]!);
  const perOperation =
    names.length > 1
      ? Object.fromEntries(
          names.map((name) => {
            const all = merge(
              outputs.map(
                (o) => o.byOperation[name as keyof typeof o.byOperation],
              ),
            ).sort();
            return [
              name,
              {
                count: all.length,
                p50: quantile(all, 0.5),
                p99: quantile(all, 0.99),
              },
            ];
          }),
        )
      : undefined;
  const counts: Record<Outcome, number> = {
    ok: 0,
    graphql: 0,
    http: 0,
    timeout: 0,
    network: 0,
  };
  for (const o of outputs) {
    for (const k of Object.keys(counts) as Outcome[]) counts[k] += o.counts[k];
  }
  const dropped = outputs.reduce((n, o) => n + o.dropped, 0);
  const total = Object.values(counts).reduce((a, b) => a + b, 0) + dropped;
  return {
    scenario,
    load,
    throughput: counts.ok / (durationMs / 1000),
    p50: at(0.5),
    p90: at(0.9),
    p99: at(0.99),
    p999: at(0.999),
    max: at(1),
    ...(perOperation && { operations: perOperation }),
    counts,
    dropped,
    errorRate: total === 0 ? 1 : (total - counts.ok) / total,
    server: serverStats,
    errors: [...new Set(outputs.flatMap((o) => o.errors))].slice(0, 5),
  };
}

function merge(parts: Array<Float64Array | undefined>): Float64Array {
  const out = new Float64Array(parts.reduce((n, p) => n + (p?.length ?? 0), 0));
  let offset = 0;
  for (const p of parts) {
    if (!p) continue;
    out.set(p, offset);
    offset += p.length;
  }
  return out;
}

/** The q-quantile of sorted values; NaN when there are none. */
function quantile(sorted: Float64Array, q: number): number {
  if (sorted.length === 0) return NaN;
  return sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))]!;
}

const ms = (n: number) =>
  Number.isNaN(n) ? "-" : n < 10 ? n.toFixed(2) : n.toFixed(0);
const columns = [
  [mode === "closed" ? "in flight" : "offered/s", 10],
  ["ok/s", 8],
  ["p50", 7],
  ["p90", 7],
  ["p99", 7],
  ["p99.9", 7],
  ["max", 7],
  ["errors", 8],
  ["cores", 6],
  ["loop p99", 9],
  ["loop max", 9],
  ["rss MB", 7],
] as const;
function cells(s: Step): string[] {
  return [
    String(s.load),
    s.throughput.toFixed(0),
    ms(s.p50),
    ms(s.p90),
    ms(s.p99),
    ms(s.p999),
    ms(s.max),
    `${(s.errorRate * 100).toFixed(1)}%`,
    s.server ? s.server.cpuCores.toFixed(2) : "hung",
    s.server ? ms(s.server.loopDelayP99Ms) : "-",
    s.server ? ms(s.server.loopDelayMaxMs) : "-",
    s.server ? s.server.rssMB.toFixed(0) : "-",
  ];
}
const printRow = (row: string[]) =>
  console.log(row.map((c, i) => c.padStart(columns[i]![1])).join(" "));

const results: Step[] = [];
const started = new Date();
console.log(
  `lora-graphql load: ${mode}-loop, scale ${scale} (${size.festivals} festivals, ${size.users} users), ` +
    `${args.duration}s steps, latencies in ms, ${cpus()[0]?.model ?? "?"} × ${availableParallelism()}`,
);
for (const scenario of scenarioNames) {
  const { server, port, seedMs } = await Server.start({
    scale,
    persist: args.persist,
    timeoutMs: 10_000,
  });
  const ops = Object.keys(scenarios[scenario]!)
    .map((op) =>
      (operations[op as keyof typeof operations] as Operation).write
        ? `${op}*`
        : op,
    )
    .join(" + ");
  console.log(
    `\n${scenario}: ${ops} (seeded in ${(seedMs / 1000).toFixed(1)}s)`,
  );
  printRow(columns.map(([name]) => name));
  for (const load of steps) {
    const s = await step(scenario, port, server, load);
    results.push(s);
    printRow(cells(s));
    if (s.operations) {
      console.log(
        "    " +
          Object.entries(s.operations)
            .map(([op, o]) => `${op} p50 ${ms(o.p50)} p99 ${ms(o.p99)}`)
            .join(", "),
      );
    }
    for (const e of s.errors) console.log(`    ${e}`);
    if (!s.server) {
      console.log("    server unresponsive: ending this ramp");
      break;
    }
    if (s.errorRate > maxErrorRate) {
      console.log("    error rate past --max-errors: ending this ramp");
      break;
    }
  }
  await server.stop();
}

// Summary: peak throughput, and the most load served within the SLO.
const summary = scenarioNames.flatMap((scenario) => {
  const rows = results.filter((r) => r.scenario === scenario);
  if (rows.length === 0) return [];
  const peak = rows.reduce((a, b) => (b.throughput > a.throughput ? b : a));
  const healthy = rows.filter((r) => r.p99 <= slo && r.errorRate <= 0.001);
  const withinSlo = healthy.length
    ? healthy.reduce((a, b) => (b.throughput > a.throughput ? b : a))
    : undefined;
  return [{ scenario, peak, withinSlo }];
});

const stamp = started.toISOString().replace(/[:.]/g, "-");
await mkdir(args.out, { recursive: true });
const base = `${args.out}/${stamp}`;
// The native binary can be rebuilt between runs: record which one ran.
const bindingDir = dirname(
  dirname(createRequire(import.meta.url).resolve("@loradb/lora-node")),
);
const binary = (await readdir(bindingDir)).find((f) => f.endsWith(".node"));
const binaryBuilt = binary
  ? (await stat(`${bindingDir}/${binary}`)).mtime.toISOString()
  : "unknown";
const meta = {
  started: started.toISOString(),
  mode,
  steps,
  scale,
  size,
  durationSec: Number(args.duration),
  warmupSec: Number(args.warmup),
  persist: args.persist,
  uvThreadpool: args["uv-threads"] ?? "default",
  clientThreads,
  claims: args.claims,
  sloP99Ms: slo,
  node: process.version,
  cpu: cpus()[0]?.model,
  cores: availableParallelism(),
  memoryGB: Math.round(totalmem() / 2 ** 30),
  loraNodeBinary: binary ? `${binary}, built ${binaryBuilt}` : "not found",
};
await writeFile(`${base}.json`, JSON.stringify({ meta, results }, null, 2));

const md = [
  `# lora-graphql load run, ${meta.started}`,
  "",
  `${mode}-loop, scale ${scale}, ${meta.durationSec}s steps after ${meta.warmupSec}s warmup, ` +
    `${meta.cpu} × ${meta.cores}, ${meta.memoryGB} GB, Node ${meta.node}, ` +
    `UV_THREADPOOL_SIZE ${meta.uvThreadpool}, claims ${meta.claims}${args.persist ? ", WAL-backed" : ", in memory"}, ` +
    `${meta.loraNodeBinary}. Latencies in ms.`,
  "",
  "## Summary",
  "",
  `| scenario | peak ok/s | at load | p99 there | best ok/s with p99 ≤ ${slo} ms |`,
  "|---|--:|--:|--:|--:|",
  ...summary.map(
    (s) =>
      `| ${s.scenario} | ${s.peak.throughput.toFixed(0)} | ${s.peak.load} | ${ms(s.peak.p99)} | ` +
      (s.withinSlo
        ? `${s.withinSlo.throughput.toFixed(0)} (at ${s.withinSlo.load})`
        : "none") +
      " |",
  ),
  "",
  "## Steps",
  "",
  `| scenario | ${columns.map(([name]) => name).join(" | ")} |`,
  `|---|${columns.map(() => "--:").join("|")}|`,
  ...results.map((r) => `| ${r.scenario} | ${cells(r).join(" | ")} |`),
  "",
];
await writeFile(`${base}.md`, md.join("\n"));
console.log(`\nresults: ${base}.json, ${base}.md`);
process.exit(0);
