# Load harness

How many concurrent requests a lora-graphql server takes, and how latency, CPU and the event loop hold up as load grows. `yarn bench` measures one request at a time; this measures a server under load.

```sh
yarn bench:load                                  # default scenarios, 1 to 512 in flight
yarn bench:load --scenario mixed --rate 100,200,300       # open loop, fixed arrival rates
yarn bench:load --scenario page,connection,aggregate --scale 5
yarn bench:load --scenario byKey --claims unique # a distinct JWT per request
yarn bench:load --scenario byKey,traverse --workers 4 --uv-threads 8   # 4 GraphQL threads, one engine
yarn bench:load --help
```

## How it works

- `run.ts` forks a fresh `server.ts` per scenario, each with its own seeded bench graph (`bench/seed.ts`, scaled by `--scale`). The server is a minimal `node:http` GraphQL endpoint with a parsed-document cache, so the numbers are the library's and the binding's, not a framework's.
- Each step runs `--threads` client threads (`loadgen.ts`) for a warmup, then a measured window. The runner merges their raw latencies, so percentiles are exact.
- **Closed loop** (`--concurrency`): N requests always in flight. This answers "how much throughput at N concurrent clients".
- **Open loop** (`--rate`): requests leave on a schedule, and latency counts from when each was due. A stalled server therefore shows its queue instead of hiding it. Use it for latency at a given traffic level.
- After every step, the server reports over IPC:
  - CPU in cores (1.0 is one saturated JS thread);
  - event-loop delay (p99 and max: how long the main thread was blocked);
  - RSS;
  - peak in-flight requests.

  A server that does not answer is marked `hung`, and its ramp ends. A ramp also ends when errors pass `--max-errors`.
- Results go to `bench/load/results/<time>.json` and `.md` (gitignored). They record the native binary's build time, because `@loradb/lora-node` can be rebuilt between runs.

## Worker threads over one engine (`--workers N`)

One process can run several GraphQL event loops over one database. With `--workers N`:

- The server's main thread creates a persistent database directory, asserts the schema and seeds the graph once.
- N `worker_threads` each open that same directory. The lora-node registry keeps one engine per path per process, so every worker reads and writes the same graph. Each worker checks at startup that it sees the seeded node count, and the run fails otherwise.
- Each worker builds its own `LoraGraphQL` and listens on its own port. macOS has no `reusePort`, so instead of a dispatcher the client spreads its connections evenly across the ports. On Linux a production server could share one port with `reusePort`.
- CPU and RSS are the whole process. Event-loop delay is the worst worker's; in-flight is the sum.
- `--workers 1` measures the worker path itself (a WAL-backed directory, served from a worker thread), so compare 1, 2 and 4 with each other. The default, 0, serves from the main thread.

The workers still share one libuv pool (`UV_THREADPOOL_SIZE`), where reads run, so raise `--uv-threads` with the worker count.

**Caveat for a real server:** `lora.onWrite` (and anything built on it, such as a response cache invalidated by writes) is per `LoraGraphQL` instance. A write served by one worker does not fire another worker's listeners. Invalidate across threads from the engine's committed change feed (`changeFeed: true`, then `lora.changes()`), which carries every commit to the shared engine whatever thread made it (not exercised by this harness), or broadcast writes between threads yourself, for example over a `BroadcastChannel`. In-memory databases are not shared between workers: each `createDatabase()` without a directory is its own graph.

## Reading the numbers

- **`cores` near 1.0 with flat `ok/s`**: the process is bound by its single JS thread. More concurrency only adds latency.
- **`loop max` close to the request latency**: the query ran on the main thread and blocked every other request.
- **`cores` near 0 with timeouts**: the server is stalled, not busy.
- Client and server share the machine, so a step that saturates it measures both. Keep `--threads` below the core count.
