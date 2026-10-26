/**
 * Failure modes that used to hang, freeze or kill the host process: a
 * synchronous call waiting for a lock only the JS thread could release,
 * handles dropped without being closed, libuv workers parked on the writer
 * lock, and panics unwinding into N-API. Cases that could hang run in a
 * child process, which is killed on a hang instead of this runner.
 */
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";
// eslint-disable-next-line @typescript-eslint/ban-ts-comment
// @ts-ignore - resolved at runtime via the native.js loader
import native from "../ts/native.js";

const entry = fileURLToPath(new URL("../dist/index.js", import.meta.url));

function child(
  script: string,
  { threads = 4, args = [] as string[], ms = 15_000 } = {},
) {
  return new Promise<{ finished: boolean; out: string; code: number | null }>(
    (resolve) => {
      const p = spawn(
        process.execPath,
        [
          ...args,
          "--input-type=module",
          "-e",
          `const { createDatabase } = await import(${JSON.stringify(entry)});\n` +
            `const sleep = (ms) => new Promise((r) => setTimeout(r, ms));\n` +
            `const collect = async () => { for (let i = 0; i < 10; i++) { globalThis.gc(); await sleep(20); } };\n` +
            script,
        ],
        {
          stdio: ["ignore", "pipe", "pipe"],
          env: { ...process.env, UV_THREADPOOL_SIZE: String(threads) },
        },
      );
      let out = "";
      p.stdout.on("data", (b) => (out += b));
      p.stderr.on("data", (b) => (out += b));
      const timer = setTimeout(() => {
        p.kill("SIGKILL");
        resolve({ finished: false, out, code: null });
      }, ms);
      p.on("exit", (code) => {
        clearTimeout(timer);
        resolve({ finished: true, out, code });
      });
    },
  );
}

const GC = { args: ["--expose-gc"] };

describe("loadSnapshot while the writer lock is held for the JS thread", () => {
  // `loadSnapshot` waited for the writer lock on the JS thread. The holder,
  // an open transaction or write stream, needs that thread to let go, so
  // the process froze for good.
  it("waits for an open transaction without freezing the event loop", async () => {
    const r = await child(
      `const db = await createDatabase();
       await db.execute("CREATE (:A)");
       const snap = await db.saveSnapshot();
       await db.execute("CREATE (:B)");
       const tx = await db.begin();
       let tick = false;
       setTimeout(() => (tick = true), 10);
       const loading = db.loadSnapshot(snap);
       await sleep(100);
       console.log("tick=" + tick);
       await tx.commit();
       await loading;
       const { rows } = await db.execute("MATCH (n) RETURN count(n) AS c");
       console.log("nodes=" + rows[0].c);`,
    );
    expect(r.finished, r.out).toBe(true);
    expect(r.out).toContain("tick=true");
    expect(r.out).toContain("nodes=1");
  });

  it("waits for an open write stream without freezing the event loop", async () => {
    const r = await child(
      `const db = await createDatabase();
       await db.execute("CREATE (:A)");
       const snap = await db.saveSnapshot();
       const stream = db.stream("CREATE (n:S) RETURN n");
       let tick = false;
       setTimeout(() => (tick = true), 10);
       const loading = db.loadSnapshot(snap);
       await sleep(100);
       console.log("tick=" + tick);
       stream.close();
       await loading;
       const { rows } = await db.execute("MATCH (n) RETURN count(n) AS c");
       console.log("nodes=" + rows[0].c);`,
    );
    expect(r.finished, r.out).toBe(true);
    expect(r.out).toContain("tick=true");
    expect(r.out).toContain("nodes=1");
  });
});

