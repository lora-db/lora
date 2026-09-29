// Latency of generated mutations on the bench graph, and of the connect
// pre-delete written as seek-then-expand against one pattern.
//   yarn bench

import { graphql } from "graphql";
import { bench, describe } from "vitest";
import { seed } from "./seed.js";

const { db, lora } = await seed();
const schema = lora.getSchema();

let n = 0;
const run = async (source: string) => {
  const r = await graphql({ schema, source, contextValue: {} });
  if (r.errors) throw r.errors[0];
};
const cypher = async (text: string, params: Record<string, unknown> = {}) => {
  await db.execute(text, params as never);
};

describe("mutations, end to end", () => {
  bench("create", () =>
    run(
      `mutation { createFestivals(input: [{ key: "p${n++}", name: "P" }]) { info { nodesCreated } } }`,
    ),
  );
  bench("create with a genre and two followers connected", () =>
    run(`mutation { createFestivals(input: [{
      key: "n${n++}", name: "N",
      genre: { connect: { key: "g1" } },
      followers: { connect: [{ key: "u1" }, { key: "u2" }] }
    }]) { info { relationshipsCreated } } }`),
  );
  bench("disconnect, then connect a follower", async () => {
    await run(
      `mutation { updateFestival(key: "f1", update: { followers: { disconnect: ["u1"] } }) { info { relationshipsDeleted } } }`,
    );
    await run(
      `mutation { updateFestival(key: "f1", update: { followers: { connect: [{ key: "u1" }] } }) { info { relationshipsCreated } } }`,
    );
  });
  bench("replace a single relationship", () =>
    run(
      `mutation { updateFestival(key: "f2", update: { genre: { connect: { key: "g${n++ % 10}" } } }) { info { relationshipsCreated } } }`,
    ),
  );
});

describe("connect pre-delete: seek then expand vs one pattern", () => {
  const rows = [{ from: "f3", to: "u9" }];
  bench("generated: seek the owner, then expand", () =>
    cypher(
      `UNWIND $rows AS row
       MATCH (a:Festival) WHERE a.key = row.from
       MATCH (a)<-[r:FOLLOWS]-(b:User) WHERE b:User AND b.key = row.to
       RETURN r`,
      { rows },
    ),
  );
  bench("naive: key tests in the expanding pattern", () =>
    cypher(
      `UNWIND $rows AS row
       MATCH (a:Festival)<-[r:FOLLOWS]-(b:User)
       WHERE b:User AND a.key = row.from AND b.key = row.to
       RETURN r`,
      { rows },
    ),
  );
});
