// Online snapshots (`db.snapshot(targetDir)`), `db.checkpoint()` and the
// `lora snapshot` command. The kill -9 and command tests run compiled code
// in child processes: build it first (`yarn build:ts`).

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterAll, describe, expect, it } from "vitest";
import {
  createDatabase,
  LoraError,
  type Database,
  type LoraChangeBatch,
} from "../ts/index.js";

const dirs: string[] = [];
async function tempDir(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), "lora-node-snapshot-"));
  dirs.push(dir);
  return dir;
}
afterAll(async () => {
  await Promise.all(dirs.map((d) => rm(d, { recursive: true, force: true })));
});

const dist = join(process.cwd(), "dist");

async function count(db: Database): Promise<number> {
  const { rows } = await db.execute("MATCH (n) RETURN count(n) AS c");
  return rows[0]!.c as number;
}

/** Nodes each write transaction creates: a copy holds a multiple of it. */
const PER_TX = 7;
const WRITE_TX = "UNWIND range(1, 7) AS i CREATE (:W {tx: $tx, i: i})";

describe("db.snapshot()", () => {
  it("writes a copy that opens as the graph it was taken from", async () => {
    const dir = await tempDir();
    const live = join(dir, "live");
    const target = join(dir, "copy");
    const db = await createDatabase("festimap", { databaseDir: live });
    expect(typeof db.snapshot).toBe("function");
    expect(typeof db.checkpoint).toBe("function");
    await db.execute(
      "CREATE CONSTRAINT rk IF NOT EXISTS FOR (r:Record) REQUIRE r.key IS UNIQUE",
    );
    await db.execute(
      "UNWIND range(1, 1000) AS i CREATE (:Record {key: 'k' + toString(i)})",
    );

    const snapshot = await db.snapshot(target);
    expect(snapshot).toEqual({
      path: join(target, "festimap.loradb"),
      bytes: expect.any(Number),
      lsn: expect.any(Number),
      nodeCount: 1000,
      relationshipCount: 0,
    });
    expect(snapshot.bytes).toBeGreaterThan(0);
    expect(await readdir(target)).toEqual(["festimap.loradb"]);
    await db.execute("CREATE (:Record {key: 'later'})");

    const copy = await createDatabase("festimap", { databaseDir: target });
    expect(await count(copy)).toBe(1000);
    await expect(
      copy.execute("CREATE (:Record {key: 'k1'})"),
    ).rejects.toMatchObject({ code: "LORA_UNIQUE_CONSTRAINT" });
    copy.dispose();
    db.dispose();
  });

  it("refuses a target that is not empty, and an in-memory database", async () => {
    const dir = await tempDir();
    const db = await createDatabase("app", { databaseDir: join(dir, "live") });
    await db.execute("CREATE (:X)");
    const full = join(dir, "full");
    await mkdir(full);
    await writeFile(join(full, "other"), "x");
    const err = await db.snapshot(full).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(LoraError);
    expect((err as LoraError).code).toBe("LORA_VALIDATION");
    db.dispose();

    const memory = await createDatabase();
    await expect(memory.snapshot(join(dir, "m"))).rejects.toMatchObject({
      code: "LORA_CONFIG",
    });
    await expect(memory.checkpoint()).rejects.toMatchObject({
      code: "LORA_CONFIG",
    });
  });

  it("holds whole transactions, exactly those up to its LSN, while writes run", async () => {
    const dir = await tempDir();
    const db = await createDatabase("app", { databaseDir: join(dir, "live") });
    const feed = db.changes({ bufferSize: 100_000 });
    await feed.ready;

    let stop = false;
    let written = 0;
    const writer = (async () => {
      while (!stop) await db.execute(WRITE_TX, { tx: written++ });
    })();
    const snapshots = [];
    for (let round = 0; round < 3; round++) {
      await new Promise((resolve) => setTimeout(resolve, 30));
      snapshots.push(await db.snapshot(join(dir, `copy-${round}`)));
    }
    stop = true;
    await writer;

    const batches: LoraChangeBatch[] = [];
    while (batches.length < written) {
      const next = await feed.next();
      if (next.done) break;
      batches.push(next.value);
    }
    await feed.return();
    expect(batches.length).toBe(written);

    for (const [round, snapshot] of snapshots.entries()) {
      const copy = await createDatabase("app", {
        databaseDir: join(dir, `copy-${round}`),
      });
      const nodes = await count(copy);
      copy.dispose();
      expect(nodes).toBe(snapshot.nodeCount);
      expect(nodes % PER_TX).toBe(0);
      const committed = batches
        .filter((b) => b.lsn <= snapshot.lsn)
        .reduce(
          (sum, b) =>
            sum + b.changes.filter((c) => c.kind === "nodeCreated").length,
          0,
        );
      expect(nodes).toBe(committed);
    }
    db.dispose();
  });
});

describe("db.checkpoint()", () => {
  it("checkpoints a named database and reports the snapshot", async () => {
    const dir = await tempDir();
    const db = await createDatabase("app", { databaseDir: dir });
    await db.execute("UNWIND range(1, 10) AS i CREATE (:X {i: i})");
    const meta = await db.checkpoint();
    expect(meta).toEqual({
      formatVersion: expect.any(Number),
      nodeCount: 10,
      relationshipCount: 0,
      walLsn: expect.any(Number),
    });
    db.dispose();
  });
});

/** Run `node <args>`; resolve with its exit code and output. */
function runNode(
  args: string[],
): Promise<{ code: number | null; stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, args, {
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (c) => (stdout += c));
    child.stderr.on("data", (c) => (stderr += c));
    child.on("error", reject);
    child.on("exit", (code) => resolve({ code, stdout, stderr }));
  });
}

