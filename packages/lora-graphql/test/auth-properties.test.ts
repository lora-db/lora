// Property-based checks of read rules: random graphs, claim sets and
// context values (missing, null, prototype-like keys) and random nested
// AND / OR / NOT filters. Every read path is compared against a reference
// evaluator in JavaScript, and must never return a node outside the rules.

import { createDatabase } from "@loradb/lora-node";
import fc from "fast-check";
import { graphql, type GraphQLSchema } from "graphql";
import { expect, test } from "vitest";
import { LoraGraphQL, loraDriver, toGlobalId } from "../src/index.js";
import { encodeCursor, toBase64Url } from "../src/compile/cursor.js";

const typeDefs = /* GraphQL */ `
  type Doc
    @node
    @query(aggregate: true)
    @authorization(
      filter: [
        {
          where: {
            AND: [
              { node: { tenant: { eq: "$context.tenant" } } }
              {
                OR: [
                  { node: { owner: { eq: "$jwt.sub" } } }
                  { jwt: { roles: { includes: "admin" } } }
                ]
              }
            ]
          }
        }
      ]
    ) {
    key: String! @key @relayId
    tenant: String!
    owner: String! @filterable(byValue: [EQ, IN])
    level: Int! @filterable(byValue: [EQ, GT, LT]) @sortable
    links: [Doc!]! @relationship(type: "LINKS", direction: OUT)
  }
`;

interface DocRow {
  key: string;
  tenant: string;
  owner: string;
  level: number;
}

type Where =
  | { AND: Where[] }
  | { OR: Where[] }
  | { NOT: Where }
  | { level: { eq?: number; gt?: number; lt?: number } }
  | { owner: { eq?: string; in?: string[] } };

const owners = ["a", "b", "c"];

const docArb = (i: number) =>
  fc.record({
    key: fc.constant<string>(`d${i}`),
    tenant: fc.constantFrom("t1", "t2"),
    owner: fc.constantFrom(...owners),
    level: fc.integer({ min: 0, max: 9 }),
  });

const graphArb = fc.integer({ min: 0, max: 12 }).chain((n) =>
  fc.record({
    docs: fc.tuple(...Array.from({ length: n }, (_, i) => docArb(i))),
    links: fc.array(
      fc.tuple(
        fc.nat({ max: Math.max(n - 1, 0) }),
        fc.nat({ max: Math.max(n - 1, 0) }),
      ),
      { maxLength: n * 2 },
    ),
  }),
);

// Claims arrive as parsed JSON, so "__proto__" is an own key here.
const claimValue = fc.oneof(
  fc.constantFrom(...owners, "", "__proto__", "constructor"),
  fc.constant(null),
  fc.integer(),
  fc.array(fc.constantFrom("admin", "user", "__proto__"), { maxLength: 3 }),
  fc.constant({ admin: true }),
);
const jwtArb = fc.option(
  fc
    .dictionary(
      fc.constantFrom("sub", "roles", "__proto__", "constructor", "toString"),
      claimValue,
      { maxKeys: 4 },
    )
    .map((d) => JSON.parse(JSON.stringify(d)) as Record<string, unknown>),
  { nil: undefined },
);
const contextArb = fc.record({
  tenant: fc.option(fc.constantFrom("t1", "t2", "__proto__"), {
    nil: undefined,
  }),
  inherited: fc.boolean(),
});

const whereArb: fc.Arbitrary<Where> = fc.letrec<{ where: Where }>((tie) => ({
  where: fc.oneof(
    { depthSize: "small", withCrossShrink: true },
    fc.record({
      level: fc.oneof(
        fc.record({ eq: fc.integer({ min: 0, max: 9 }) }),
        fc.record({ gt: fc.integer({ min: -1, max: 9 }) }),
        fc.record({ lt: fc.integer({ min: 0, max: 10 }) }),
      ),
    }),
    fc.record({
      owner: fc.oneof(
        fc.record({ eq: fc.constantFrom(...owners, "z") }),
        fc.record({
          in: fc.array(fc.constantFrom(...owners, "z"), { maxLength: 3 }),
        }),
      ),
    }),
    fc.record({ AND: fc.array(tie("where"), { maxLength: 3 }) }),
    fc.record({ OR: fc.array(tie("where"), { maxLength: 3 }) }),
    fc.record({ NOT: tie("where") }),
  ),
})).where;

