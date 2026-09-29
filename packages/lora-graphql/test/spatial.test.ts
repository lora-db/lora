import { festivalHarness, type Harness } from "./harness.js";

const typeDefs = /* GraphQL */ `
  type Venue @node @mutation {
    key: String! @key
    name: String!
    location: Point! @filterable(byValue: [WITHIN_BBOX, DISTANCE])
    stage: CartesianPoint
  }
`;

let h: Harness;
beforeAll(async () => {
  h = await festivalHarness({ typeDefs, seed: [] });
  await h.data(`mutation {
    createVenues(input: [
      { key: "ams", name: "Amsterdam", location: { longitude: 4.90, latitude: 52.37 }, stage: { x: 1, y: 2 } }
      { key: "utr", name: "Utrecht", location: { longitude: 5.12, latitude: 52.09 } }
      { key: "ber", name: "Berlin", location: { longitude: 13.40, latitude: 52.52 } }
    ]) { info { nodesCreated } }
  }`);
});

test("points round-trip through mutations", async () => {
  const d = await h.data<{ venue: unknown }>(
    `{ venue(key: "ams") { location { longitude latitude srid crs } stage { x y crs } } }`,
  );
  expect(d.venue).toEqual({
    location: { longitude: 4.9, latitude: 52.37, srid: 4326, crs: "WGS-84-2D" },
    stage: { x: 1, y: 2, crs: "cartesian" },
  });
});

test("bounding box and distance filters", async () => {
  const keys = async (where: string) =>
    (
      await h.data<{ venues: Array<{ key: string }> }>(
        `{ venues(where: ${where}) { key } }`,
      )
    ).venues.map((v) => v.key);
  expect(
    await keys(`{ location: { withinBBox: {
      lowerLeft: { longitude: 3.3, latitude: 50.7 },
      upperRight: { longitude: 7.2, latitude: 53.6 } } } }`),
  ).toEqual(["ams", "utr"]);
  expect(
    await keys(
      `{ location: { distance: { from: { longitude: 4.9, latitude: 52.37 }, lte: 50000 } } }`,
    ),
  ).toEqual(["ams", "utr"]);
});

test("spatial filters imply a POINT index and seek through it", async () => {
  expect(
    h.lora
      .requirements()
      .map((r) => (r.kind === "index" ? r.index : r.constraint)),
  ).toEqual(["NODE_KEY", "POINT"]);
  const [f] = await h.lora.explain(
    `{ venues(where: { location: { distance: { from: { longitude: 4.9, latitude: 52.37 }, lte: 1000 } } }) { key } }`,
  );
  expect(f!.reports[0]!.operators).toContain("NodeByPointScan");
  expect(f!.reports[0]!.findings).toEqual([]);
});
