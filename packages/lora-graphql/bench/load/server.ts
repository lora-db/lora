// The server under test: the bench graph behind a minimal node:http GraphQL
// endpoint, so the numbers are the library's and the binding's, not a
// framework's. Parsed and validated documents are cached, as graphql-http
// and Yoga do. Spawned by run.ts, one process per scenario, and driven over
// IPC: it reports "ready", then answers "stats" with what the last step
// cost (event-loop delay, CPU, memory, in-flight requests). An
// x-bench-claims header stands in for a verified token: its JSON becomes
// the context's jwt.
//
// With `workers` set, the main thread seeds a persistent database
// directory and N worker_threads each open that same directory (the
// lora-node registry shares one engine per path per process), build their
// own LoraGraphQL and listen on their own port. The client spreads its
// connections across the ports. Event-loop numbers are then the worst
// worker's, in-flight the sum.

import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { monitorEventLoopDelay } from "node:perf_hooks";
import {
  isMainThread,
  parentPort,
  Worker,
  workerData,
} from "node:worker_threads";
import { createDatabase } from "@loradb/lora-node";
import { execute, parse, validate, type DocumentNode } from "graphql";
import { LoraGraphQL, loraDriver } from "../../src/index.js";
import { appTypeDefs } from "../../test/fixtures.js";
import { graphSize, seedGraph } from "../seed.js";

export interface ServerConfig {
  scale: number;
  /** Back the database with a WAL directory instead of memory only. */
  persist: boolean;
  timeoutMs: number;
  /** Serve from this many worker threads over one persistent directory. */
  workers?: number;
}

export interface ServerStats {
  wallMs: number;
  /** CPU time over wall time: 1 means one core busy. */
  cpuCores: number;
  loopDelayP50Ms: number;
  loopDelayP99Ms: number;
  loopDelayMaxMs: number;
  rssMB: number;
  heapMB: number;
  maxInflight: number;
}

export type ServerMessage =
  | { type: "ready"; ports: number[]; seedMs: number }
  | { type: "stats"; stats: ServerStats };

/** What one serving thread reports for a step. */
interface ThreadStats {
  loopDelayP50Ms: number;
  loopDelayP99Ms: number;
  loopDelayMaxMs: number;
  maxInflight: number;
}

/** A serving worker thread's input: the directory the main thread seeded. */
interface WorkerInput {
  config: ServerConfig;
  dir: string;
}

type WorkerMessage =
  | { type: "ready"; port: number; festivals: number }
  | { type: "stats"; stats: ThreadStats };

/**
 * One GraphQL endpoint on the calling thread's event loop. `stats` reads
 * and resets what the thread did since the last call.
 */
async function serve(
  lora: LoraGraphQL,
): Promise<{ port: number; stats: () => ThreadStats }> {
  const schema = lora.getSchema();
  const documents = new Map<string, DocumentNode>();
  const loopDelay = monitorEventLoopDelay({ resolution: 1 });
  loopDelay.enable();
  let inflight = 0;
  let maxInflight = 0;

  async function answer(
    raw: string,
    jwt: Record<string, unknown> | undefined,
  ): Promise<{ status: number; body: string }> {
    const { query, variables } = JSON.parse(raw) as {
      query: string;
      variables?: Record<string, unknown>;
    };
    let document = documents.get(query);
    if (!document) {
      document = parse(query);
      const errors = validate(schema, document);
      if (errors.length > 0) {
        return { status: 400, body: JSON.stringify({ errors }) };
      }
      documents.set(query, document);
    }
    const result = await execute({
      schema,
      document,
      variableValues: variables,
      contextValue: jwt ? { jwt } : {},
    });
    return { status: 200, body: JSON.stringify(result) };
  }

  const server = createServer((req, res) => {
    inflight++;
    maxInflight = Math.max(maxInflight, inflight);
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => {
      const claims = req.headers["x-bench-claims"];
      void answer(
        Buffer.concat(chunks).toString(),
        typeof claims === "string" ? JSON.parse(claims) : undefined,
      )
        .catch((error: unknown) => ({
          status: 500,
          body: JSON.stringify({ errors: [{ message: String(error) }] }),
        }))
        .then(({ status, body }) => {
          inflight--;
          res.writeHead(status, { "content-type": "application/json" });
          res.end(body);
        });
    });
  });
  const port = await new Promise<number>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (typeof address !== "object" || !address) {
        reject(new Error("no address"));
      } else resolve(address.port);
    });
  });
  const stats = (): ThreadStats => {
    const result: ThreadStats = {
      loopDelayP50Ms: loopDelay.percentile(50) / 1e6,
      loopDelayP99Ms: loopDelay.percentile(99) / 1e6,
      loopDelayMaxMs: loopDelay.max / 1e6,
      maxInflight,
    };
    loopDelay.reset();
    maxInflight = inflight;
    return result;
  };
  return { port, stats };
}

