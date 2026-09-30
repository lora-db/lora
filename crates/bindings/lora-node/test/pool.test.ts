/**
 * Interactive transactions never occupy a libuv worker while they wait.
 *
 * A write transaction waiting for the writer lock used to block a pool thread;
 * with more waiting transactions than pool threads, the transaction holding the
 * lock could never get a thread for its next statement and the process stopped,
 * reads included (E-3 in the Festimap brief). Each case runs in a child process
 * with a tiny pool, so a regression hangs the child — which is killed — instead
 * of this runner.
 */
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const entry = fileURLToPath(new URL("../dist/index.js", import.meta.url));

function child(script: string, threads: number, ms = 15_000) {
  return new Promise<{ finished: boolean; out: string; code: number | null }>(
    (resolve) => {
      const p = spawn(
        process.execPath,
        [
          "--input-type=module",
          "-e",
          `const { createDatabase } = await import(${JSON.stringify(entry)});\n${script}`,
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

describe("interactive transactions and the libuv pool", () => {
  it("commits 50 concurrent transactions on a 2-thread pool, reads answering meanwhile", async () => {
    const r = await child(
      `const db = await createDatabase();
       await db.execute("CREATE (:N {key: 'a', n: 0})");
       const one = async () => {
         const tx = await db.begin();
         const { rows } = await tx.execute("MATCH (n:N {key: 'a'}) RETURN n.n AS n");
         await new Promise((r) => setTimeout(r, 2)); // decide in JS
         await tx.execute("MATCH (n:N {key: 'a'}) SET n.n = $n", { n: rows[0].n + 1 });
         await tx.commit();
       };
       const reads = Array.from({ length: 20 }, () => db.execute("MATCH (n:N) RETURN count(n) AS c"));
       await Promise.all([...Array.from({ length: 50 }, one), ...reads]);
       const { rows } = await db.execute("MATCH (n:N {key: 'a'}) RETURN n.n AS n");
       console.log("N=" + rows[0].n);`,
      2,
    );
    expect(r.finished, r.out).toBe(true);
    // Serialised by the writer lock: every read-modify-write saw the last commit.
    expect(r.out.trim()).toBe("N=50");
  });

  it("mixes interactive transactions, batches and auto-commit writes on a 1-thread pool", async () => {
    const r = await child(
      `const db = await createDatabase();
       await db.execute("CREATE (:N {key: 'a', n: 0})");
       const inc = "MATCH (n:N {key: 'a'}) SET n.n = n.n + 1";
       const jobs = [];
       for (let i = 0; i < 30; i++) {
         if (i % 3 === 0) jobs.push((async () => { const tx = await db.begin(); await tx.execute(inc); await new Promise((r) => setTimeout(r, 1)); await tx.executeMany([{ query: inc }]); await tx.commit(); })());
         else if (i % 3 === 1) jobs.push(db.transaction([{ query: inc }, { query: inc }]));
         else jobs.push(db.execute(inc));
       }
       await Promise.all(jobs);
       const { rows } = await db.execute("MATCH (n:N {key: 'a'}) RETURN n.n AS n");
       console.log("N=" + rows[0].n);`,
      1,
    );
    expect(r.finished, r.out).toBe(true);
    // 10 interactive × 2 + 10 batches × 2 + 10 auto-commit × 1
    expect(r.out.trim()).toBe("N=50");
  });

  it("exits while transactions wait for the writer lock", async () => {
    const r = await child(
      `const db = await createDatabase();
       const holder = await db.begin();
       await holder.execute("CREATE (:N {key: 'x'})");
       for (let i = 0; i < 8; i++) void db.begin(); // they wait for the lock
       await new Promise((r) => setTimeout(r, 100));
       console.log("EXITING");
       process.exit(0);`,
      2,
      10_000,
    );
    expect(r).toMatchObject({ finished: true, code: 0 });
    expect(r.out).toContain("EXITING");
  });

  it("rolls back on a failed statement and lets the next writer in", async () => {
    const r = await child(
      `const db = await createDatabase();
       await db.execute("CREATE CONSTRAINT k IF NOT EXISTS FOR (n:N) REQUIRE n.key IS UNIQUE");
       await db.execute("CREATE (:N {key: 'taken'})");
       const failing = async () => {
         const tx = await db.begin();
         try { await tx.execute("CREATE (:N {key: 'taken'})"); } catch { return "rolled back"; }
         return "committed?";
       };
       const results = await Promise.all(Array.from({ length: 10 }, failing));
       const tx = await db.begin();
       await tx.execute("CREATE (:N {key: 'fresh'})");
       await tx.commit();
       const { rows } = await db.execute("MATCH (n:N) RETURN count(n) AS c");
       console.log(JSON.stringify({ results: [...new Set(results)], count: rows[0].c }));`,
      2,
    );
    expect(r.finished, r.out).toBe(true);
    expect(JSON.parse(r.out.trim())).toEqual({
      results: ["rolled back"],
      count: 2,
    });
  });
  it("settles every command queued behind a failed statement and exits", async () => {
    const r = await child(
      `const db = await createDatabase();
       const status = (all) => all.map((x) => x.status + ":" + (x.reason?.code ?? ""));
       const tx = await db.begin();
       const a = await Promise.allSettled([tx.execute("THIS IS NOT CYPHER"), tx.execute("RETURN 1"), tx.executeMany([{ query: "RETURN 1" }])]);
       const tx2 = await db.begin();
       const b = await Promise.allSettled([tx2.execute("THIS IS NOT CYPHER"), tx2.commit()]);
       const tx3 = await db.begin();
       const c = await Promise.allSettled([tx3.commit(), tx3.execute("RETURN 1")]);
       console.log(JSON.stringify([status(a), status(b), status(c)]));`,
      2,
      10_000,
    );
    expect(r).toMatchObject({ finished: true, code: 0 });
    expect(JSON.parse(r.out.trim())).toEqual([
      [
        "rejected:LORA_PARSE",
        "rejected:LORA_TRANSACTION",
        "rejected:LORA_TRANSACTION",
      ],
      ["rejected:LORA_PARSE", "rejected:LORA_TRANSACTION"],
      ["fulfilled:", "rejected:LORA_TRANSACTION"],
    ]);
  });

  it("rejects begin() after dispose() and exits", async () => {
    const r = await child(
      `const db = await createDatabase();
       db.dispose();
       await db.begin().then(() => console.log("opened?"), (e) => console.log("rejected " + e.message));`,
      2,
      10_000,
    );
    expect(r).toMatchObject({ finished: true, code: 0 });
    expect(r.out.trim()).toBe("rejected database is closed");
  });

  it("returns from dispose() promptly with a begin() waiting on a shared directory", async () => {
    // Two handles on one directory share an engine and its writer lock. The
    // lock holder is a transaction on the other handle, which needs the JS
    // thread for its next command: dispose() must not wait for the waiter.
    const r = await child(
      `const { mkdtempSync } = await import("node:fs");
       const { tmpdir } = await import("node:os");
       const { join } = await import("node:path");
       const dir = mkdtempSync(join(tmpdir(), "lora-pool-"));
       const a = await createDatabase("g", { databaseDir: dir });
       const b = await createDatabase("g", { databaseDir: dir });
       const held = await b.begin();
       await held.execute("CREATE (:N)");
       const waiting = a.begin().then(() => "opened?", (e) => "rejected " + e.code);
       await new Promise((r) => setTimeout(r, 50));
       a.dispose();
       const outcome = await waiting;
       await held.commit();
       const { rows } = await b.execute("MATCH (n:N) RETURN count(n) AS c");
       b.dispose();
       console.log(JSON.stringify({ outcome, count: rows[0].c }));`,
      2,
      10_000,
    );
    expect(r).toMatchObject({ finished: true, code: 0 });
    expect(JSON.parse(r.out.trim())).toEqual({
      outcome: "rejected LORA_TRANSACTION",
      count: 1,
    });
  });

  it("opens a mutating stream while a transaction holds the writer lock", async () => {
    // Opening it used to wait for the lock on the JS thread, which the
    // transaction needed to commit.
    const r = await child(
      `const db = await createDatabase();
       const tx = await db.begin();
       await tx.execute("CREATE (:A)");
       const s = db.stream("MATCH (n) SET n.y = 1 RETURN n.y AS y");
       const cols = s.columns();
       const rows = s.toArray();
       await tx.commit();
       console.log(JSON.stringify({ cols, rows: await rows }));
       const unfinished = db.stream("MATCH (n) SET n.y = 2 RETURN n.y AS y");
       await unfinished.next();
       unfinished.close(); // rolls back
       const { rows: after } = await db.execute("MATCH (n) RETURN n.y AS y");
       console.log(JSON.stringify(after));`,
      2,
      10_000,
    );
    expect(r).toMatchObject({ finished: true, code: 0 });
    expect(r.out.trim().split("\n")).toEqual([
      JSON.stringify({ cols: ["y"], rows: [{ y: 1 }] }),
      JSON.stringify([{ y: 1 }]),
    ]);
  });

  it("starts a statement's timeout when it runs, not while it is queued", async () => {
    const r = await child(
      `const db = await createDatabase();
       const tx = await db.begin();
       const slow = tx.execute("UNWIND range(1, 3000000) AS x RETURN count(x) AS c");
       const quick = tx.execute("RETURN 1 AS one", {}, { timeoutMs: 200 });
       const [s, q] = await Promise.allSettled([slow, quick]);
       await tx.rollback();
       console.log(s.status + " " + (q.status === "fulfilled" ? "ok" : q.reason.code));`,
      2,
    );
    expect(r).toMatchObject({ finished: true, code: 0 });
    expect(r.out.trim()).toBe("fulfilled ok");
  });
});
