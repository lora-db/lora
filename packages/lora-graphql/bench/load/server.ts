// The server under test: the bench graph behind a minimal node:http GraphQL
// endpoint, so the numbers are the library's and the binding's, not a
// framework's. Parsed and validated documents are cached, as graphql-http
// and Yoga do. Spawned by run.ts, one process per scenario, and driven over
// IPC: it reports "ready", then answers "stats" with what the last step
// cost (event-loop delay, CPU, memory, in-flight requests). An
// x-bench-claims header stands in for a verified token: its JSON becomes
// the context's jwt.

import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { monitorEventLoopDelay } from "node:perf_hooks";
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
  | { type: "ready"; port: number; seedMs: number }
  | { type: "stats"; stats: ServerStats };

const config = JSON.parse(process.env.LOAD_SERVER_CONFIG!) as ServerConfig;
const send = (message: ServerMessage) => process.send!(message);

const dir = config.persist
  ? await mkdtemp(join(tmpdir(), "lora-graphql-load-"))
  : undefined;
const db = dir
  ? await createDatabase("load", { databaseDir: dir })
  : await createDatabase();
const lora = new LoraGraphQL({
  typeDefs: appTypeDefs,
  driver: loraDriver(db),
  timeoutMs: config.timeoutMs,
});
await lora.assertSchema({ create: true });
const seeded = performance.now();
await seedGraph(db, graphSize(config.scale));
const seedMs = performance.now() - seeded;

const schema = lora.getSchema();
const documents = new Map<string, DocumentNode>();
const loopDelay = monitorEventLoopDelay({ resolution: 1 });
loopDelay.enable();
let inflight = 0;
let maxInflight = 0;
let since = { wall: performance.now(), cpu: process.cpuUsage() };

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

function stats(): ServerStats {
  const now = { wall: performance.now(), cpu: process.cpuUsage() };
  const wallMs = now.wall - since.wall;
  const cpuMs =
    (now.cpu.user - since.cpu.user + now.cpu.system - since.cpu.system) / 1000;
  const memory = process.memoryUsage();
  const result: ServerStats = {
    wallMs,
    cpuCores: cpuMs / wallMs,
    loopDelayP50Ms: loopDelay.percentile(50) / 1e6,
    loopDelayP99Ms: loopDelay.percentile(99) / 1e6,
    loopDelayMaxMs: loopDelay.max / 1e6,
    rssMB: memory.rss / 2 ** 20,
    heapMB: memory.heapUsed / 2 ** 20,
    maxInflight,
  };
  since = now;
  loopDelay.reset();
  maxInflight = inflight;
  return result;
}

process.on("message", (message: { type: string }) => {
  if (message.type === "stats") send({ type: "stats", stats: stats() });
});
process.on("disconnect", () => {
  server.close();
  void (
    dir ? rm(dir, { recursive: true, force: true }) : Promise.resolve()
  ).finally(() => process.exit(0));
});

server.listen(0, "127.0.0.1", () => {
  const address = server.address();
  if (typeof address !== "object" || !address) throw new Error("no address");
  stats();
  send({ type: "ready", port: address.port, seedMs });
});
