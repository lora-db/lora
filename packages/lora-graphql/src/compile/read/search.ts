// Full-text and vector search root fields, as lists and as connections.

import type { FieldNode, GraphQLObjectType, SelectionSetNode } from "graphql";
import { requestError } from "../../errors.js";
import type { NodeType, SearchIndex } from "../../model/types.js";
import { names } from "../../schema/names.js";
import {
  authFilter,
  authValidate,
  checkAuthentication,
  checkFieldAuthentication,
  fieldValidate,
} from "../auth.js";
import { bind, type CompileContext } from "../context.js";
import { decodeCursor } from "../cursor.js";
import {
  and,
  bin,
  fn,
  or,
  prop,
  v,
  type Clause,
  type Expr,
} from "../cypher.js";
import { labelRows, perRowCost } from "../cost.js";
import { compileNodeWhere } from "../filter.js";
import { collectFields, subSelections } from "../selection.js";
import type { CompiledRead, Args, Where, RawConnection } from "./types.js";
import {
  finish,
  deniedItem,
  assertNoneDenied,
  coalesceFalse,
  countCap,
} from "./common.js";
import {
  cursorValues,
  keysetPredicate,
  resolveLimit,
  resolveSort,
  sortItems,
  sortSignature,
  type SortKey,
} from "./sort.js";
import { projectNode } from "./project.js";
import { connectionSelections } from "./connection.js";

/** A search root field's value: nodes with their scores. */
export interface SearchResult {
  node: unknown;
  score: number;
}

/**
 * A full-text or vector search root field:
 *
 *   CALL db.index.fulltext.queryNodes($index, $query) YIELD node AS this, score
 *   WHERE <where> AND <read filter>
 *   WITH this, score ORDER BY score DESC, this.key ASC LIMIT $limit
 *
 * With `sort`, the matches are ordered by its fields, then the key, and a
 * connection pages by keyset on them; `score` is still returned.
 *
 * Vector search asks the index for more candidates than the page when a
 * filter may drop some, because the index returns the top k before WHERE.
 */
/** The order search results come in, as a cursor signature. */
const SEARCH_SORT = "score:DESC,key:ASC";

/**
 * A search as a connection (`connection: true`): pages forward by keyset
 * on (score DESC, key ASC). A vector index returns its top candidates
 * before any filter, so vector connections page within a fixed pool of
 * `4 × @limit(max:)` candidates.
 */
