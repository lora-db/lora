// The Query fields: list, connection, single, aggregate and grouped roots
// per readable @node type, search over its indexes, Relay's node / nodes,
// interface and union roots, and top-level @cypher queries.

import {
  GraphQLFloat,
  GraphQLID,
  GraphQLInt,
  GraphQLObjectType,
  GraphQLString,
  type GraphQLFieldConfigMap,
  type GraphQLFieldResolver,
  type GraphQLInputType,
} from "graphql";
import { encodeCursor } from "../compile/cursor.js";
import type { RootKind } from "../compile/read/types.js";
import type { SearchResult } from "../compile/read/search.js";
import { requestError } from "../errors.js";
import { MAX_LIMIT } from "../model/build/limits.js";
import type { GraphModel, NodeType } from "../model/types.js";
import type { AggregateTypes } from "./aggregates.js";
import { listOf, nonNull, type BaseTypes } from "./base.js";
import type { Filters } from "./filters.js";
import { assertReadable } from "./guard.js";
import type { SchemaHooks } from "./hooks.js";
import { names } from "./names.js";
import type { ObjectTypes } from "./objects.js";
import type { SortedEdge } from "./resolvers.js";

export function buildQuery(
  model: GraphModel,
  hooks: SchemaHooks,
  base: BaseTypes,
  filters: Filters,
  objectTypes: ObjectTypes,
  aggregateTypes: AggregateTypes,
) {
  const { resolveRoot, resolveNode } = hooks;
  const { pageInfo, nodeInterface, scalarType } = base;
  const { whereOf } = filters;
  const {
    objects,
    connections,
    abstractTypes,
    connectionResolvers,
    listArgs,
    connectionArgs,
    cypherOutput,
    cypherArgs,
  } = objectTypes;
  const { aggregates, groups } = aggregateTypes;

  // --- Query -------------------------------------------------------------------

  const query: GraphQLFieldConfigMap<unknown, unknown> = {};
  const root =
    (kind: RootKind, node: NodeType): GraphQLFieldResolver<unknown, unknown> =>
    (_src, _args, context, info) =>
      resolveRoot(kind, node, info, context);

  for (const node of model.nodes.values()) {
    if (!node.read) continue;
    const obj = objects.get(node.name)!;
    query[names.listRoot(node)] = {
      type: nonNull(listOf(nonNull(obj))),
      description: `${node.name} nodes, at most \`limit\` (default ${node.limit.default}, max ${node.limit.max}).`,
      args: listArgs(node.name),
      resolve: root("list", node),
    };
    query[names.connectionRoot(node)] = {
      type: nonNull(connections.get(node.name)!),
      description: `${node.name} nodes, paginated with keyset cursors.`,
      args: connectionArgs(node.name),
      resolve: root("connection", node),
    };
    query[names.singleRoot(node)] = {
      type: obj,
      description: `The ${node.name} with this ${node.key.name}, or null.`,
      args: {
        [node.key.name]: {
          type: nonNull(scalarType(node.key) as GraphQLInputType),
        },
      },
      resolve: root("single", node),
    };
    if (node.aggregate) {
      query[names.aggregateRoot(node)] = {
        type: nonNull(aggregates.get(node.name)!),
        args: { where: { type: whereOf(node.name) } },
        resolve: root("aggregate", node),
      };
      const grouped = groups.get(node.name);
      if (grouped) {
        query[names.groupedRoot(node)] = {
          type: nonNull(listOf(nonNull(grouped.group))),
          description: `${node.name} aggregates per group of values, ordered by them; at most \`limit\` groups.`,
          args: {
            by: { type: nonNull(listOf(nonNull(grouped.field))) },
            where: { type: whereOf(node.name) },
            limit: { type: GraphQLInt },
          },
          resolve: root("grouped", node),
        };
      }
    }
  }
  for (const node of model.nodes.values()) {
    if (!node.read || node.search.length === 0) continue;
    const obj = objects.get(node.name)!;
    const match = new GraphQLObjectType<SearchResult>({
      name: names.match(node.name),
      description: `A ${node.name} and how well it matched, highest first.`,
      fields: {
        score: { type: nonNull(GraphQLFloat) },
        node: {
          type: nonNull(obj),
          resolve: (src) => assertReadable(src.node),
        },
      },
    });
    const searchEdge = new GraphQLObjectType<SortedEdge & { score: number }>({
      name: names.searchEdge(node.name),
      fields: {
        cursor: {
          type: nonNull(GraphQLString),
          resolve: (src) =>
            encodeCursor(src.__sort, src.__cursor, model.cursorSecret),
        },
        score: { type: nonNull(GraphQLFloat) },
        node: {
          type: nonNull(obj),
          resolve: (src) => assertReadable(src.node),
        },
      },
    });
    const searchConnection = new GraphQLObjectType({
      name: names.searchConnection(node.name),
      description: `${node.name} search results, highest score first, paged with cursors.`,
      fields: {
        edges: {
          type: nonNull(listOf(nonNull(searchEdge))),
          resolve: connectionResolvers.edges,
        },
        pageInfo: {
          type: nonNull(pageInfo),
          resolve: connectionResolvers.pageInfo,
        },
        totalCount: {
          type: nonNull(GraphQLInt),
          description:
            "Every match after `where` and the read rules (a vector search counts within its candidate window).",
          resolve: connectionResolvers.totalCount,
        },
      },
    });
    for (const index of node.search) {
      const common = {
        where: { type: whereOf(node.name) },
        limit: { type: GraphQLInt },
      };
      const inputs =
        index.kind === "fulltext"
          ? { query: { type: nonNull(GraphQLString) } }
          : {
              vector: { type: listOf(nonNull(GraphQLFloat)) },
              to: { type: scalarType(node.key) as GraphQLInputType },
            };
      query[`${index.queryName}Connection`] = {
        type: nonNull(searchConnection),
        description:
          index.kind === "fulltext"
            ? `${index.queryName}, paged with cursors.`
            : `${index.queryName}, paged with cursors within the top ${Math.max(node.limit.max * 4, 1)} candidates.`,
        args: {
          ...inputs,
          where: { type: whereOf(node.name) },
          first: { type: GraphQLInt },
          after: { type: GraphQLString },
        },
        resolve: (_src, _args, context, info) =>
          hooks.resolveSearch(node, index, info, context, true),
      };
      query[index.queryName] = {
        type: nonNull(listOf(nonNull(match))),
        description:
          index.kind === "fulltext"
            ? `Full-text search over ${index.fields.map((f) => f.name).join(", ")}. Terms are ANDed; a trailing * matches a prefix.`
            : `${node.name} nodes most similar to \`vector\`, or to the ${node.name} with ${node.key.name} \`to\` (${index.similarity.toLowerCase()}).`,
        args:
          index.kind === "fulltext"
            ? { query: { type: nonNull(GraphQLString) }, ...common }
            : {
                vector: { type: listOf(nonNull(GraphQLFloat)) },
                to: { type: scalarType(node.key) as GraphQLInputType },
                ...common,
              },
        resolve: (_src, _args, context, info) =>
          hooks.resolveSearch(node, index, info, context, false),
      };
    }
  }

  if (nodeInterface) {
    query["node"] = {
      type: nodeInterface,
      description: "Fetch any object by its global id.",
      args: { id: { type: nonNull(GraphQLID) } },
      resolve: (_src, args, context, info) =>
        resolveNode((args as { id: string }).id, info, context),
    };
    const maxIds = model.maxLimit ?? MAX_LIMIT;
    query["nodes"] = {
      type: nonNull(listOf(nodeInterface)),
      description: `Fetch objects by their global ids, at most ${maxIds}: one entry per id, in order, null where an id is unknown or not readable.`,
      args: { ids: { type: nonNull(listOf(nonNull(GraphQLID))) } },
      resolve: (_src, args, context, info) => {
        const ids = (args as { ids: string[] }).ids;
        if (ids.length > maxIds) {
          throw requestError(
            "BAD_USER_INPUT",
            `nodes: takes at most ${maxIds} ids, got ${ids.length}`,
          );
        }
        // Each id resolves as node(id:) does, its type's READ rules
        // included: a filtered-out node is null, and an error (a failed
        // validate rule) nulls its own entry only.
        return ids.map((id) => resolveNode(id, info, context));
      },
    };
  }

  for (const abstract of model.abstracts.values()) {
    if (!abstract.read) continue;
    query[abstract.plural] = {
      type: nonNull(listOf(nonNull(abstractTypes.get(abstract.name)!))),
      description: `${abstract.name} nodes of every ${abstract.kind === "interface" ? "implementation" : "member"}, at most \`limit\` (default ${abstract.limit.default}, max ${abstract.limit.max}).`,
      args: listArgs(abstract.name),
      resolve: (_src, _args, context, info) =>
        hooks.resolveAbstract(abstract, info, context),
    };
  }

  for (const f of model.queries) {
    query[f.name] = {
      type: cypherOutput(f),
      description: f.description,
      args: cypherArgs(f),
      resolve: (_src, _args, context, info) =>
        hooks.resolveCypher(f, info, context),
    };
  }

  return query;
}
