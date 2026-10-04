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
  GraphQLUnionType,
  type GraphQLFieldConfig,
  type GraphQLFieldConfigArgumentMap,
  type GraphQLFieldConfigMap,
  type GraphQLFieldResolver,
  type GraphQLInputFieldConfigMap,
  type GraphQLInputType,
  type GraphQLOutputType,
  type GraphQLResolveInfo,
  GraphQLScalarType,
} from "graphql";
import { encodeCursor } from "../compile/cursor.js";
import type {
  RawConnection,
  RawEdge,
  RootKind,
  SearchResult,
} from "../compile/read.js";
import { ModelError, requestError } from "../errors.js";
import { lowerFirst, MAX_LIMIT } from "../model/build.js";
import type {
  AbstractType,
  CustomField,
  CypherField,
  FilterOperator,
  GraphModel,
  NodeType,
  RelationshipField,
  RelationshipPropertiesType,
  ScalarField,
  ScalarType,
  SearchIndex,
} from "../model/types.js";
import { toGlobalId } from "./global-id.js";
import { assertReadable } from "./guard.js";
import { buildMutations, type MutationResolver } from "./mutations.js";
import { hasOwnConnection, names } from "./names.js";
import { CUSTOM_SCALARS } from "./scalars.js";

/** One event of a generated subscription. */
export interface ChangeEvent {
  operation: "CREATE" | "UPDATE" | "DELETE" | "CONNECT" | "DISCONNECT";
  key: unknown;
  /** When the write was committed, ISO-8601. */
  timestamp?: string | undefined;
  /** For CONNECT / DISCONNECT: the relationship and its other end. */
  relationship?:
    | { field: string; type: string; relatedType: string; relatedKey: unknown }
    | undefined;
  /** Stored properties before the write (`previousState: true`). */
  previous?: Record<string, unknown> | undefined;
}

export interface SchemaHooks {
  resolveAbstract: (
    abstract: AbstractType,
    info: GraphQLResolveInfo,
    context: unknown,
  ) => Promise<unknown>;
  subscribe: (
    node: NodeType,
    args: Record<string, unknown>,
    context: unknown,
  ) => AsyncIterable<ChangeEvent>;
  resolveChangedNode: (
    node: NodeType,
    event: ChangeEvent,
    info: GraphQLResolveInfo,
    context: unknown,
  ) => Promise<unknown>;
  /**
   * A `previousState` value as the subscriber may see it: throws when
   * field-level @authentication refuses it, applies masks by the claims.
   */
  previousValue: (
    node: NodeType,
    field: ScalarField,
    value: unknown,
    context: unknown,
  ) => unknown;
  /** Implementations of custom scalars, by name (the scalars option). */
  scalars?: Readonly<Record<string, GraphQLScalarType>> | undefined;
  /** The resolver of a `@customResolver` field (from the resolvers option). */
  customResolver: (
    node: NodeType,
    field: CustomField,
  ) => GraphQLFieldResolver<Record<string, unknown>, unknown>;
  resolveSearch: (
    node: NodeType,
    index: SearchIndex,
    info: GraphQLResolveInfo,
    context: unknown,
    connection: boolean,
  ) => Promise<unknown>;
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

/** The rows of a page in the requested order. */
function pageOf(src: RawConnection): RawEdge[] {
  const page = src.__rows.slice(0, src.__first);
  return src.__backward ? page.reverse() : page;
}

const nonNull = <T extends GraphQLOutputType | GraphQLInputType>(t: T) =>
  new GraphQLNonNull(t);
const listOf = <T extends GraphQLOutputType | GraphQLInputType>(t: T) =>
  new GraphQLList(t);

/**
 * Throws for a field whose row-level READ rule failed: the compiler
 * projects it as `{ __forbidden: true }` (with `__unauthenticated` when
 * the request had no token), whatever the field's type.
 */
function assertFieldReadable(value: unknown, info: GraphQLResolveInfo): void {
  if (
    value === null ||
    typeof value !== "object" ||
    (value as { __forbidden?: unknown }).__forbidden !== true
  ) {
    return;
  }
  if ((value as { __unauthenticated?: unknown }).__unauthenticated) {
    throw requestError(
      "UNAUTHENTICATED",
      `${info.parentType.name}.${info.fieldName} needs an authenticated request`,
    );
  }
  throw requestError(
    "FORBIDDEN",
    `not allowed to read ${info.parentType.name}.${info.fieldName}`,
  );
}

/** Values under response keys: the compiler projects each alias. */
const byResponseKey: GraphQLFieldResolver<Record<string, unknown>, unknown> = (
  source,
  _args,
  _ctx,
  info,
) => {
  const value = source[info.path.key as string];
  assertFieldReadable(value, info);
  return value;
};

/** A stored VECTOR as its numbers. */
const vectorByResponseKey: GraphQLFieldResolver<
  Record<string, unknown>,
  unknown
> = (source, _args, _ctx, info) => {
  const value = source[info.path.key as string] as
    | { values?: number[] }
    | null
    | undefined;
  assertFieldReadable(value, info);
  return value && !Array.isArray(value) ? (value.values ?? null) : value;
};

/** Same, for nodes: fails on nodes a READ validate rule rejects. */
const nodesByResponseKey: GraphQLFieldResolver<
  Record<string, unknown>,
  unknown
> = (source, _args, _ctx, info) => {
  const value = source[info.path.key as string];
  assertFieldReadable(value, info);
  return assertReadable(value);
};

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
  INCLUDES: "includes",
  IS_NULL: "isNull",
  CASE_INSENSITIVE: "caseInsensitive",
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
  INCLUDES: "The list contains this value.",
  IS_NULL: "true: the value is absent; false: it is present.",
  CASE_INSENSITIVE:
    "The same string operators, ignoring case. Not index-backed: prefer @fulltext for search.",
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