export function compileSearch(
  ctx: CompileContext,
  node: NodeType,
  index: SearchIndex,
  args: Args,
  fieldNodes: readonly FieldNode[],
  connection = false,
): CompiledRead {
  checkAuthentication(ctx, node, "READ");
  const label = node.labels[0]!;
  ctx.reads.labels.add(label);
  const limit = connection
    ? resolveLimit(args["first"], node.limit, "first")
    : resolveLimit(args["limit"], node.limit, "limit");
  const after = connection
    ? (args["after"] as string | null | undefined)
    : null;
  // A `sort` replaces score order: by its fields, then the key.
  const sorted = args["sort"] as unknown[] | null | undefined;
  const keys: SortKey[] | undefined =
    sorted && sorted.length > 0
      ? resolveSort(ctx, node, sorted, true)
      : undefined;
  const signature = keys ? sortSignature(keys) : SEARCH_SORT;
  const cursor =
    after != null
      ? decodeCursor(
          after,
          signature,
          keys ? keys.length : 2,
          ctx.model.cursorSecret,
        )
      : undefined;
  const keyset = !cursor
    ? undefined
    : keys
      ? keysetPredicate(ctx, "this", keys, cursor)
      : or(
          bin("<", v("score"), bind(ctx, cursor[0])),
          and(
            bin("=", v("score"), bind(ctx, cursor[0])),
            bin(">", prop(v("this"), node.key.property), bind(ctx, cursor[1])),
          ),
        );
  // The index call and its filters, for the page (with the keyset) and
  // for `totalCount` (every match, in its own parameter scope).
  const matches = (c: CompileContext, keyset: Expr | undefined): Clause[] => {
    const where = and(
      compileNodeWhere(c, node, "this", args["where"] as Where | undefined),
      authFilter(c, node, "this", "READ"),
      keyset,
    );
    const clauses: Clause[] = [];
    if (index.kind === "fulltext") {
      const query = shortPrefixesWhole(
        args["query"] as string,
        index.minPrefixLength,
      );
      clauses.push({
        kind: "procedure",
        procedure: "db.index.fulltext.queryNodes",
        args: [
          bind(c, index.name),
          bind(c, query),
          // The index ranks only its best `maxMatches`.
          ...(index.maxMatches !== undefined
            ? [
                {
                  kind: "map" as const,
                  entries: [{ key: "limit", value: bind(c, index.maxMatches) }],
                },
              ]
            : []),
        ],
        yields: [{ item: "node", alias: "this" }, { item: "score" }],
        where,
      });
      return clauses;
    }
    const vector = args["vector"] as number[] | null | undefined;
    const to = args["to"];
    if ((vector == null) === (to == null)) {
      throw requestError(
        "BAD_USER_INPUT",
        `${index.queryName} takes exactly one of \`vector\` and \`to\``,
      );
    }
    if (vector && vector.length !== index.dimensions) {
      throw requestError(
        "BAD_USER_INPUT",
        `\`vector\` has ${vector.length} dimensions; ${node.name}.${index.field.name} has ${index.dimensions}`,
      );
    }
    // Every ranked node's vector decides its score, so the vector field's
    // READ rules apply to each result as to the anchor: a node whose
    // vector the reader may not read is not ranked at all.
    checkFieldAuthentication(c, node.name, index.field);
    const readable = coalesceFalse(
      fieldValidate(c, node, index.field, "this", "READ"),
    );
    const pool = Math.max(node.limit.max * 4, 1);
    const candidates = connection
      ? pool
      : Math.min(
          (limit + (to != null ? 1 : 0)) * (where || readable ? 4 : 1),
          pool,
        );
    let source: Expr;
    let exclude: Expr | undefined;
    if (to != null) {
      clauses.push({
        kind: "match",
        pattern: { start: { variable: "anchor", labels: [label] }, hops: [] },
        // The anchor's vector is read to rank the others: it must be
        // readable, or an unreadable node's vector would leak through them.
        where: and(
          bin("=", prop(v("anchor"), node.key.property), bind(c, to)),
          authFilter(c, node, "anchor", "READ"),
          coalesceFalse(authValidate(c, node, "anchor", "READ", "BEFORE")),
          coalesceFalse(fieldValidate(c, node, index.field, "anchor", "READ")),
        ),
      });
      source = prop(v("anchor"), index.field.property);
      exclude = bin(
        "<>",
        prop(v("this"), node.key.property),
        prop(v("anchor"), node.key.property),
      );
    } else {
      source = bind(c, vector);
    }
    clauses.push({
      kind: "procedure",
      procedure: "db.index.vector.queryNodes",
      args: [bind(c, index.name), bind(c, candidates), source],
      yields: [{ item: "node", alias: "this" }, { item: "score" }],
      where: and(exclude, readable, where),
    });
    return clauses;
  };
  const sel = connection
    ? searchConnectionSelections(ctx, node, fieldNodes)
    : undefined;
  if (sel && !sel.page) {
    // Only `totalCount`: no page is read.
    return searchCount(ctx, node, index, args, matches, (rows) => ({
      __rows: [],
      __first: limit,
      __after: cursor !== undefined,
      __backward: false,
      __sort: signature,
      __totalCount: rows,
    }));
  }
  const clauses = matches(ctx, keyset);
  ctx.cost += searchCandidatesCost(ctx, node, index, args);
  const selections = sel
    ? sel.node
    : searchNodeSelections(ctx, node, fieldNodes);
  const projection = projectNode(ctx, node, "this", selections, limit);
  clauses.push(
    {
      kind: "with",
      items: [{ expr: v("this") }, { expr: v("score") }],
      orderBy: keys
        ? sortItems("this", keys)
        : [
            { expr: v("score"), direction: "DESC" },
            { expr: prop(v("this"), node.key.property), direction: "ASC" },
          ],
      // One more than the page, to tell whether another follows.
      limit: bind(ctx, connection ? limit + 1 : limit),
    },
    ...projection.pre,
    {
      kind: "return",
      items: [
        { expr: projection.expr, alias: "node" },
        { expr: v("score"), alias: "score" },
        ...(connection
          ? [
              {
                expr: keys
                  ? cursorValues("this", keys)
                  : {
                      kind: "list" as const,
                      items: [v("score"), prop(v("this"), node.key.property)],
                    },
                alias: "__cursor",
              },
            ]
          : []),
      ],
    },
  );
  if (sel) {
    const count = sel.totalCount
      ? searchCount(ctx, node, index, args, matches, () => undefined)
      : undefined;
    const compiled = finish(
      ctx,
      clauses,
      ["node", "score", "__cursor"],
      ([r, c]): RawConnection => {
        const row = c?.rows[0];
        if (c) assertNoneDenied(row);
        return {
          __rows: r!.rows.map((row) => ({
            node: row["node"],
            score: row["score"],
            __cursor: row["__cursor"] as unknown[],
          })),
          __first: limit,
          __after: cursor !== undefined,
          __backward: false,
          __sort: signature,
          ...(c ? { __totalCount: Number(row?.["totalCount"] ?? 0) } : {}),
        };
      },
      undefined,
      count?.statements ?? [],
    );
    if (count) compiled.cost += count.cost;
    return compiled;
  }
  return finish(
    ctx,
    clauses,
    ["node", "score"],
    ([r]): SearchResult[] =>
      r!.rows.map((row) => ({
        node: row["node"],
        score: row["score"] as number,
      })),
    undefined,
  );
}

