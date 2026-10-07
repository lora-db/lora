// The access model, stated as a table and run against a real database.
// Each probe runs in a transaction that is rolled back.

import { readFile } from "node:fs/promises";
import { test } from "node:test";
import {
  createTestLoraGraphQL,
  expectAccess,
} from "@loradb/lora-graphql/testing";

const file = (name: string) => readFile(new URL(name, import.meta.url), "utf8");

test("posts: published for everyone, drafts for their author", async () => {
  const t = await createTestLoraGraphQL({
    typeDefs: await file("./schema.graphql"),
    seed: await file("./seed.cypher"),
  });

  await expectAccess(t, {
    as: undefined, // an anonymous caller
    allowed: ["read Post engines"],
    denied: ["read Post notes", "delete Post engines"],
  });

  await expectAccess(t, {
    as: { sub: "ada" },
    allowed: ["read Post notes", "update Post notes", "delete Post notes"],
    denied: ["update Post cobol", "delete Post cobol"],
  });

  await expectAccess(t, {
    as: { sub: "grace" },
    allowed: ["read Post engines", "update Post cobol"],
    denied: ["read Post notes", "update Post engines"],
  });

  t.close();
});
