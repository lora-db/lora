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
});
