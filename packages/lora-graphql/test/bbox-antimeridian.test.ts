// `withinBBox` with a lower-left longitude greater than the upper-right one
// covers the antimeridian, and still seeks the point index.

import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL, expectSeeks } from "../src/testing.js";

type Result = {
  data?: unknown;
  errors?: ReadonlyArray<{ message: string; extensions?: unknown }>;
};
const keysOf = (r: Result, field: string) => {
  expect(r.errors).toBeUndefined();
  return ((r.data as Record<string, Array<{ key: string }>>)[field] ?? []).map(
    (x) => x.key,
  );
};

describe("withinBBox across the antimeridian", () => {
  const typeDefs =
    "type F @node { key: String! @key  at: Point! @filterable(byValue: [WITHIN_BBOX]) }";
  const seed = [
    "CREATE (:F {key:'fiji', at: point({longitude: 178.0, latitude: -18.0})}), (:F {key:'samoa', at: point({longitude: -172.0, latitude: -13.8})}), (:F {key:'paris', at: point({longitude: 2.35, latitude: 48.85})})",
  ];
  const across = `{ fs(where:{at:{withinBBox:{lowerLeft:{latitude:-60,longitude:170},upperRight:{latitude:60,longitude:-170}}}}, sort: [{ key: ASC }]){ key } }`;

  test("a box with west > east covers the date line, on the point index", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(keysOf(await t.run(across), "fs")).toEqual(["fiji", "samoa"]);
    await expectSeeks(t.lora, across);
  });

  test("an ordinary box is unchanged", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const europe = `{ fs(where:{at:{withinBBox:{lowerLeft:{latitude:40,longitude:-10},upperRight:{latitude:60,longitude:10}}}}){ key } }`;
    expect(keysOf(await t.run(europe), "fs")).toEqual(["paris"]);
  });
});