// ---------------------------------------------------------------------------
// Reference evaluator
// ---------------------------------------------------------------------------

const own = (o: unknown, k: string): unknown =>
  o !== null && typeof o === "object" && Object.hasOwn(o, k)
    ? (o as Record<string, unknown>)[k]
    : undefined;

function visible(
  doc: DocRow,
  jwt: Record<string, unknown> | undefined,
  context: Record<string, unknown>,
): boolean {
  if (!jwt) return false;
  const tenant = own(context, "tenant");
  if (tenant === undefined || tenant === null) return false;
  if (doc.tenant !== tenant) return false;
  const sub = own(jwt, "sub");
  const bySub = sub !== undefined && sub !== null && doc.owner === sub;
  const roles = own(jwt, "roles");
  const byRole = Array.isArray(roles) && roles.some((r) => r === "admin");
  return bySub || byRole;
}

/**
 * A filter left with no test (`AND: []`, and AND / OR / NOT of only such)
 * is left out wherever it stands, not read as TRUE: a NOT over it or an OR
 * branch of it adds nothing. A literal `OR: []` still matches nothing.
 */
function isEmpty(w: Where): boolean {
  if ("AND" in w) return w.AND.every(isEmpty);
  if ("OR" in w) return w.OR.length > 0 && w.OR.every(isEmpty);
  if ("NOT" in w) return isEmpty(w.NOT);
  return false;
}

function matches(doc: DocRow, w: Where): boolean {
  if ("AND" in w) return w.AND.every((x) => isEmpty(x) || matches(doc, x));
  if ("OR" in w) {
    if (w.OR.length === 0) return false;
    const branches = w.OR.filter((x) => !isEmpty(x));
    return branches.length === 0 || branches.some((x) => matches(doc, x));
  }
  if ("NOT" in w) return isEmpty(w.NOT) || !matches(doc, w.NOT);
  if ("level" in w) {
    const f = w.level;
    if (f.eq !== undefined) return doc.level === f.eq;
    if (f.gt !== undefined) return doc.level > f.gt;
    return doc.level < f.lt!;
  }
  const f = w.owner;
  if (f.eq !== undefined) return doc.owner === f.eq;
  return f.in!.includes(doc.owner);
}

// ---------------------------------------------------------------------------

async function build(graph: {
  docs: DocRow[];
  links: Array<[number, number]>;
}) {
  const db = await createDatabase();
  const lora = new LoraGraphQL({ typeDefs, driver: loraDriver(db) });
  await lora.assertSchema({ create: true });
  if (graph.docs.length > 0) {
    await db.execute(
      "UNWIND $docs AS d CREATE (:Doc {key: d.key, tenant: d.tenant, owner: d.owner, level: d.level})",
      {
        docs: graph.docs,
      } as never,
    );
    await db.execute(
      "UNWIND $links AS l MATCH (a:Doc) WHERE a.key = l[0] MATCH (b:Doc) WHERE b.key = l[1] CREATE (a)-[:LINKS]->(b)",
      { links: graph.links.map(([a, b]) => [`d${a}`, `d${b}`]) } as never,
    );
  }
  return lora.getSchema();
}

async function query(
  schema: GraphQLSchema,
  source: string,
  contextValue: object,
  variableValues: Record<string, unknown> = {},
) {
  return graphql({ schema, source, contextValue, variableValues });
}

const keysOf = (rows: Array<{ key: string }>) => rows.map((r) => r.key).sort();

