import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterAll, expect, test } from "vitest";
import { createDatabase } from "@loradb/lora-node";
import { LoraGraphQL, loraDriver } from "../src/index.js";
import { appTypeDefs } from "./fixtures.js";

const dirs: string[] = [];
afterAll(async () => {
  await Promise.all(dirs.map((d) => rm(d, { recursive: true, force: true })));
});

test("snapshot() writes an online copy of the database behind the API", async () => {
  const dir = await mkdtemp(join(tmpdir(), "lora-graphql-snapshot-"));
  dirs.push(dir);
  const db = await createDatabase("festimap", {
    databaseDir: join(dir, "live"),
  });
  const lora = new LoraGraphQL({
    typeDefs: appTypeDefs,
    driver: loraDriver(db),
  });
  await db.execute(
    "UNWIND range(1, 20) AS i CREATE (:Festival {key: 'f' + toString(i), name: 'F'})",
  );

  const snapshot = await lora.snapshot(join(dir, "copy"));
  expect(snapshot).toMatchObject({
    path: join(dir, "copy", "festimap.loradb"),
    nodeCount: 20,
    relationshipCount: 0,
  });
  expect(snapshot.bytes).toBeGreaterThan(0);
  expect(snapshot.lsn).toBeGreaterThan(0);

  const copy = await createDatabase("festimap", {
    databaseDir: join(dir, "copy"),
  });
  const { rows } = await copy.execute(
    "MATCH (f:Festival) RETURN count(f) AS c",
  );
  expect(rows[0]!["c"]).toBe(20);
  copy.dispose();
  db.dispose();
});

test("snapshot() names what it needs when the driver has none", async () => {
  const lora = new LoraGraphQL({
    typeDefs: appTypeDefs,
    driver: { run: async () => [] },
  });
  await expect(lora.snapshot("/nowhere")).rejects.toThrow(
    /needs a driver with snapshot\(\)/,
  );
});
