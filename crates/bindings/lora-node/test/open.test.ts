import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";
import { createDatabase } from "../ts/index.js";

const dirs: string[] = [];
afterEach(async () => {
  for (const dir of dirs.splice(0))
    await rm(dir, { recursive: true, force: true });
});

async function tempDir(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), "lora-open-"));
  dirs.push(dir);
  return dir;
}

describe("createDatabase", () => {
  it("opens a persistent database off the event loop", async () => {
    const dir = await tempDir();
    const writer = await createDatabase("app", { databaseDir: dir });
    await writer.execute(
      "UNWIND range(1, 20000) AS i CREATE (:Record {key: i, title: 'record ' + toString(i)})",
    );
    writer.dispose();

    // A blocking open would resolve as a microtask before any timer ran.
    let ticks = 0;
    const timer = setInterval(() => ticks++, 1);
    try {
      const db = await createDatabase("app", { databaseDir: dir });
      expect(ticks).toBeGreaterThan(0);
      expect(await db.nodeCount()).toBe(20000);
      db.dispose();
    } finally {
      clearInterval(timer);
    }
  });

  it("rejects a failed open instead of throwing synchronously", async () => {
    const dir = await tempDir();
    const pending = createDatabase("../escape", { databaseDir: dir });
    await expect(pending).rejects.toMatchObject({ code: "LORA_DATABASE_NAME" });
  });
});