test("reads return exactly the rows the rules allow", async () => {
  await fc.assert(
    fc.asyncProperty(
      graphArb,
      jwtArb,
      contextArb,
      fc.option(whereArb, { nil: undefined }),
      async (graph, jwt, ctx, where) => {
        const schema = await build(graph);
        const context: Record<string, unknown> = ctx.inherited
          ? Object.create(
              ctx.tenant === undefined ? null : { tenant: ctx.tenant },
            )
          : ctx.tenant === undefined
            ? {}
            : { tenant: ctx.tenant };
        if (jwt !== undefined) context["jwt"] = jwt;
        const allowed = graph.docs.filter((d) => visible(d, jwt, context));
        const expected = allowed
          .filter((d) => where === undefined || matches(d, where))
          .map((d) => d.key)
          .sort();
        const allowedKeys = new Set(allowed.map((d) => d.key));

        const r = await query(
          schema,
          `query($w: DocWhere) {
            docs(where: $w, limit: 100) { key links(limit: 100) { key } }
            docsConnection(where: $w, first: 100) { totalCount edges { node { key } } }
            docsAggregate(where: $w) { count }
          }`,
          context,
          { w: where ?? null },
        );
        expect(r.errors).toBeUndefined();
        const data = r.data as {
          docs: Array<{ key: string; links: Array<{ key: string }> }>;
          docsConnection: {
            totalCount: number;
            edges: Array<{ node: { key: string } }>;
          };
          docsAggregate: { count: number };
        };
        expect(keysOf(data.docs)).toEqual(expected);
        expect(keysOf(data.docsConnection.edges.map((e) => e.node))).toEqual(
          expected,
        );
        expect(data.docsConnection.totalCount).toBe(expected.length);
        expect(data.docsAggregate.count).toBe(expected.length);
        for (const doc of data.docs) {
          const from = Number(doc.key.slice(1));
          // One entry per relationship: parallel edges repeat the node.
          const neighbours = graph.links
            .filter(([a]) => a === from)
            .map(([, b]) => `d${b}`);
          expect(keysOf(doc.links)).toEqual(
            neighbours.filter((k) => allowedKeys.has(k)).sort(),
          );
        }
      },
    ),
    { numRuns: 150 },
  );
}, 120_000);

test("forged cursors and global ids never reach a hidden node", async () => {
  await fc.assert(
    fc.asyncProperty(
      graphArb,
      jwtArb,
      fc.oneof(
        fc.string(),
        fc
          .array(fc.oneof(fc.integer(), fc.string(), fc.constant(null)), {
            maxLength: 3,
          })
          .map((v) => encodeCursor("level:ASC,key:ASC", v)),
        fc.string().map((s) => toBase64Url(s)),
      ),
      fc.oneof(
        fc.string(),
        fc.nat({ max: 15 }).map((i) => toGlobalId("Doc", `d${i}`)),
        fc.string().map((s) => toBase64Url(s)),
      ),
      async (graph, jwt, cursor, id) => {
        const schema = await build(graph);
        const context: Record<string, unknown> = { tenant: "t1" };
        if (jwt !== undefined) context["jwt"] = jwt;
        const allowed = new Set(
          graph.docs.filter((d) => visible(d, jwt, context)).map((d) => d.key),
        );
        const r = await query(
          schema,
          `query($after: String, $id: ID!) {
            docsConnection(first: 100, after: $after, sort: [{ level: ASC }]) { edges { node { key } } }
            node(id: $id) { ... on Doc { key } }
          }`,
          context,
          { after: cursor, id },
        );
        const data = r.data as {
          docsConnection: { edges: Array<{ node: { key: string } }> } | null;
          node: { key: string } | null;
        } | null;
        for (const e of data?.docsConnection?.edges ?? []) {
          expect(allowed.has(e.node.key)).toBe(true);
        }
        if (data?.node) expect(allowed.has(data.node.key)).toBe(true);
        for (const error of r.errors ?? []) {
          expect(["INVALID_CURSOR", "BAD_USER_INPUT", undefined]).toContain(
            error.extensions?.["code"],
          );
        }
      },
    ),
    { numRuns: 150 },
  );
}, 120_000);