async function openLora(config: ServerConfig, dir: string | undefined) {
  const db = dir
    ? await createDatabase("load", { databaseDir: dir })
    : await createDatabase();
  const lora = new LoraGraphQL({
    typeDefs: appTypeDefs,
    driver: loraDriver(db),
    timeoutMs: config.timeoutMs,
  });
  return { db, lora };
}

if (isMainThread) {
  const config = JSON.parse(process.env.LOAD_SERVER_CONFIG!) as ServerConfig;
  const send = (message: ServerMessage) => process.send!(message);
  const workers = config.workers ?? 0;

  const dir =
    config.persist || workers > 0
      ? await mkdtemp(join(tmpdir(), "lora-graphql-load-"))
      : undefined;
  const { db, lora } = await openLora(config, dir);
  await lora.assertSchema({ create: true });
  const seeded = performance.now();
  await seedGraph(db, graphSize(config.scale));
  const seedMs = performance.now() - seeded;

  let ports: number[];
  let threadStats: () => Promise<ThreadStats[]>;
  const threads: Worker[] = [];
  if (workers > 0) {
    // Each worker opens the seeded directory: one engine, N event loops.
    const waiting = new Map<Worker, (message: WorkerMessage) => void>();
    const next = (worker: Worker) =>
      new Promise<WorkerMessage>((resolve) => waiting.set(worker, resolve));
    for (let i = 0; i < workers; i++) {
      const worker = new Worker(new URL(import.meta.url), {
        workerData: { config, dir: dir! } satisfies WorkerInput,
      });
      worker.on("message", (message: WorkerMessage) => {
        const resolve = waiting.get(worker);
        waiting.delete(worker);
        resolve?.(message);
      });
      worker.on("error", (error) => {
        console.error(error);
        process.exit(1);
      });
      threads.push(worker);
    }
    ports = await Promise.all(
      threads.map(async (worker) => {
        const ready = await next(worker);
        if (ready.type !== "ready") throw new Error("worker did not start");
        // The worker must see the main thread's graph: one shared engine.
        if (ready.festivals !== graphSize(config.scale).festivals) {
          throw new Error(
            `a worker sees ${ready.festivals} festivals: not the seeded engine`,
          );
        }
        return ready.port;
      }),
    );
    threadStats = () =>
      Promise.all(
        threads.map(async (worker) => {
          const reply = next(worker);
          worker.postMessage({ type: "stats" });
          const message = await reply;
          if (message.type !== "stats") throw new Error("expected stats");
          return message.stats;
        }),
      );
  } else {
    const endpoint = await serve(lora);
    ports = [endpoint.port];
    threadStats = async () => [endpoint.stats()];
  }

  let since = { wall: performance.now(), cpu: process.cpuUsage() };
  const stats = async (): Promise<ServerStats> => {
    const perThread = await threadStats();
    const now = { wall: performance.now(), cpu: process.cpuUsage() };
    const wallMs = now.wall - since.wall;
    const cpuMs =
      (now.cpu.user - since.cpu.user + now.cpu.system - since.cpu.system) /
      1000;
    const memory = process.memoryUsage();
    since = now;
    const worst = (key: keyof ThreadStats) =>
      Math.max(...perThread.map((t) => t[key]));
    return {
      wallMs,
      cpuCores: cpuMs / wallMs,
      loopDelayP50Ms: worst("loopDelayP50Ms"),
      loopDelayP99Ms: worst("loopDelayP99Ms"),
      loopDelayMaxMs: worst("loopDelayMaxMs"),
      rssMB: memory.rss / 2 ** 20,
      heapMB: memory.heapUsed / 2 ** 20,
      maxInflight: perThread.reduce((n, t) => n + t.maxInflight, 0),
    };
  };

  process.on("message", (message: { type: string }) => {
    if (message.type === "stats") {
      void stats().then((s) => send({ type: "stats", stats: s }));
    }
  });
  process.on("disconnect", () => {
    void Promise.all(threads.map((t) => t.terminate()))
      .then(() => (dir ? rm(dir, { recursive: true, force: true }) : undefined))
      .finally(() => process.exit(0));
  });

  await stats();
  send({ type: "ready", ports, seedMs });
} else {
  const { config, dir } = workerData as WorkerInput;
  const { db, lora } = await openLora(config, dir);
  const counted = (await db.execute(
    "MATCH (f:Festival) RETURN count(f) AS n",
  )) as { rows: Array<{ n: number }> };
  const endpoint = await serve(lora);
  parentPort!.on("message", (message: { type: string }) => {
    if (message.type === "stats") {
      parentPort!.postMessage({
        type: "stats",
        stats: endpoint.stats(),
      } satisfies WorkerMessage);
    }
  });
  parentPort!.postMessage({
    type: "ready",
    port: endpoint.port,
    festivals: Number(counted.rows[0]?.n),
  } satisfies WorkerMessage);
}