  // Custom scalars: the implementation given in the scalars option, or a
  // pass-through that serializes and parses like its storage type. The
  // SDL's description wins over the implementation's, so clients see the
  // schema's documentation and the schema hash does not depend on which
  // implementation was passed.
  const customScalars = new Map<string, GraphQLScalarType>();
  const customScalar = (name: string): GraphQLScalarType | undefined => {
    const stored = model.scalars.get(name);
    if (!stored) return undefined;
    let t = customScalars.get(name);
    if (!t) {
      const documented = model.scalarDescriptions.get(name);
      const given = hooks.scalars?.[name];
      if (given) {
        t =
          documented === undefined || documented === given.description
            ? given
            : new GraphQLScalarType({
                ...given.toConfig(),
                description: documented,
              });
      } else {
        const base = baseType(stored, undefined) as GraphQLScalarType;
        t = new GraphQLScalarType({
          name,
          description: documented ?? `Stored as ${stored}.`,
          serialize: base.serialize,
          parseValue: base.parseValue,
          parseLiteral: base.parseLiteral,
        });
      }
      customScalars.set(name, t);
    }
    return t;
  };
  const scalarType = (
    f: ScalarField,
  ): GraphQLScalarType | GraphQLEnumType | GraphQLObjectType =>
    (f.customScalar && customScalar(f.customScalar)) ||
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

  // String operators, ignoring case: `toLower(x) <op> lower(value)`.
  const caseInsensitiveFilter = (owner: string, f: ScalarField) => {
    const text = new Set<FilterOperator>([
      "EQ",
      ...[...f.filters].filter((op) =>
        ["IN", "CONTAINS", "STARTS_WITH", "ENDS_WITH"].includes(op),
      ),
    ]);
    const fields: GraphQLInputFieldConfigMap = {};
    for (const op of text) {
      fields[OPERATOR_FIELDS[op]] = {
        type: op === "IN" ? listOf(nonNull(GraphQLString)) : GraphQLString,
        description: OPERATOR_DOCS[op],
      };
    }
    return new GraphQLInputObjectType({
      name: `${names.fieldFilter(owner, f.name)}CaseInsensitive`,
      fields,
    });
  };

