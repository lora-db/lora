// GraphModel → executable GraphQLSchema. Generates only what the model
// opts into: filters for @filterable fields, sorts for @sortable ones,
// aggregates for @query(aggregate: true), mutations for @mutation.

import {
  GraphQLBoolean,
  GraphQLEnumType,
  GraphQLFloat,
  GraphQLID,
  GraphQLInputObjectType,
  GraphQLInt,
  GraphQLInterfaceType,
  GraphQLList,
  GraphQLNonNull,
  GraphQLObjectType,
  GraphQLSchema,
  GraphQLString,
  type GraphQLFieldConfig,
  type GraphQLFieldConfigArgumentMap,
  type GraphQLFieldConfigMap,
  type GraphQLFieldResolver,
  type GraphQLInputFieldConfigMap,
  type GraphQLInputType,
  type GraphQLOutputType,
  type GraphQLResolveInfo,
  type GraphQLScalarType,
} from "graphql";
import { encodeCursor } from "../compile/cursor.js";
import type { RawConnection, RawEdge, RootKind } from "../compile/read.js";
import { ModelError, requestError } from "../errors.js";
import type {
  CypherField,
  FilterOperator,
  GraphModel,
  NodeType,
  RelationshipField,
  RelationshipPropertiesType,
  ScalarField,
  ScalarType,
} from "../model/types.js";
import { toGlobalId } from "./global-id.js";
import { assertReadable } from "./guard.js";
import { buildMutations, type MutationResolver } from "./mutations.js";
import { hasOwnConnection, names } from "./names.js";
import { CUSTOM_SCALARS } from "./scalars.js";

export interface SchemaHooks {
  resolveRoot: RootResolver;
  resolveNode: NodeResolver;
  resolveCypher: CypherRootResolver;
  resolveMutation: MutationResolver;
}

export type CypherRootResolver = (
  field: CypherField,
  info: GraphQLResolveInfo,
  context: unknown,
) => Promise<unknown>;

export type RootResolver = (
  kind: RootKind,
  node: NodeType,
  info: GraphQLResolveInfo,
  context: unknown,
) => Promise<unknown>;

export type NodeResolver = (
  id: string,
  info: GraphQLResolveInfo,
  context: unknown,
) => Promise<unknown>;

type SortedEdge = RawEdge & { __sort: string };

const nonNull = <T extends GraphQLOutputType | GraphQLInputType>(t: T) =>
  new GraphQLNonNull(t);
const listOf = <T extends GraphQLOutputType | GraphQLInputType>(t: T) =>
  new GraphQLList(t);

/** Values under response keys: the compiler projects each alias. */
const byResponseKey: GraphQLFieldResolver<Record<string, unknown>, unknown> = (
  source,
  _args,
  _ctx,
  info,
) => source[info.path.key as string];

/** Same, for nodes: fails on nodes a READ validate rule rejects. */
const nodesByResponseKey: GraphQLFieldResolver<
  Record<string, unknown>,
  unknown
> = (source, _args, _ctx, info) =>
  assertReadable(source[info.path.key as string]);

const OPERATOR_FIELDS: Record<FilterOperator, string> = {
  EQ: "eq",
  IN: "in",
  LT: "lt",
  LTE: "lte",
  GT: "gt",
  GTE: "gte",
  CONTAINS: "contains",
  STARTS_WITH: "startsWith",
  ENDS_WITH: "endsWith",
  WITHIN_BBOX: "withinBBox",
  DISTANCE: "distance",
};

const OPERATOR_DOCS: Record<FilterOperator, string> = {
  EQ: "Equal to.",
  IN: "Equal to any of.",
  LT: "Less than.",
  LTE: "Less than or equal to.",
  GT: "Greater than.",
  GTE: "Greater than or equal to.",
  CONTAINS: "Contains the substring.",
  STARTS_WITH: "Starts with.",
  ENDS_WITH: "Ends with.",
  WITHIN_BBOX: "Inside the bounding box, edges included.",
  DISTANCE: "Within this distance of a point (metres for geographic points).",
};