/**
 * `totalCount` of a search connection: every match after \`where\` and the
 * read rules (a vector search counts within its candidate window), in its
 * own parameter scope. A row the READ rules would refuse makes the count
 * FORBIDDEN, as on other connections.
 */
function searchCount(
  ctx: CompileContext,
  node: NodeType,
  index: SearchIndex,
  args: Args,
  matches: (c: CompileContext, keyset: Expr | undefined) => Clause[],
  shape: (count: number) => RawConnection | undefined,
): CompiledRead {
  const statsCtx: CompileContext = {
    ...ctx,
    params: {},
    vars: new Set(["this"]),
    computed: new Map(),
  };
  const clauses: Clause[] = [
    ...matches(statsCtx, undefined),
    ...countCap(statsCtx, ctx.maxTotalCount),
    {
      kind: "return",
      items: [
        { expr: fn("count", v("this")), alias: "totalCount" },
        ...deniedItem(statsCtx, node, "this"),
      ],
    },
  ];
  const compiled = finish(
    statsCtx,
    clauses,
    ["totalCount"],
    ([r]) => {
      const row = r!.rows[0];
      assertNoneDenied(row);
      return shape(Number(row?.["totalCount"] ?? 0));
    },
    undefined,
  );
  compiled.cost = 1 + searchCandidatesCost(ctx, node, index, args);
  return compiled;
}

/**
 * The matches a full-text search reads before its page: the index's
 * `maxMatches`, else every node of the label (statistics; the type's page
 * size without them), each examined by the `where`. A vector search reads
 * a fixed candidate pool, counted by its page.
 */
function searchCandidatesCost(
  ctx: CompileContext,
  node: NodeType,
  index: SearchIndex,
  args: Args,
): number {
  if (index.kind !== "fulltext") return 0;
  const rows = Math.min(
    index.maxMatches ?? Infinity,
    labelRows(ctx, node, node.limit.default),
  );
  return rows * (1 + perRowCost(ctx, node, args["where"] as Where | undefined));
}

/**
 * `query` with every `*` word shorter than `min` matched whole: `lo*`
 * becomes `lo`. Words are the index analyzer's: runs of letters and
 * digits.
 */
export function shortPrefixesWhole(
  query: string,
  min: number | undefined,
): string {
  if (min === undefined || min <= 1) return query;
  return query.replace(/([\p{L}\p{N}]+)\*/gu, (whole, word: string) =>
    [...word].length < min ? word : whole,
  );
}

/** The `edges { node }` selections of a search connection. */
function searchConnectionSelections(
  ctx: CompileContext,
  node: NodeType,
  fieldNodes: readonly FieldNode[],
): ReturnType<typeof connectionSelections> {
  const connType = ctx.schema.getType(
    names.searchConnection(node.name),
  ) as GraphQLObjectType;
  const edgeType = ctx.schema.getType(
    names.searchEdge(node.name),
  ) as GraphQLObjectType;
  return connectionSelections(
    ctx,
    connType,
    edgeType,
    subSelections(fieldNodes),
    node,
    undefined,
  );
}

/** The `node` selections of a `<Type>Match` list, merged across aliases. */
function searchNodeSelections(
  ctx: CompileContext,
  node: NodeType,
  fieldNodes: readonly FieldNode[],
): SelectionSetNode[] {
  const matchType = ctx.schema.getType(
    names.match(node.name),
  ) as GraphQLObjectType;
  const out: SelectionSetNode[] = [];
  for (const [, nodes] of collectFields(
    ctx,
    matchType,
    subSelections(fieldNodes),
  )) {
    if (nodes[0]!.name.value === "node") out.push(...subSelections(nodes));
  }
  return out;
}
