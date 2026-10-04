import { beforeAll, describe, expect, it } from "vitest";
import { createDatabase, LoraError, type Database } from "../ts/index.js";

// Two 20k-node labels: a cartesian product over them is 4*10^8 rows,
// far beyond anything that finishes within a 50 ms timeout.
const N = 20_000;

// If a regression ever lets one of these queries ignore its deadline, it
// would grow native memory without bound and could freeze the machine
// (V8's --max-old-space-size does not cover native allocations). Kill
// the worker instead: the test fails, the machine stays responsive.
const RSS_LIMIT_BYTES = 4 * 1024 ** 3;
const watchdog = setInterval(() => {
  if (process.memoryUsage.rss() > RSS_LIMIT_BYTES) {
    console.error(
      `timeouts.test.ts: RSS above ${RSS_LIMIT_BYTES} bytes; aborting worker`,
    );
    process.kill(process.pid, "SIGKILL");
  }
}, 50);
watchdog.unref();
const CARTESIAN = "MATCH (a:A), (b:B) RETURN count(*) AS c";

async function seed(db: Database): Promise<void> {
  // timeoutMs: 0 opts the bulk load out of any database-wide default.
  await db.execute(
    `UNWIND range(1, ${N}) AS i CREATE (:A {i: i})`,
    {},
    { timeoutMs: 0 },
  );
  await db.execute(
    `UNWIND range(1, ${N}) AS i CREATE (:B {i: i})`,
    {},
    { timeoutMs: 0 },
  );
}

async function elapsed(
  p: Promise<unknown>,
): Promise<{ ms: number; err: unknown }> {
  const t = performance.now();
  const err = await p.then(
    () => undefined,
    (e: unknown) => e,
  );
  return { ms: performance.now() - t, err };
}

describe("query timeouts and cancellation", () => {
  let db: Database;
  beforeAll(async () => {
    db = await createDatabase();
    await seed(db);
  }, 60_000);

  it("rejects an expensive read with LORA_TIMEOUT near timeoutMs", async () => {
    const { ms, err } = await elapsed(
      db.execute(CARTESIAN, {}, { timeoutMs: 50 }),
    );
    expect(err).toBeInstanceOf(LoraError);
    expect((err as LoraError).code).toBe("LORA_TIMEOUT");
    expect(ms).toBeLessThan(1_000);
  });

  it("releases the writer lock when an expensive write times out", async () => {
    // The predicate keeps the output bounded (at most N pairs) while the
    // scan still walks the full 4*10^8-row product, so the write is partway
    // through when the deadline fires. It is an expression rather than
    // `b.i = 1` so the planner cannot turn it into an index lookup.
    const { err } = await elapsed(
      db.execute(
        "MATCH (a:A), (b:B) WHERE b.i + a.i = a.i + 1 CREATE (:Pair)",
        {},
        {
          timeoutMs: 50,
        },
      ),
    );
    expect((err as LoraError).code).toBe("LORA_TIMEOUT");

    // The next write goes straight through, and the timed-out write left
    // nothing behind.
    const { ms, err: writeErr } = await elapsed(
      db.execute("CREATE (:After {ok: true})"),
    );
    expect(writeErr).toBeUndefined();
    expect(ms).toBeLessThan(500);
    const { rows } = await db.execute("MATCH (p:Pair) RETURN count(p) AS c");
    expect(rows[0].c).toBe(0);
  });

  it("cancels a running query when its AbortSignal fires", async () => {
    const controller = new AbortController();
    setTimeout(() => controller.abort(), 50);
    const { ms, err } = await elapsed(
      db.execute(CARTESIAN, {}, { signal: controller.signal }),
    );
    expect((err as Error).name).toBe("AbortError");
    expect(ms).toBeLessThan(1_000);
  });

  it("rejects immediately when the signal is already aborted", async () => {
    const controller = new AbortController();
    controller.abort();
    const { err } = await elapsed(
      db.execute("RETURN 1", {}, { signal: controller.signal }),
    );
    expect((err as Error).name).toBe("AbortError");
  });

  it("does not affect queries that finish in time", async () => {
    const controller = new AbortController();
    const { rows } = await db.execute(
      "RETURN 1 AS x",
      {},
      {
        timeoutMs: 5_000,
        signal: controller.signal,
      },
    );
    expect(rows[0].x).toBe(1);
    controller.abort(); // after completion: harmless
  });

  it("bounds a whole transaction() batch and rolls it back", async () => {
    const { err } = await elapsed(
      db.transaction(
        [
          { query: "CREATE (:TxMarker)" },
          { query: "MATCH (a:A), (b:B) RETURN count(*) AS c" },
        ],
        "read_write",
        { timeoutMs: 50 },
      ),
    );
    expect((err as LoraError).code).toBe("LORA_TIMEOUT");
    const { rows } = await db.execute(
      "MATCH (m:TxMarker) RETURN count(m) AS c",
    );
    expect(rows[0].c).toBe(0);
  });

  it("stops a stream when its signal aborts", async () => {
    const controller = new AbortController();
    const stream = db.stream(
      "MATCH (a:A) RETURN a.i AS i",
      {},
      { signal: controller.signal },
    );
    const first = await stream.next();
    expect(first.done).toBe(false);
    controller.abort();
    await expect(stream.next()).rejects.toMatchObject({ name: "AbortError" });
  });

  it("applies the database-wide queryTimeoutMs default", async () => {
    const bounded = await createDatabase(undefined, { queryTimeoutMs: 50 });
    await seed(bounded);
    const { err } = await elapsed(bounded.execute(CARTESIAN));
    expect((err as LoraError).code).toBe("LORA_TIMEOUT");
    // An explicit timeoutMs of 0 opts a call out of the default.
    const { rows } = await bounded.execute(
      "RETURN 2 AS x",
      {},
      { timeoutMs: 0 },
    );
    expect(rows[0].x).toBe(2);
  }, 60_000);
});

