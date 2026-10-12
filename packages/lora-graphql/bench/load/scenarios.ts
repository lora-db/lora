// The operations the load harness sends, against the bench graph
// (bench/seed.ts, appTypeDefs). Each isolates one cost: a keyed seek, an
// ordered page, a traversal, a count, a @cypher field, a write. A scenario
// is one operation or a weighted mix of them.

import type { GraphSize } from "../seed.js";

export type Rng = () => number;

/** mulberry32: a small seeded PRNG, so runs pick the same keys. */
export function rng(seed: number): Rng {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export interface Operation {
  query: string;
  variables(pick: (n: number) => number, size: GraphSize, seq: number): object;
  write?: boolean;
}

const festival = (pick: (n: number) => number, size: GraphSize) =>
  "f" + pick(size.festivals);
const user = (pick: (n: number) => number, size: GraphSize) =>
  "u" + pick(size.users);

/**
 * The festivals `touch` writes and `--subscribers` follow: few enough that
 * every write has followers.
 */
export const HOT_FESTIVALS = 1000;

export const operations = {
  byKey: {
    query: `query ($key: ID!) { festival(key: $key) { key name capacity } }`,
    variables: (pick, size) => ({ key: festival(pick, size) }),
  },
  page: {
    query: `query ($min: Int) {
      festivals(where: { capacity: { gt: $min } }, sort: [{ name: ASC }], limit: 25) { key name capacity }
    }`,
    variables: (pick) => ({ min: pick(4000) }),
  },
  traverse: {
    query: `query ($key: String!) {
      user(key: $key) { name follows(limit: 10) { key name genre { name } } }
    }`,
    variables: (pick, size) => ({ key: user(pick, size) }),
  },
  connection: {
    query: `query ($min: Int) {
      festivalsConnection(where: { capacity: { gt: $min } }, first: 20) {
        totalCount edges { cursor node { key name } } pageInfo { hasNextPage endCursor }
      }
    }`,
    variables: (pick) => ({ min: pick(4000) }),
  },
  aggregate: {
    query: `query ($min: Int) { festivalsAggregate(where: { capacity: { gt: $min } }) { count } }`,
    variables: (pick) => ({ min: pick(4000) }),
  },
  cypherField: {
    query: `query ($min: Int) {
      festivals(where: { capacity: { gt: $min } }, limit: 25) { key followerCount similar(limit: 3) { key } }
    }`,
    variables: (pick) => ({ min: pick(4000) }),
  },
  wide: {
    query: `{ users(limit: 100) { key follows(limit: 100) { key name capacity } } }`,
    variables: () => ({}),
  },
  create: {
    query: `mutation ($name: String!) { createFestivals(input: [{ name: $name }]) { festivals { key } } }`,
    variables: (_pick, _size, seq) => ({ name: `load ${seq}` }),
    write: true,
  },
  update: {
    query: `mutation ($key: ID!, $capacity: Int) {
      updateFestival(key: $key, update: { capacity: $capacity }) { festival { key capacity } }
    }`,
    variables: (pick, size) => ({
      key: festival(pick, size),
      capacity: pick(5000),
    }),
    write: true,
  },
  /** A create that connects two existing nodes: one call into the database. */
  createConnect: {
    query: `mutation ($name: String!, $genre: String!, $user: String!) {
      createFestivals(input: [{ name: $name, genre: { connect: { key: $genre } }, followers: { connect: [{ key: $user }] } }]) {
        festivals { key genre { key } }
      }
    }`,
    variables: (pick, size, seq) => ({
      name: `load ${seq}`,
      genre: "g" + pick(10),
      user: user(pick, size),
    }),
    write: true,
  },
  /** A bulk update: the keys are looked up, then written, in a transaction. */
  bulkUpdate: {
    query: `mutation ($min: Int, $max: Int, $capacity: Int) {
      updateFestivals(where: { capacity: { gt: $min, lt: $max } }, update: { capacity: $capacity }, limit: 100) {
        info { nodesUpdated }
      }
    }`,
    variables: (pick) => {
      // Two capacity values (4 festivals each per unit of scale), written
      // back into the range they were found in: on average it stays full.
      const min = pick(4990);
      return { min, max: min + 3, capacity: min + 1 + pick(2) };
    },
    write: true,
  },
  /**
   * A delete of a node with relationships: its neighbours are read first.
   * The graph shrinks as it runs, and a key drawn again deletes nothing.
   */
  delete: {
    query: `mutation ($key: ID!) { deleteFestival(key: $key) { nodesDeleted relationshipsDeleted } }`,
    variables: (pick, size) => ({ key: festival(pick, size) }),
    write: true,
  },
  /**
   * An update of a hot festival that stamps when it was sent, for
   * `--subscribers`: an event's delay is counted from that moment.
   */
  touch: {
    query: `mutation ($key: ID!, $name: String!) {
      updateFestival(key: $key, update: { name: $name }) { festival { key } }
    }`,
    variables: (pick, size) => ({
      key: "f" + pick(Math.min(HOT_FESTIVALS, size.festivals)),
      name: `at ${Date.now()}`,
    }),
    write: true,
  },
  connect: {
    query: `mutation ($user: String!, $festival: ID!) {
      updateUser(key: $user, update: { follows: { connect: [{ key: $festival }] } }) { user { key } }
    }`,
    variables: (pick, size) => ({
      user: user(pick, size),
      festival: festival(pick, size),
    }),
    write: true,
  },
} satisfies Record<string, Operation>;

export type OperationName = keyof typeof operations;

/** A scenario: operations with relative weights. */
export type Scenario = Partial<Record<OperationName, number>>;

export const scenarios: Record<string, Scenario> = {
  ...(Object.fromEntries(
    Object.keys(operations).map((name) => [name, { [name]: 1 }]),
  ) as Record<OperationName, Scenario>),
  /** A read-heavy API: mostly seeks and traversals, 10% writes. */
  mixed: {
    byKey: 4,
    traverse: 2,
    page: 1,
    connection: 1,
    cypherField: 1,
    update: 1,
  },
  /** Cheap reads while writers hold the writer lock. */
  readsUnderWrites: { byKey: 1, update: 1 },
};

/** Draw operation names from a scenario's weights. */
export function picker(scenario: Scenario, random: Rng): () => OperationName {
  const entries = Object.entries(scenario) as Array<[OperationName, number]>;
  const total = entries.reduce((sum, [, w]) => sum + w, 0);
  return () => {
    let r = random() * total;
    for (const [name, weight] of entries) {
      r -= weight;
      if (r < 0) return name;
    }
    return entries[entries.length - 1]![0];
  };
}