describe("handles dropped without being closed", () => {
  it("rolls back a dropped write stream and frees the writer lock", async () => {
    const r = await child(
      `const db = await createDatabase();
       (() => { db.stream("CREATE (n:S) RETURN n"); })();
       await collect();
       try {
         await db.execute("CREATE (:C)", {}, { timeoutMs: 3000 });
         console.log("write=ok");
       } catch (e) { console.log("write=" + e.code); }
       const { rows } = await db.execute("MATCH (n) RETURN count(n) AS c");
       console.log("nodes=" + rows[0].c);`,
      GC,
    );
    expect(r.finished, r.out).toBe(true);
    expect(r.out).toContain("write=ok");
    expect(r.out).toContain("nodes=1");
  });

  it("rolls back a dropped transaction and frees the writer lock", async () => {
    const r = await child(
      `const db = await createDatabase();
       await (async () => {
         const tx = await db.begin();
         await tx.execute("CREATE (:T)");
       })();
       await collect();
       try {
         await db.execute("CREATE (:C)", {}, { timeoutMs: 3000 });
         console.log("write=ok");
       } catch (e) { console.log("write=" + e.code); }
       const { rows } = await db.execute("MATCH (n) RETURN count(n) AS c");
       console.log("nodes=" + rows[0].c);`,
      GC,
    );
    expect(r.finished, r.out).toBe(true);
    expect(r.out).toContain("write=ok");
    expect(r.out).toContain("nodes=1");
  });

  it("lets a database with an open change feed be collected", async () => {
    // The native waker held the feed's callback strongly, and through it
    // the feed and the database handle: neither could ever be collected.
    const r = await child(
      `const gone = new FinalizationRegistry((what) => console.log("collected=" + what));
       await (async () => {
         const db = await createDatabase();
         const feed = db.changes();
         await feed.ready;
         gone.register(db, "db");
       })();
       await collect();`,
      GC,
    );
    expect(r.finished, r.out).toBe(true);
    expect(r.out).toContain("collected=db");
  });

  it("keeps a consumer waiting for changes alive across collections", async () => {
    const r = await child(
      `const db = await createDatabase();
       let got = 0;
       (async () => {
         for await (const batch of db.changes()) { got += batch.changes.length; break; }
       })();
       await sleep(20);
       await collect();
       await db.execute("CREATE (:X)");
       await sleep(100);
       console.log("got=" + got);`,
      GC,
    );
    expect(r.finished, r.out).toBe(true);
    expect(r.out).toContain("got=1");
  });
});

describe("writes waiting for a JS-held writer lock stay off the libuv pool", () => {
  it("leaves the pool to fs and crypto while writes queue behind a transaction", async () => {
    // Each write used to wait for the lock on a libuv worker: two of them
    // took this 2-thread pool, and nothing else in the process (fs, dns,
    // zlib, crypto) could run until the transaction ended.
    const r = await child(
      `const { pbkdf2 } = await import("node:crypto");
       const db = await createDatabase();
       const tx = await db.begin();
       const writes = Array.from({ length: 6 }, (_, i) =>
         db.execute("CREATE (:W {i: $i})", { i }),
       );
       const cleared = db.clear().then(() => db.execute("CREATE (:W {i: 6})"));
       await sleep(20);
       const t = Date.now();
       await new Promise((res, rej) => pbkdf2("x", "y", 1, 32, "sha256", (e) => (e ? rej(e) : res())));
       console.log("crypto_ms=" + (Date.now() - t));
       await tx.commit();
       await Promise.all([...writes, cleared]);
       const { rows } = await db.execute("MATCH (w:W) RETURN count(w) AS c");
       console.log("writes=" + rows[0].c);`,
      { threads: 2 },
    );
    expect(r.finished, r.out).toBe(true);
    expect(Number(/crypto_ms=(\d+)/.exec(r.out)?.[1])).toBeLessThan(1_000);
    expect(r.out).toMatch(/writes=\d+/);
  });

  it("exits on process.exit() with a transaction open and a write waiting", async () => {
    const r = await child(
      `const db = await createDatabase();
       await db.begin();
       db.execute("CREATE (:C)").catch(() => {});
       await sleep(50);
       process.exit(0);`,
      { threads: 2, ms: 10_000 },
    );
    expect(r.finished, r.out).toBe(true);
    expect(r.code).toBe(0);
  });

  it("exits on process.exit() with a write stream open and a write waiting", async () => {
    const r = await child(
      `const db = await createDatabase();
       db.stream("CREATE (n:S) RETURN n");
       db.execute("CREATE (:C)").catch(() => {});
       await sleep(50);
       process.exit(0);`,
      { threads: 2, ms: 10_000 },
    );
    expect(r.finished, r.out).toBe(true);
    expect(r.code).toBe(0);
  });
});

describe("panics in the native binding", () => {
  // A panic unwinding out of an N-API callback aborted the process. The
  // binding's test hooks panic on purpose.
  it("rejects with LORA_INTERNAL and leaves the handle usable", async () => {
    const db = await native.openDatabase();
    expect(() => db.__panicForTesting()).toThrow(/^LORA_INTERNAL: /);
    await expect(db.__panicInTaskForTesting(false)).rejects.toThrow(
      /^LORA_INTERNAL: /,
    );
    await expect(db.__panicInTaskForTesting(true)).rejects.toThrow(
      /^LORA_INTERNAL: /,
    );

    // The registries the sync panic held locked still work.
    const stream = db.openStream("RETURN 1 AS x", null, null, null);
    expect(db.streamNext(stream)).toEqual({ x: 1 });
    const token = db.createCancelToken(null);
    db.releaseCancelToken(token);
    const tx = await db.beginTransaction(null, null);
    await db.txFinish(tx, false);
    expect(await db.execute("CREATE (:Ok)", null, null, null)).toBeInstanceOf(
      Buffer,
    );
    expect(db.nodeCount()).toBe(1);
    db.dispose();
  });
});