// `@loradb/lora-graphql` compiles relationship filters to nested pattern
// comprehensions. Their loops run inside one expression evaluation, below
// every operator boundary, and once ignored both timeoutMs and the signal.
describe("deadlines inside pattern comprehensions", () => {
  const NESTED =
    "MATCH (f:Festival) WHERE size([(f)<-[:FOLLOWS]-(u:User) " +
    "WHERE size([(u)-[:FOLLOWS]->(g:Festival) " +
    "WHERE size([(g)<-[:FOLLOWS]-(w:User) WHERE w.name CONTAINS 'zz' | 1]) > 0 " +
    "| 1]) > 0 | 1]) > 0 RETURN f.key AS key ORDER BY key";
  let db: Database;
  beforeAll(async () => {
    db = await createDatabase();
    const run = (q: string) => db.execute(q, {}, { timeoutMs: 0 });
    await run("CREATE INDEX FOR (n:Festival) ON (n.i)");
    await run("CREATE INDEX FOR (n:User) ON (n.i)");
    await run(
      "UNWIND range(0, 4999) AS i CREATE (:Festival {i: i, key: 'f' + toString(i)})",
    );
    await run(
      "UNWIND range(0, 1999) AS i CREATE (:User {i: i, name: 'user ' + toString(i)})",
    );
    await run(
      "UNWIND range(0, 22999) AS i " +
        "MATCH (u:User {i: (i * 7919) % 2000}), (f:Festival {i: (i * 104729) % 5000}) " +
        "CREATE (u)-[:FOLLOWS]->(f)",
    );
  }, 60_000);

  it("rejects with LORA_TIMEOUT near timeoutMs", async () => {
    const { ms, err } = await elapsed(
      db.execute(NESTED, {}, { timeoutMs: 100 }),
    );
    expect((err as LoraError).code).toBe("LORA_TIMEOUT");
    expect(ms).toBeLessThan(300);
  });

  it("stops promptly when the signal aborts", async () => {
    const controller = new AbortController();
    setTimeout(() => controller.abort(), 50);
    const { ms, err } = await elapsed(
      db.execute(NESTED, {}, { signal: controller.signal }),
    );
    expect((err as Error).name).toBe("AbortError");
    expect(ms).toBeLessThan(250);
  });
});
