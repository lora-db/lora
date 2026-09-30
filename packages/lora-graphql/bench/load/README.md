# Load harness

How many concurrent requests a lora-graphql server takes, and how latency, CPU and the event loop hold up as load grows. `yarn bench` measures one request at a time; this measures a server under load.

```sh
yarn bench:load                                  # default scenarios, 1 to 512 in flight
yarn bench:load --scenario mixed --rate 100,200,300       # open loop, fixed arrival rates
yarn bench:load --scenario page,connection,aggregate --scale 5
yarn bench:load --scenario byKey --claims unique # a distinct JWT per request
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

## Reading the numbers

- **`cores` near 1.0 with flat `ok/s`**: the process is bound by its single JS thread. More concurrency only adds latency.
- **`loop max` close to the request latency**: the query ran on the main thread and blocked every other request.
- **`cores` near 0 with timeouts**: the server is stalled, not busy.
- Client and server share the machine, so a step that saturates it measures both. Keep `--threads` below the core count.
