/**
 * A write never waits for the writer lock on the only thread.
 *
 * WebAssembly runs every call on one thread, and the engine's writer mutex
 * cannot wait there: locking it twice panics ("cannot recursively acquire
 * mutex") and traps the module with `RuntimeError: unreachable`. A write
 * stream holds that lock across calls until it is drained or closed, so a
 * write started meanwhile used to trap. It now fails with LORA_TRANSACTION,
 * reads keep working, and closing the stream frees the writer. Everything
 * else takes and releases the lock inside one synchronous call, so any
 * number of concurrent `transaction()` calls simply run one after another.
 *
 * Each case runs in a child process with a timeout, so a regression that
 * blocks the thread (e.g. a threaded build whose mutex does wait) kills the
 * child instead of hanging this runner.
 */
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const entry = fileURLToPath(new URL("../dist/index.js", import.meta.url));

function child(script: string, ms = 15_000) {
  return new Promise<{ finished: boolean; out: string; code: number | null }>(
    (resolve) => {
      const p = spawn(
        process.execPath,
        [
          "--input-type=module",
          "-e",
          `const { createDatabase } = await import(${JSON.stringify(entry)});
           const db = await createDatabase({ runtime: "main-thread" });
           const code = async (p) => { try { await p; return "ok"; } catch (e) { return e.code ?? String(e); } };
           ${script}`,
        ],
        { stdio: ["ignore", "pipe", "pipe"] },
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

function lastJson(out: string): Record<string, unknown> {
  const line = out.trim().split("\n").pop() ?? "";
  return JSON.parse(line) as Record<string, unknown>;
}

describe("the writer lock on one thread", () => {
  it("rejects writes while a write stream is open, serves reads, frees on close", async () => {
    const r = await child(
      `const stream = db.stream("UNWIND range(1, 3) AS i CREATE (:S {i: i}) RETURN i");
       await stream.next();
       const whileOpen = {
         execute: await code(db.execute("CREATE (:X)")),
         transaction: await code(db.transaction([{ query: "CREATE (:X)" }])),
         stream: await code((async () => db.stream("CREATE (:X) RETURN 1 AS x"))()),
         clear: await code(db.clear()),
         importRows: await code(db.importRowsWithCypher(new TextEncoder().encode('{"a":1}\\n'), "jsonl", "UNWIND $rows AS r CREATE (:X)")),
         importStream: await code(db.importStream(new Blob(['{"a":1}\\n']).stream(), "jsonl", "UNWIND $rows AS r CREATE (:X)")),
         read: await code(db.execute("MATCH (n) RETURN count(n) AS c")),
         readTx: await code(db.transaction([{ query: "MATCH (n) RETURN n" }], "read_only")),
         readStream: await code(db.stream("MATCH (n) RETURN n").toArray()),
         nodeCount: await db.nodeCount(),
         saveSnapshot: await code(db.saveSnapshot()),
       };
       stream.close();
       const afterClose = await db.nodeCount();
       const writeAfter = await code(db.execute("CREATE (:Y)"));
       const drained = await db.stream("UNWIND range(1, 3) AS i CREATE (:S {i: i}) RETURN i").toArray();
       const writeAfterDrain = await code(db.execute("CREATE (:Y)"));
       console.log(JSON.stringify({ whileOpen, afterClose, writeAfter, drained: drained.length, writeAfterDrain, total: await db.nodeCount() }));`,
    );
    expect(r.finished, r.out).toBe(true);
    expect(r.code, r.out).toBe(0);
    expect(lastJson(r.out)).toEqual({
      whileOpen: {
        execute: "LORA_TRANSACTION",
        transaction: "LORA_TRANSACTION",
        stream: "LORA_TRANSACTION",
        clear: "LORA_TRANSACTION",
        importRows: "LORA_TRANSACTION",
        importStream: "LORA_TRANSACTION",
        read: "ok",
        readTx: "ok",
        readStream: "ok",
        nodeCount: 0,
        saveSnapshot: "ok",
      },
      afterClose: 0,
      writeAfter: "ok",
      drained: 3,
      writeAfterDrain: "ok",
      total: 5,
    });
  });

  it("holds the writer for a write export until it is cancelled", async () => {
    const r = await child(
      `await db.execute("UNWIND range(1, 600) AS i CREATE (:N {i: i})");
       const rs = db.openExportStream("MATCH (n:N) SET n.seen = true RETURN n.i AS i", null, "jsonl");
       const reader = rs.getReader();
       await reader.read();
       const whileOpen = await code(db.execute("CREATE (:X)"));
       await reader.cancel();
       const afterCancel = await code(db.execute("CREATE (:X)"));
       console.log(JSON.stringify({ whileOpen, afterCancel }));`,
    );
    expect(r.finished, r.out).toBe(true);
    expect(lastJson(r.out)).toEqual({
      whileOpen: "LORA_TRANSACTION",
      afterCancel: "ok",
    });
  });

  it("runs 50 concurrent transactions, auto-commit writes and reads", async () => {
    const r = await child(
      `const txs = Array.from({ length: 50 }, (_, i) =>
         db.transaction([
           { query: "CREATE (:T {i: $i})", params: { i } },
           { query: "MATCH (t:T) RETURN count(t) AS c" },
         ]));
       const writes = Array.from({ length: 20 }, (_, i) => db.execute("CREATE (:W {i: $i})", { i }));
       const reads = Array.from({ length: 20 }, () => db.execute("MATCH (n) RETURN count(n) AS c"));
       await Promise.all([...txs, ...writes, ...reads]);
       console.log(JSON.stringify({ nodes: await db.nodeCount() }));`,
    );
    expect(r.finished, r.out).toBe(true);
    expect(lastJson(r.out)).toEqual({ nodes: 70 });
  });

  it("keeps streams usable after their database is disposed", async () => {
    const r = await child(
      `const write = db.stream("UNWIND range(1, 3) AS i CREATE (:S {i: i}) RETURN i");
       await write.next();
       const read = db.stream("UNWIND range(1, 3) AS i RETURN i");
       await read.next();
       const exp = db.openExportStream("UNWIND range(1, 600) AS i RETURN i", null, "jsonl").getReader();
       await exp.read();
       db.dispose();
       const rest = (await write.toArray()).length + (await read.toArray()).length;
       await exp.cancel();
       const other = await createDatabase({ runtime: "main-thread" });
       await other.execute("UNWIND range(1, 100) AS i CREATE (:N {i: i})");
       console.log(JSON.stringify({ rest, other: await other.nodeCount() }));`,
    );
    expect(r.finished, r.out).toBe(true);
    expect(r.code, r.out).toBe(0);
    expect(lastJson(r.out)).toEqual({ rest: 4, other: 100 });
  });
});