export function buildSchema(
  model: GraphModel,
  hooks: SchemaHooks,
): GraphQLSchema {
  const { resolveRoot, resolveNode } = hooks;
  const sortDirection = new GraphQLEnumType({
    name: "SortDirection",
    values: {
      ASC: { description: "Ascending; nulls last." },
      DESC: { description: "Descending; nulls first." },
    },
  });

  const pageInfo = new GraphQLObjectType({
    name: "PageInfo",
    fields: {
      hasNextPage: { type: nonNull(GraphQLBoolean) },
      hasPreviousPage: { type: nonNull(GraphQLBoolean) },
      startCursor: { type: GraphQLString },
      endCursor: { type: GraphQLString },
    },
  });

  const countFilter = new GraphQLInputObjectType({
    name: "CountFilter",
    description: "Compare the number of related nodes.",
    fields: {
      eq: { type: GraphQLInt },
      lt: { type: GraphQLInt },
      lte: { type: GraphQLInt },
      gt: { type: GraphQLInt },
      gte: { type: GraphQLInt },
    },
  });

  const point = new GraphQLObjectType({
    name: "Point",
    description: "A WGS-84 geographic point.",
    fields: {
      longitude: { type: nonNull(GraphQLFloat) },
      latitude: { type: nonNull(GraphQLFloat) },
      height: { type: GraphQLFloat },
      srid: { type: nonNull(GraphQLInt) },
      crs: { type: nonNull(GraphQLString) },
    },
  });

  const cartesianPoint = new GraphQLObjectType({
    name: "CartesianPoint",
    description: "A point in a cartesian coordinate system.",
    fields: {
      x: { type: nonNull(GraphQLFloat) },
      y: { type: nonNull(GraphQLFloat) },
      z: { type: GraphQLFloat },
      srid: { type: nonNull(GraphQLInt) },
      crs: { type: nonNull(GraphQLString) },
    },
  });

  const enums = new Map<string, GraphQLEnumType>();
  for (const e of model.enums.values()) {
    enums.set(
      e.name,
      new GraphQLEnumType({
        name: e.name,
        description: e.description,
        values: Object.fromEntries(
          e.values.map((val) => [val.name, { description: val.description }]),
        ),
      }),
    );
  }

  const relay = [...model.nodes.values()].some((n) => n.key.relayId);
  const nodeInterface = relay
    ? new GraphQLInterfaceType({
        name: "Node",
        description: "An object with a global id.",
        fields: { id: { type: nonNull(GraphQLID) } },
        resolveType: (value) =>
          (value as { __typename?: string }).__typename ?? undefined,
      })
    : undefined;

  const scalarType = (
    f: ScalarField,
  ): GraphQLScalarType | GraphQLEnumType | GraphQLObjectType =>
    baseType(f.type, f.enumName);
  const baseType = (
    type: ScalarType,
    enumName: string | undefined,
  ): GraphQLScalarType | GraphQLEnumType | GraphQLObjectType => {
    switch (type) {
      case "String":
        return GraphQLString;
      case "ID":
        return GraphQLID;
      case "Int":
        return GraphQLInt;
      case "Float":
        return GraphQLFloat;
      case "Boolean":
        return GraphQLBoolean;
      case "Point":
        return point;
      case "CartesianPoint":
        return cartesianPoint;
      case "Enum":
        return enums.get(enumName!)!;
      default:
        return CUSTOM_SCALARS[type]!;
    }
  };

  const scalarOutput = (f: ScalarField): GraphQLOutputType => {
    const base = scalarType(f);
    const t: GraphQLOutputType = f.list ? listOf(nonNull(base)) : base;
    return f.required ? nonNull(t) : t;
  };

  // --- Filters -------------------------------------------------------------

  // --- Points ---------------------------------------------------------------

  const pointInput = new GraphQLInputObjectType({
    name: "PointInput",
    description: "A WGS-84 point. Add height for 3D.",
    fields: {
      longitude: { type: nonNull(GraphQLFloat) },
      latitude: { type: nonNull(GraphQLFloat) },
      height: { type: GraphQLFloat },
    },
  });
  const cartesianPointInput = new GraphQLInputObjectType({
    name: "CartesianPointInput",
    description: "A cartesian point. Add z for 3D.",
    fields: {
      x: { type: nonNull(GraphQLFloat) },
      y: { type: nonNull(GraphQLFloat) },
      z: { type: GraphQLFloat },
    },
  });
  const spatialInputs = (cartesian: boolean) => {
    const point = cartesian ? cartesianPointInput : pointInput;
    const prefix = cartesian ? "CartesianPoint" : "Point";
    return {
      bbox: new GraphQLInputObjectType({
        name: `${prefix}BBoxInput`,
        fields: {
          lowerLeft: {
            type: nonNull(point),
            description: "Minimum corner (south-west).",
          },
          upperRight: {
            type: nonNull(point),
            description: "Maximum corner (north-east).",
          },
        },
      }),
      distance: new GraphQLInputObjectType({
        name: `${prefix}DistanceInput`,
        fields: {
          from: { type: nonNull(point) },
          lte: { type: nonNull(GraphQLFloat) },
        },
      }),
    };
  };
  const spatial = { geo: spatialInputs(false), cartesian: spatialInputs(true) };

  /** The input form of a stored field: points take PointInput. */
  const inputTypeOf = (f: ScalarField): GraphQLInputType =>
    f.type === "Point"
      ? pointInput
      : f.type === "CartesianPoint"
        ? cartesianPointInput
        : (scalarType(f) as GraphQLInputType);

  const fieldFilter = (owner: string, f: ScalarField) => {
    const base = inputTypeOf(f);
    const fields: GraphQLInputFieldConfigMap = {};
    const kinds = f.type === "CartesianPoint" ? spatial.cartesian : spatial.geo;
    for (const op of f.filters) {
      const name = OPERATOR_FIELDS[op];
      fields[name] = {
        type:
          op === "IN"
            ? listOf(nonNull(base))
            : op === "WITHIN_BBOX"
              ? kinds.bbox
              : op === "DISTANCE"
                ? kinds.distance
                : base,
        description: OPERATOR_DOCS[op],
      };
    }
    return new GraphQLInputObjectType({
      name: names.fieldFilter(owner, f.name),
      fields,
    });
  };

  const wheres = new Map<string, GraphQLInputObjectType>();
  const whereOf = (typeName: string) => wheres.get(typeName)!;

  for (const node of model.nodes.values()) {
    const where: GraphQLInputObjectType = new GraphQLInputObjectType({
      name: names.where(node.name),
      description: `Filters ${node.name} nodes. Absent or null filters are ignored.`,
      fields: () => {
        const fields: GraphQLInputFieldConfigMap = {
          AND: { type: listOf(nonNull(where)) },
          OR: { type: listOf(nonNull(where)) },
          NOT: { type: where },
        };
        for (const f of node.fields.values()) {
          if (f.kind === "scalar") {
            if (f.filters.size > 0 && !f.private) {
              fields[f.name] = { type: fieldFilter(node.name, f) };
            }
          } else if (f.kind === "relationship" && f.filterable) {
            fields[f.name] = {
              type: f.list ? relationFilter(node, f) : whereOf(f.target),
              description: f.list
                ? undefined
                : `Has a related ${f.target} matching the filter.`,
            };
          }
        }
        return fields;
      },
    });
    wheres.set(node.name, where);
  }

  const relationFilter = (node: NodeType, rel: RelationshipField) =>
    new GraphQLInputObjectType({
      name: names.relationFilter(node.name, rel.name),
      fields: {
        some: {
          type: whereOf(rel.target),
          description: "At least one related node matches.",
        },
        all: {
          type: whereOf(rel.target),
          description: "Every related node matches.",
        },
        none: {
          type: whereOf(rel.target),
          description: "No related node matches.",
        },
        single: {
          type: whereOf(rel.target),
          description: "Exactly one related node matches.",
        },
        count: {
          type: countFilter,
          description: "The number of related nodes.",
        },
      },
    });

  const propsWheres = new Map<string, GraphQLInputObjectType | undefined>();
  const propsWhere = (props: RelationshipPropertiesType) => {
    if (propsWheres.has(props.name)) return propsWheres.get(props.name);
    const filterable = [...props.fields.values()].filter(
      (f) => f.filters.size > 0 && !f.private,
    );
    let t: GraphQLInputObjectType | undefined;
    if (filterable.length > 0) {
      t = new GraphQLInputObjectType({
        name: names.where(props.name),
        fields: () => ({
          AND: { type: listOf(nonNull(t!)) },
          OR: { type: listOf(nonNull(t!)) },
          NOT: { type: t! },
          ...Object.fromEntries(
            filterable.map((f) => [
              f.name,
              { type: fieldFilter(props.name, f) },
            ]),
          ),
        }),
      });
    }
    propsWheres.set(props.name, t);
    return t;
  };

  // --- Sorts -----------------------------------------------------------------

  const sorts = new Map<string, GraphQLInputObjectType>();
  for (const node of model.nodes.values()) {
    const fields: GraphQLInputFieldConfigMap = {};
    for (const f of node.fields.values()) {
      if (f.kind === "scalar" && (f.key || f.sortable)) {
        fields[f.name] = { type: sortDirection };
      }
    }
    sorts.set(
      node.name,
      new GraphQLInputObjectType({
        name: names.sort(node.name),
        description: `Sorts ${node.name} nodes. One field per item; the @key breaks ties.`,
        fields,
      }),
    );
  }

  // --- Objects and connections ----------------------------------------------

  const objects = new Map<string, GraphQLObjectType>();
  const connections = new Map<string, GraphQLObjectType>();

  const connectionResolvers = {
    edges: (src: RawConnection) =>
      src.__rows.slice(0, src.__first).map((row) => {
        assertReadable(row.node);
        return { ...row, __sort: src.__sort };
      }),
    pageInfo: (src: RawConnection) => {
      const page = src.__rows.slice(0, src.__first);
      // Cursors carry sort values: as readable as the nodes themselves.
      for (const row of page) assertReadable(row.node);
      const cursor = (row: RawEdge | undefined) =>
        row ? encodeCursor(src.__sort, row.__cursor) : null;
      return {
        hasNextPage: src.__rows.length > src.__first,
        hasPreviousPage: src.__after,
        startCursor: cursor(page[0]),
        endCursor: cursor(page[page.length - 1]),
      };
    },
    totalCount: (src: RawConnection & { __denied?: number }) => {
      if ((src.__denied ?? 0) > 0) {
        throw requestError(
          "FORBIDDEN",
          "not allowed to read some of these nodes",
        );
      }
      return src.__totalCount ?? 0;
    },
  };

  const makeConnection = (
    name: string,
    edgeName: string,
    nodeType: () => GraphQLObjectType,
    propertiesType?: () => GraphQLObjectType,
  ) => {
    const edge = new GraphQLObjectType<SortedEdge>({
      name: edgeName,
      fields: () => ({
        cursor: {
          type: nonNull(GraphQLString),
          resolve: (src) => encodeCursor(src.__sort, src.__cursor),
        },
        node: {
          type: nonNull(nodeType()),
          resolve: (src: SortedEdge) => src.node,
        },
        ...(propertiesType
          ? {
              properties: {
                type: nonNull(propertiesType()),
                resolve: (src: SortedEdge) => src.properties,
              },
            }
          : {}),
      }),
    });
    return new GraphQLObjectType({
      name,
      fields: () => ({
        edges: {
          type: nonNull(listOf(nonNull(edge))),
          resolve: connectionResolvers.edges,
        },
        pageInfo: {
          type: nonNull(pageInfo),
          resolve: connectionResolvers.pageInfo,
        },
        totalCount: {
          type: nonNull(GraphQLInt),
          resolve: connectionResolvers.totalCount,
        },
      }),
    });
  };

  const propsObjects = new Map<string, GraphQLObjectType>();
  const propsObject = (props: RelationshipPropertiesType) => {
    let t = propsObjects.get(props.name);
    if (!t) {
      t = new GraphQLObjectType({
        name: props.name,
        description: props.description,
        fields: () =>
          Object.fromEntries(
            [...props.fields.values()]
              .filter((f) => !f.private)
              .map((f) => [
                f.name,
                {
                  type: scalarOutput(f),
                  description: f.description,
                  resolve: byResponseKey,
                },
              ]),
          ),
      });
      propsObjects.set(props.name, t);
    }
    return t;
  };

  const listArgs = (target: string): GraphQLFieldConfigArgumentMap => ({
    where: { type: whereOf(target) },
    sort: { type: listOf(nonNull(sorts.get(target)!)) },
    limit: { type: GraphQLInt },
  });

  const connectionArgs = (
    target: string,
    where: GraphQLInputType = whereOf(target),
  ): GraphQLFieldConfigArgumentMap => ({
    where: { type: where },
    sort: { type: listOf(nonNull(sorts.get(target)!)) },
    first: { type: GraphQLInt },
    after: { type: GraphQLString },
  });

  for (const node of model.nodes.values()) {
    const obj: GraphQLObjectType = new GraphQLObjectType({
      name: node.name,
      description: node.description,
      interfaces: node.key.relayId && nodeInterface ? [nodeInterface] : [],
      fields: () => {
        const fields: GraphQLFieldConfigMap<
          Record<string, unknown>,
          unknown
        > = {};
        if (node.key.relayId) {
          fields["id"] = {
            type: nonNull(GraphQLID),
            description: "Global id.",
            resolve: (src, _a, _c, info) =>
              toGlobalId(node.name, src[info.path.key as string]),
          };
        }
        for (const f of node.fields.values()) {
          if (f.kind === "scalar") {
            if (f.private) continue;
            fields[f.name] = {
              type: scalarOutput(f),
              description: f.description,
              resolve: byResponseKey,
            };
            continue;
          }
          if (f.kind === "cypher") {
            fields[f.name] = {
              type: cypherOutput(f),
              description: f.description,
              args: cypherArgs(f),
              resolve: f.node ? nodesByResponseKey : byResponseKey,
            };
            continue;
          }
          const target = objects.get(f.target)!;
          if (!f.list) {
            fields[f.name] = {
              type: f.required ? nonNull(target) : target,
              description: f.description,
              args: { where: { type: whereOf(f.target) } },
              resolve: nodesByResponseKey,
            };
            continue;
          }
          fields[f.name] = {
            type: nonNull(listOf(nonNull(target))),
            description: f.description,
            args: listArgs(f.target),
            resolve: nodesByResponseKey,
          };
          fields[names.connectionField(f.name)] = relationshipConnectionField(
            node,
            f,
          );
        }
        return fields;
      },
    });
    objects.set(node.name, obj);
    connections.set(
      node.name,
      makeConnection(
        names.connection(node.name),
        names.edge(node.name),
        () => obj,
      ),
    );
  }

  function relationshipConnectionField(
    node: NodeType,
    rel: RelationshipField,
  ): GraphQLFieldConfig<Record<string, unknown>, unknown> {
    if (!hasOwnConnection(rel)) {
      return {
        type: nonNull(connections.get(rel.target)!),
        args: connectionArgs(rel.target),
        resolve: byResponseKey,
      };
    }
    const props = model.relationshipProperties.get(rel.properties!)!;
    const edgeWhere = propsWhere(props);
    const where = new GraphQLInputObjectType({
      name: names.relConnectionWhere(node.name, rel.name),
      fields: {
        node: { type: whereOf(rel.target) },
        ...(edgeWhere ? { edge: { type: edgeWhere } } : {}),
      },
    });
    const conn = makeConnection(
      names.relConnection(node.name, rel.name),
      names.relEdge(node.name, rel.name),
      () => objects.get(rel.target)!,
      () => propsObject(props),
    );
    return {
      type: nonNull(conn),
      args: connectionArgs(rel.target, where),
      resolve: byResponseKey,
    };
  }

  // --- @cypher -----------------------------------------------------------------

  const namedType = (named: string) =>
    objects.get(named) ??
    enums.get(named) ??
    baseType(named as ScalarType, undefined);

  function cypherOutput(f: CypherField): GraphQLOutputType {
    const base = namedType(f.type.named) as GraphQLOutputType;
    const item = f.type.list && f.type.itemRequired ? nonNull(base) : base;
    const t: GraphQLOutputType = f.type.list ? listOf(item) : item;
    return f.type.required ? nonNull(t) : t;
  }

  function cypherArgs(f: CypherField): GraphQLFieldConfigArgumentMap {
    const out: GraphQLFieldConfigArgumentMap = {};
    for (const a of f.args) {
      const base = namedType(a.type.named) as GraphQLInputType;
      const item = a.type.list && a.type.itemRequired ? nonNull(base) : base;
      const t: GraphQLInputType = a.type.list ? listOf(item) : item;
      out[a.name] = {
        type: a.type.required ? nonNull(t) : t,
        defaultValue: a.defaultValue,
        description: a.description,
      };
    }
    return out;
  }

  // --- Aggregates -------------------------------------------------------------

  const aggregateFieldTypes = new Map<string, GraphQLObjectType>();
  const aggregateFieldType = (f: ScalarField) => {
    const base = scalarType(f) as GraphQLScalarType;
    const numeric =
      f.type === "Int" || f.type === "Float" || f.type === "BigInt";
    const name = `${base.name}Aggregate`;
    let t = aggregateFieldTypes.get(name);
    if (!t) {
      t = new GraphQLObjectType({
        name,
        fields: {
          min: { type: base },
          max: { type: base },
          ...(numeric
            ? {
                avg: { type: GraphQLFloat },
                sum: { type: f.type === "Int" ? GraphQLFloat : base },
              }
            : {}),
        },
      });
      aggregateFieldTypes.set(name, t);
    }
    return t;
  };

  const aggregates = new Map<string, GraphQLObjectType>();
  for (const node of model.nodes.values()) {
    if (!node.aggregate) continue;
    const fields: GraphQLFieldConfigMap<unknown, unknown> = {
      count: { type: nonNull(GraphQLInt) },
    };
    for (const f of node.fields.values()) {
      if (
        f.kind === "scalar" &&
        (f.key || f.sortable) &&
        f.type !== "Boolean" &&
        f.type !== "Enum"
      ) {
        fields[f.name] = { type: nonNull(aggregateFieldType(f)) };
      }
    }
    aggregates.set(
      node.name,
      new GraphQLObjectType({ name: names.aggregate(node.name), fields }),
    );
  }

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

  const mutation: GraphQLFieldConfigMap<unknown, unknown> = buildMutations(
    {
      model,
      object: (name) => objects.get(name)!,
      inputType: inputTypeOf,
    },
    hooks.resolveMutation,
  );
  for (const f of model.mutations) {
    mutation[f.name] = {
      type: cypherOutput(f),
      description: f.description,
      args: cypherArgs(f),
      resolve: (_src, _args, context, info) =>
        hooks.resolveCypher(f, info, context),
    };
  }

  if (Object.keys(query).length === 0) {
    throw new ModelError([
      { message: "no readable @node types: the Query type would be empty" },
    ]);
  }

  return new GraphQLSchema({
    query: new GraphQLObjectType({ name: "Query", fields: query }),
    mutation:
      Object.keys(mutation).length > 0
        ? new GraphQLObjectType({ name: "Mutation", fields: mutation })
        : undefined,
    types: [...objects.values()],
  });
}