  const fieldFilter = (owner: string, f: ScalarField) => {
    const base = inputTypeOf(f);
    const fields: GraphQLInputFieldConfigMap = {};
    const kinds = f.type === "CartesianPoint" ? spatial.cartesian : spatial.geo;
    for (const op of f.filters) {
      const name = OPERATOR_FIELDS[op];
      let type: GraphQLInputType;
      switch (op) {
        case "IN":
          type = listOf(nonNull(base));
          break;
        case "WITHIN_BBOX":
          type = kinds.bbox;
          break;
        case "DISTANCE":
          type = kinds.distance;
          break;
        case "IS_NULL":
          type = GraphQLBoolean;
          break;
        case "CASE_INSENSITIVE":
          type = caseInsensitiveFilter(owner, f);
          break;
        case "CONTAINS":
        case "STARTS_WITH":
        case "ENDS_WITH":
          // A fragment of a custom scalar's value is not one: text
          // operators take the storage type.
          type = f.customScalar
            ? (baseType(f.type, undefined) as GraphQLInputType)
            : base;
          break;
        default:
          type = base;
      }
      fields[name] = { type, description: OPERATOR_DOCS[op] };
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
          } else if (f.kind === "cypher") {
            if (f.computed && f.computed.filters.size > 0) {
              fields[f.name] = {
                type: fieldFilter(node.name, f.computed),
                description:
                  "Computed by @cypher for every node considered: no index applies.",
              };
            }
          } else if (f.kind === "relationship" && f.filterable) {
            fields[f.name] = {
              type: f.list ? relationFilter(node, f) : whereOf(f.target),
              description: f.list
                ? undefined
                : `Has a related ${f.target} matching the filter.`,
            };
            if (!f.list) {
              fields[`${f.name}Exists`] = {
                type: GraphQLBoolean,
                description: `true: ${f.name} is set; false: it is not (a related ${f.target} the reader can't see counts as none).`,
              };
            }
            if (f.list && f.properties && !polymorphic(f)) {
              fields[names.connectionField(f.name)] = {
                type: connectionFilter(node, f),
                description: `Quantifiers over ${f.name} with their relationship properties.`,
              };
            }
          }
        }
        return fields;
      },
    });
    wheres.set(node.name, where);
  }

  // Interfaces: their filterable fields, applied to every implementation,
  // and `typename` to pick implementations. Unions: one where per member.
  for (const abstract of model.abstracts.values()) {
    const implementation =
      abstract.kind === "interface"
        ? new GraphQLEnumType({
            name: `${abstract.name}Implementation`,
            values: Object.fromEntries(abstract.members.map((m) => [m, {}])),
          })
        : undefined;
    const where: GraphQLInputObjectType = new GraphQLInputObjectType({
      name: names.where(abstract.name),
      description:
        abstract.kind === "interface"
          ? `Filters ${abstract.name} nodes of every implementation.`
          : `Filters ${abstract.name} members. With any member named, members not named are left out.`,
      fields: () => {
        if (abstract.kind === "union") {
          return Object.fromEntries(
            abstract.members.map((m) => [m, { type: whereOf(m) }]),
          );
        }
        const fields: GraphQLInputFieldConfigMap = {
          AND: { type: listOf(nonNull(where)) },
          OR: { type: listOf(nonNull(where)) },
          NOT: { type: where },
          typename: {
            type: listOf(nonNull(implementation!)),
            description: "Only these implementations.",
          },
        };
        for (const f of abstract.fields.values()) {
          if (f.filters.size > 0) {
            fields[f.name] = { type: fieldFilter(abstract.name, f) };
          }
        }
        return fields;
      },
    });
    wheres.set(abstract.name, where);
  }

  const polymorphic = (rel: RelationshipField) =>
    model.abstracts.has(rel.target);

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
        ...(polymorphic(rel) || !rel.aggregate
          ? {}
          : {
              aggregate: {
                type: relationAggregateFilter(node, rel),
                description:
                  "Aggregates of the related nodes (and relationship properties).",
              },
            }),
      },
    });

  // --- Connection and aggregate filters -------------------------------------

  const connectionWheres = new Map<string, GraphQLInputObjectType>();
  const connectionWhere = (node: NodeType, rel: RelationshipField) => {
    const name = names.relConnectionWhere(node.name, rel.name);
    let t = connectionWheres.get(name);
    if (!t) {
      const props = model.relationshipProperties.get(rel.properties!)!;
      const edgeWhere = propsWhere(props);
      t = new GraphQLInputObjectType({
        name,
        fields: () => ({
          node: { type: whereOf(rel.target) },
          ...(edgeWhere ? { edge: { type: edgeWhere } } : {}),
        }),
      });
      connectionWheres.set(name, t);
    }
    return t;
  };

  const connectionFilter = (node: NodeType, rel: RelationshipField) => {
    const pair = connectionWhere(node, rel);
    return new GraphQLInputObjectType({
      name: `${names.relConnection(node.name, rel.name)}Filter`,
      fields: {
        some: { type: pair, description: "At least one pair matches." },
        all: { type: pair, description: "Every pair matches." },
        none: { type: pair, description: "No pair matches." },
        single: { type: pair, description: "Exactly one pair matches." },
      },
    });
  };

  const comparisons = new Map<string, GraphQLInputObjectType>();
  const comparison = (base: GraphQLScalarType) => {
    let t = comparisons.get(base.name);
    if (!t) {
      t = new GraphQLInputObjectType({
        name: `${base.name}Comparison`,
        fields: {
          eq: { type: base },
          lt: { type: base },
          lte: { type: base },
          gt: { type: base },
          gte: { type: base },
        },
      });
      comparisons.set(base.name, t);
    }
    return t;
  };

  /** Fields whose values can be aggregated: ordered scalars, not lists. */
  const aggregatable = (f: ScalarField) =>
    !f.list &&
    !f.private &&
    f.selectableOn.aggregate &&
    f.selectableOn.read &&
    !["Boolean", "Enum", "Point", "CartesianPoint"].includes(f.type);
  const isNumeric = (f: ScalarField) =>
    f.type === "Int" || f.type === "Float" || f.type === "BigInt";

  const aggregateFieldFilters = new Map<string, GraphQLInputObjectType>();
  const aggregateFieldFilter = (f: ScalarField) => {
    const base = scalarType(f) as GraphQLScalarType;
    const name = `${base.name}AggregateFilter`;
    let t = aggregateFieldFilters.get(name);
    if (!t) {
      t = new GraphQLInputObjectType({
        name,
        fields: {
          min: { type: comparison(base) },
          max: { type: comparison(base) },
          ...(isNumeric(f)
            ? {
                avg: { type: comparison(GraphQLFloat) },
                sum: {
                  type: comparison(f.type === "Int" ? GraphQLFloat : base),
                },
              }
            : {}),
          ...(f.type === "Duration"
            ? {
                avg: { type: comparison(base) },
                sum: { type: comparison(base) },
              }
            : {}),
          ...(f.type === "String" || f.type === "ID"
            ? {
                shortestLength: { type: comparison(GraphQLInt) },
                longestLength: { type: comparison(GraphQLInt) },
                averageLength: { type: comparison(GraphQLFloat) },
              }
            : {}),
        },
      });
      aggregateFieldFilters.set(name, t);
    }
    return t;
  };

  const aggregateWheres = new Map<string, GraphQLInputObjectType | undefined>();
  const aggregateWhere = (typeName: string, fields: Iterable<ScalarField>) => {
    if (aggregateWheres.has(typeName)) return aggregateWheres.get(typeName);
    const usable = [...fields].filter(aggregatable);
    const t =
      usable.length > 0
        ? new GraphQLInputObjectType({
            name: `${typeName}AggregateWhere`,
            fields: Object.fromEntries(
              usable.map((f) => [f.name, { type: aggregateFieldFilter(f) }]),
            ),
          })
        : undefined;
    aggregateWheres.set(typeName, t);
    return t;
  };

  const relationAggregateFilter = (node: NodeType, rel: RelationshipField) => {
    const target = model.nodes.get(rel.target)!;
    const nodeWhere = aggregateWhere(
      target.name,
      [...target.fields.values()].filter(
        (f): f is ScalarField => f.kind === "scalar",
      ),
    );
    const props = rel.properties
      ? model.relationshipProperties.get(rel.properties)
      : undefined;
    const edgeWhere = props
      ? aggregateWhere(props.name, props.fields.values())
      : undefined;
    return new GraphQLInputObjectType({
      name: `${names.relationFilter(node.name, rel.name)}Aggregate`,
      fields: {
        count: { type: countFilter },
        ...(nodeWhere ? { node: { type: nodeWhere } } : {}),
        ...(edgeWhere ? { edge: { type: edgeWhere } } : {}),
      },
    });
  };

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
      } else if (f.kind === "cypher" && f.computed?.sortable) {
        fields[f.name] = {
          type: sortDirection,
          description: "Computed by @cypher per node: root fields only.",
        };
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
  for (const abstract of model.abstracts.values()) {
    const sortable = [...abstract.fields.values()].filter((f) => f.sortable);
    if (abstract.kind !== "interface" || sortable.length === 0) continue;
    sorts.set(
      abstract.name,
      new GraphQLInputObjectType({
        name: names.sort(abstract.name),
        description: `Sorts ${abstract.name} nodes of every implementation. Ties break by type name, then key.`,
        fields: Object.fromEntries(
          sortable.map((f) => [f.name, { type: sortDirection }]),
        ),
      }),
    );
  }

  // --- Objects and connections ----------------------------------------------

  const objects = new Map<string, GraphQLObjectType>();
  const connections = new Map<string, GraphQLObjectType>();

  // Interfaces and unions over @node types; `__typename` in each projected
  // node tells them apart.
  const abstractTypes = new Map<
    string,
    GraphQLInterfaceType | GraphQLUnionType
  >();
  const resolveType = (value: unknown) =>
    (value as { __typename?: string }).__typename ?? undefined;
  for (const abstract of model.abstracts.values()) {
    abstractTypes.set(
      abstract.name,
      abstract.kind === "interface"
        ? new GraphQLInterfaceType({
            name: abstract.name,
            description: abstract.description,
            fields: () => ({
              ...Object.fromEntries(
                [...abstract.fields.values()]
                  .filter((f) => !f.private && f.selectableOn.read)
                  .map((f) => [
                    f.name,
                    { type: scalarOutput(f), description: f.description },
                  ]),
              ),
              // Declared relationships: the implementations' field, whose
              // type and arguments the model checked are the same.
              ...Object.fromEntries(
                [...abstract.relationships.values()].map((rel) => {
                  const impl = objects.get(abstract.members[0]!)!.getFields()[
                    rel.name
                  ]!;
                  return [
                    rel.name,
                    {
                      type: impl.type,
                      description: rel.description,
                      args: Object.fromEntries(
                        impl.args.map((a) => [
                          a.name,
                          {
                            type: a.type,
                            defaultValue: a.defaultValue,
                            description: a.description,
                          },
                        ]),
                      ),
                    },
                  ];
                }),
              ),
            }),
            resolveType,
          })
        : new GraphQLUnionType({
            name: abstract.name,
            description: abstract.description,
            types: () => abstract.members.map((m) => objects.get(m)!),
            resolveType,
          }),
    );
  }
  const outputOf = (
    name: string,
  ): GraphQLObjectType | GraphQLInterfaceType | GraphQLUnionType =>
    objects.get(name) ?? abstractTypes.get(name)!;

  const connectionResolvers = {
    edges: (src: RawConnection) =>
      pageOf(src).map((row) => {
        assertReadable(row.node);
        return { ...row, __sort: src.__sort };
      }),
    pageInfo: (src: RawConnection) => {
      const page = pageOf(src);
      // Cursors carry sort values: as readable as the nodes themselves.
      for (const row of page) assertReadable(row.node);
      const cursor = (row: RawEdge | undefined) =>
        row ? encodeCursor(src.__sort, row.__cursor, model.cursorSecret) : null;
      const more = src.__rows.length > src.__first;
      return {
        hasNextPage: src.__backward ? src.__after : more,
        hasPreviousPage: src.__backward ? more : src.__after,
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
    aggregate?: () => GraphQLObjectType | undefined,
  ) => {
    const edge = new GraphQLObjectType<SortedEdge>({
      name: edgeName,
      fields: () => ({
        cursor: {
          type: nonNull(GraphQLString),
          resolve: (src) =>
            encodeCursor(src.__sort, src.__cursor, model.cursorSecret),
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
        ...(aggregate?.()
          ? {
              aggregate: {
                type: nonNull(aggregate()!),
                description:
                  "Aggregates over every matching node (not just this page).",
                resolve: (src: RawConnection) =>
                  (src as RawConnection & { __aggregate?: unknown })
                    .__aggregate,
              },
            }
          : {}),
      }),
    });
  };

  /**
   * `{ count, node { f { min max avg sum } }, edge { … } }` for connections
   * whose target has @query(aggregate: true).
   */
  const connectionAggregates = new Map<string, GraphQLObjectType>();
  const aggregateSides = new Map<string, GraphQLObjectType | undefined>();
  const relationshipCount = new GraphQLObjectType({
    name: "RelationshipCount",
    description:
      "Related nodes, and the relationships to them: they differ when several relationships lead to the same node.",
    fields: {
      nodes: { type: nonNull(GraphQLInt) },
      edges: { type: nonNull(GraphQLInt) },
    },
  });
  const connectionAggregate = (
    name: string,
    target: NodeType,
    props: RelationshipPropertiesType | undefined,
    relationship = false,
  ): GraphQLObjectType | undefined => {
    if (!target.aggregate) return undefined;
    let t = connectionAggregates.get(name);
    if (!t) {
      const side = (sideName: string, fields: Iterable<ScalarField>) => {
        if (aggregateSides.has(sideName)) return aggregateSides.get(sideName);
        const usable = [...fields].filter(aggregatable);
        const made =
          usable.length > 0
            ? new GraphQLObjectType({
                name: sideName,
                fields: Object.fromEntries(
                  usable.map((f) => [
                    f.name,
                    { type: nonNull(aggregateFieldType(f)) },
                  ]),
                ),
              })
            : undefined;
        aggregateSides.set(sideName, made);
        return made;
      };
      const node = side(
        `${target.name}AggregateNode`,
        [...target.fields.values()].filter(
          (f): f is ScalarField => f.kind === "scalar",
        ),
      );
      const edge = props
        ? side(`${props.name}AggregateEdge`, props.fields.values())
        : undefined;
      t = new GraphQLObjectType({
        name,
        fields: {
          count: relationship
            ? { type: nonNull(relationshipCount) }
            : { type: nonNull(GraphQLInt), description: "Matching nodes." },
          ...(node ? { node: { type: nonNull(node) } } : {}),
          ...(edge ? { edge: { type: nonNull(edge) } } : {}),
        },
      });
      connectionAggregates.set(name, t);
    }
    return t;
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
              // `@selectable(onRead: false)` hides it here as on a node.
              .filter((f) => !f.private && f.selectableOn.read)
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
    ...(sorts.has(target)
      ? { sort: { type: listOf(nonNull(sorts.get(target)!)) } }
      : {}),
    limit: { type: GraphQLInt },
  });

  // Relationship connections whose properties have @sortable fields sort
  // by `edge` too: `sort: [{ edge: { since: DESC } }, { name: ASC }]`.
  const edgeSorts = new Map<string, GraphQLInputObjectType>();
  const edgeSortOf = (
    node: NodeType,
    rel: RelationshipField,
    props: RelationshipPropertiesType,
  ): GraphQLInputObjectType | undefined => {
    const sortable = [...props.fields.values()].filter((f) => f.sortable);
    const target = model.nodes.get(rel.target)!;
    if (sortable.length === 0 || target.fields.has("edge")) return undefined;
    const name = `${names.relConnection(node.name, rel.name)}Sort`;
    let t = edgeSorts.get(name);
    if (!t) {
      const edgeName = names.sort(props.name);
      let edge = edgeSorts.get(edgeName);
      if (!edge) {
        edge = new GraphQLInputObjectType({
          name: edgeName,
          description: `Sorts by ${props.name} relationship properties.`,
          fields: Object.fromEntries(
            sortable.map((f) => [f.name, { type: sortDirection }]),
          ),
        });
        edgeSorts.set(edgeName, edge);
      }
      t = new GraphQLInputObjectType({
        name,
        description: `Sorts ${rel.name} by ${target.name} fields or, under \`edge\`, relationship properties. One field per item; the @key breaks ties.`,
        fields: () => ({
          ...Object.fromEntries(
            Object.values(sorts.get(rel.target)!.getFields()).map((f) => [
              f.name,
              { type: f.type, description: f.description },
            ]),
          ),
          edge: { type: edge },
        }),
      });
      edgeSorts.set(name, t);
    }
    return t;
  };

  const connectionArgs = (
    target: string,
    where: GraphQLInputType = whereOf(target),
    sort: GraphQLInputObjectType = sorts.get(target)!,
  ): GraphQLFieldConfigArgumentMap => ({
    where: { type: where },
    sort: { type: listOf(nonNull(sort)) },
    first: { type: GraphQLInt },
    after: { type: GraphQLString },
    last: {
      type: GraphQLInt,
      description: "Page backward: the last n before `before`.",
    },
    before: { type: GraphQLString },
  });

  for (const node of model.nodes.values()) {
    const obj: GraphQLObjectType = new GraphQLObjectType({
      name: node.name,
      description: node.description,
      interfaces: () => [
        ...(node.key.relayId && nodeInterface ? [nodeInterface] : []),
        ...node.interfaces
          .map((i) => abstractTypes.get(i))
          .filter(
            (t): t is GraphQLInterfaceType => t instanceof GraphQLInterfaceType,
          ),
      ],
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
            if (f.private || !f.selectableOn.read) continue;
            fields[f.name] = {
              type: scalarOutput(f),
              description: f.description,
              // Vectors are stored tagged; clients see the numbers.
              resolve: f.vector ? vectorByResponseKey : byResponseKey,
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
          if (f.kind === "custom") {
            const base = namedType(f.type.named) as GraphQLOutputType;
            const item =
              f.type.list && f.type.itemRequired ? nonNull(base) : base;
            const t: GraphQLOutputType = f.type.list ? listOf(item) : item;
            fields[f.name] = {
              type: f.type.required ? nonNull(t) : t,
              description: f.description,
              resolve: hooks.customResolver(node, f),
            };
            continue;
          }
          const target = outputOf(f.target);
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
          if (!polymorphic(f)) {
            fields[names.connectionField(f.name)] = relationshipConnectionField(
              node,
              f,
            );
          }
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
        undefined,
        () =>
          connectionAggregate(
            `${names.connection(node.name)}Aggregate`,
            node,
            undefined,
          ),
      ),
    );
  }

  function relationshipConnectionField(
    node: NodeType,
    rel: RelationshipField,
  ): GraphQLFieldConfig<Record<string, unknown>, unknown> {
    if (!hasOwnConnection(rel)) {
      // Aggregates through a relationship count relationships too, and
      // opting out removes them: both need a connection type of its own.
      const target = model.nodes.get(rel.target)!;
      const conn =
        rel.aggregate && !target.aggregate
          ? connections.get(rel.target)!
          : makeConnection(
              names.relConnection(node.name, rel.name),
              names.relEdge(node.name, rel.name),
              () => objects.get(rel.target)!,
              undefined,
              rel.aggregate
                ? () =>
                    connectionAggregate(
                      `${names.relConnection(node.name, rel.name)}Aggregate`,
                      target,
                      undefined,
                      true,
                    )
                : undefined,
            );
      return {
        type: nonNull(conn),
        args: connectionArgs(rel.target),
        resolve: byResponseKey,
      };
    }
    const props = model.relationshipProperties.get(rel.properties!)!;
    const where = connectionWhere(node, rel);
    const conn = makeConnection(
      names.relConnection(node.name, rel.name),
      names.relEdge(node.name, rel.name),
      () => objects.get(rel.target)!,
      // Every property hidden from reads (@private, @selectable(onRead:
      // false)): no edge type, and no `properties` on the edge. The
      // properties stay settable through `edge` inputs.
      [...props.fields.values()].some((f) => !f.private && f.selectableOn.read)
        ? () => propsObject(props)
        : undefined,
      rel.aggregate
        ? () =>
            connectionAggregate(
              `${names.relConnection(node.name, rel.name)}Aggregate`,
              model.nodes.get(rel.target)!,
              props,
              true,
            )
        : undefined,
    );
    return {
      type: nonNull(conn),
      args: connectionArgs(
        rel.target,
        where,
        edgeSortOf(node, rel, props) ?? sorts.get(rel.target)!,
      ),
      resolve: byResponseKey,
    };
  }

  // --- @cypher -----------------------------------------------------------------

  // Object types without @node: shapes of @cypher results, read from maps.
  const plainObjects = new Map<string, GraphQLObjectType>();
  for (const plain of model.objects.values()) {
    plainObjects.set(
      plain.name,
      new GraphQLObjectType({
        name: plain.name,
        description: plain.description,
        fields: () =>
          Object.fromEntries(
            [...plain.fields.values()].map((f) => {
              const base = namedType(f.type.named) as GraphQLOutputType;
              const item =
                f.type.list && f.type.itemRequired ? nonNull(base) : base;
              const t: GraphQLOutputType = f.type.list ? listOf(item) : item;
              return [f.name, { type: f.type.required ? nonNull(t) : t }];
            }),
          ),
      }),
    );
  }

  const namedType = (named: string) =>
    objects.get(named) ??
    abstractTypes.get(named) ??
    plainObjects.get(named) ??
    enums.get(named) ??
    customScalar(named) ??
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
          ...(f.type === "Duration"
            ? { avg: { type: base }, sum: { type: base } }
            : {}),
          ...(f.type === "String" || f.type === "ID"
            ? {
                shortest: { type: base, description: "The shortest value." },
                longest: { type: base, description: "The longest value." },
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
      // Sortable fields opt in; durations cannot be sortable, so they
      // take part whenever they are aggregatable.
      if (
        f.kind === "scalar" &&
        (f.key || f.sortable || f.type === "Duration") &&
        !f.list &&
        f.selectableOn.aggregate &&
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

  // `<plural>Grouped(by: [...])`: the aggregate once per group of @groupBy
  // field values.
  const groups = new Map<
    string,
    { field: GraphQLEnumType; group: GraphQLObjectType }
  >();
  for (const node of model.nodes.values()) {
    if (!node.aggregate) continue;
    const keys = [...node.fields.values()].filter(
      (f): f is ScalarField => f.kind === "scalar" && f.groupBy,
    );
    if (keys.length === 0) continue;
    const field = new GraphQLEnumType({
      name: `${node.name}GroupField`,
      values: Object.fromEntries(keys.map((f) => [f.name, { value: f.name }])),
    });
    const key = new GraphQLObjectType({
      name: `${node.name}GroupKey`,
      description:
        "The group's values of the fields grouped by; the other fields are null.",
      fields: Object.fromEntries(
        keys.map((f) => [f.name, { type: scalarType(f) }]),
      ),
    });
    const group = new GraphQLObjectType({
      name: `${node.name}Group`,
      fields: {
        by: { type: nonNull(key) },
        aggregate: { type: nonNull(aggregates.get(node.name)!) },
      },
    });
    groups.set(node.name, { field, group });
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

  const mutation: GraphQLFieldConfigMap<unknown, unknown> = buildMutations(
    {
      model,
      object: (name) => objects.get(name)!,
      inputType: inputTypeOf,
      where: (name) => whereOf(name),
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

  const subscription: GraphQLFieldConfigMap<unknown, unknown> = {};
  const changeOperation = new GraphQLEnumType({
    name: "ChangeOperation",
    values: {
      CREATE: {},
      UPDATE: {},
      DELETE: {},
      CONNECT: {
        description:
          "A relationship was created (`@subscription(relationships: true)`).",
      },
      DISCONNECT: {
        description:
          "A relationship was removed (`@subscription(relationships: true)`).",
      },
    },
  });
  const changeRelationship = new GraphQLObjectType({
    name: "ChangeRelationship",
    fields: {
      field: {
        type: nonNull(GraphQLString),
        description: "`Type.field` that declares it.",
      },
      type: {
        type: nonNull(GraphQLString),
        description: "The relationship type.",
      },
      relatedType: { type: nonNull(GraphQLString) },
      relatedKey: {
        type: nonNull(GraphQLString),
        resolve: (src: { relatedKey: unknown }) => String(src.relatedKey),
      },
    },
  });
  for (const node of model.nodes.values()) {
    if (!node.read || node.subscriptions.size === 0) continue;
    const obj = objects.get(node.name)!;
    const keyType = scalarType(node.key) as GraphQLInputType &
      GraphQLOutputType;
    // The stored values before an update or delete: readable scalar
    // fields without field-level validate rules (they cannot be checked
    // after the write). Masks and @authentication apply on resolve.
    const previousFields = [...node.fields.values()].filter(
      (f): f is ScalarField =>
        f.kind === "scalar" &&
        !f.private &&
        f.selectableOn.read &&
        !f.vector &&
        !(f.authorization?.validate.length ?? 0),
    );
    const previousState =
      node.subscriptionOptions.previousState && previousFields.length > 0
        ? new GraphQLObjectType<Record<string, unknown>>({
            name: `${node.name}PreviousState`,
            fields: Object.fromEntries(
              previousFields.map((f) => [
                f.name,
                {
                  type: scalarOutput(f),
                  resolve: (
                    src: Record<string, unknown>,
                    _args: unknown,
                    context: unknown,
                  ) =>
                    hooks.previousValue(
                      node,
                      f,
                      src[f.property] ?? null,
                      context,
                    ),
                },
              ]),
            ),
          })
        : undefined;
    const event = new GraphQLObjectType<ChangeEvent>({
      name: `${node.name}ChangeEvent`,
      fields: {
        operation: { type: nonNull(changeOperation) },
        timestamp: {
          type: nonNull(GraphQLString),
          description: "When the write was committed (ISO-8601).",
          resolve: (src) => src.timestamp ?? new Date(0).toISOString(),
        },
        ...(node.subscriptionOptions.relationships
          ? {
              relationship: {
                type: changeRelationship,
                description: "For CONNECT and DISCONNECT: the relationship.",
                resolve: (src: ChangeEvent) => src.relationship ?? null,
              },
            }
          : {}),
        ...(previousState
          ? {
              previousState: {
                type: previousState,
                description: "Stored values before an update or delete.",
                resolve: (src: ChangeEvent) => src.previous ?? null,
              },
            }
          : {}),
        [node.key.name]: {
          type: nonNull(keyType),
          resolve: (src) => src.key,
        },
        node: {
          type: obj,
          description: `The ${node.name} as it is now; null once deleted.`,
          resolve: (src, _args, context, info) =>
            hooks.resolveChangedNode(node, src, info, context),
        },
      },
    });
    subscription[`${lowerFirst(node.name)}Changed`] = {
      type: nonNull(event),
      description: `${node.name} changes made through this API. Give ${node.key.name} to follow one node.`,
      args: {
        [node.key.name]: { type: keyType },
        operations: { type: listOf(nonNull(changeOperation)) },
        where: {
          type: whereOf(node.name),
          description:
            "Only nodes matching this, as they are after the write. Deletions are then not sent.",
        },
      },
      subscribe: (_src, args, context) =>
        hooks.subscribe(node, args as Record<string, unknown>, context),
      resolve: (payload: unknown) => payload,
    };
  }

  if (Object.keys(query).length === 0) {
    throw new ModelError([
      { message: "no readable @node types: the Query type would be empty" },
    ]);
  }

  return new GraphQLSchema({
    query: new GraphQLObjectType({ name: "Query", fields: query }),
    subscription:
      Object.keys(subscription).length > 0
        ? new GraphQLObjectType({ name: "Subscription", fields: subscription })
        : undefined,
    mutation:
      Object.keys(mutation).length > 0
        ? new GraphQLObjectType({ name: "Mutation", fields: mutation })
        : undefined,
    types: [
      ...objects.values(),
      ...abstractTypes.values(),
      ...plainObjects.values(),
    ],
  });
}