describe("kill -9 during a snapshot", () => {
  it("leaves the live directory and every finished snapshot valid", async () => {
    const dir = await tempDir();
    const PRELOAD = 50_000;
    const script = `
      const { createDatabase } = require(${JSON.stringify(join(dist, "index.js"))});
      const dir = ${JSON.stringify(dir)};
      (async () => {
        const db = await createDatabase("app", { databaseDir: dir + "/live" });
        await db.execute("UNWIND range(1, ${PRELOAD}) AS i CREATE (:Pre {i: i, s: 'node ' + toString(i)})");
        let tx = 0;
        (async () => { for (;;) await db.execute(${JSON.stringify(WRITE_TX)}, { tx: tx++ }); })();
        for (let round = 0; ; round++) {
          process.stdout.write("start " + round + "\\n");
          const s = await db.snapshot(dir + "/copy-" + round);
          process.stdout.write("done " + round + " " + s.lsn + " " + s.nodeCount + "\\n");
        }
      })();
    `;
    const child = spawn(process.execPath, ["-e", script], {
      stdio: ["ignore", "pipe", "inherit"],
    });
    const done: Array<{ round: number; nodes: number }> = [];
    const killedIn = await new Promise<number>((resolve, reject) => {
      let buffered = "";
      child.on("exit", () => reject(new Error("child exited before the kill")));
      child.stdout.on("data", (chunk: Buffer) => {
        buffered += chunk.toString();
        let newline;
        while ((newline = buffered.indexOf("\n")) !== -1) {
          const [word, round, , nodes] = buffered.slice(0, newline).split(" ");
          buffered = buffered.slice(newline + 1);
          if (word === "done") {
            done.push({ round: Number(round), nodes: Number(nodes) });
          } else if (word === "start" && done.length > 0) {
            // A few milliseconds into the next snapshot.
            setTimeout(() => {
              child.removeAllListeners("exit");
              child.kill("SIGKILL");
              resolve(Number(round));
            }, 5);
            child.stdout.removeAllListeners("data");
            return;
          }
        }
      });
    });
    await new Promise((resolve) => child.once("exit", resolve));

    const whole = (nodes: number) => {
      expect(nodes).toBeGreaterThanOrEqual(PRELOAD);
      expect((nodes - PRELOAD) % PER_TX).toBe(0);
    };
    // The lock went with the process: the live directory opens, whole.
    const live = await createDatabase("app", {
      databaseDir: join(dir, "live"),
    });
    const liveNodes = await count(live);
    whole(liveNodes);
    live.dispose();

    for (const { round, nodes } of done) {
      const copy = await createDatabase("app", {
        databaseDir: join(dir, `copy-${round}`),
      });
      expect(await count(copy)).toBe(nodes);
      whole(nodes);
      expect(nodes).toBeLessThanOrEqual(liveNodes);
      copy.dispose();
    }

    // The interrupted copy has no container, only scratch the next open
    // removes, or a complete one.
    const interrupted = join(dir, `copy-${killedIn}`);
    if (existsSync(join(interrupted, "app.loradb"))) {
      const copy = await createDatabase("app", { databaseDir: interrupted });
      whole(await count(copy));
      copy.dispose();
    } else {
      for (const name of existsSync(interrupted)
        ? await readdir(interrupted)
        : []) {
        expect(name).toMatch(/^app\.loradb\..*\.tmp$/);
      }
    }
  }, 60_000);
});

describe("lora snapshot", () => {
  const cli = join(dist, "cli.js");

  it("copies a closed database directory", async () => {
    const dir = await tempDir();
    const db = await createDatabase("festimap", {
      databaseDir: join(dir, "live"),
    });
    await db.execute("UNWIND range(1, 25) AS i CREATE (:X {i: i})");
    db.dispose();

    const out = await runNode([
      cli,
      "snapshot",
      join(dir, "live"),
      join(dir, "copy"),
    ]);
    expect(out.stderr).toBe("");
    expect(out.code).toBe(0);
    expect(out.stdout).toContain("festimap.loradb");
    expect(out.stdout).toContain("25 nodes");

    const copy = await createDatabase("festimap", {
      databaseDir: join(dir, "copy"),
    });
    expect(await count(copy)).toBe(25);
    copy.dispose();
  });

  it("refuses a directory another process has open", async () => {
    const dir = await tempDir();
    const db = await createDatabase("festimap", { databaseDir: dir });
    await db.execute("CREATE (:X)");

    const out = await runNode([
      cli,
      "snapshot",
      dir,
      join(await tempDir(), "copy"),
    ]);
    expect(out.code).toBe(1);
    expect(out.stderr).toContain("is open in another process");
    expect(out.stderr).toContain("db.snapshot(targetDir)");
    db.dispose();
  });

  it("asks which database when the directory holds several", async () => {
    const dir = await tempDir();
    for (const name of ["a", "b"]) {
      const db = await createDatabase(name, { databaseDir: dir });
      await db.execute("CREATE (:X)");
      db.dispose();
    }
    const out = await runNode([
      cli,
      "snapshot",
      dir,
      join(await tempDir(), "copy"),
    ]);
    expect(out.code).toBe(2);
    expect(out.stderr).toContain("pick one with --name");

    const picked = await runNode([
      cli,
      "snapshot",
      dir,
      join(await tempDir(), "copy"),
      "--name",
      "b",
    ]);
    expect(picked.code).toBe(0);
  });
});
