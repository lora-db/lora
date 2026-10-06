// The `where` inputs: one filter per @filterable field, relationship
// quantifiers, connection filters over relationship properties, and
// filters on aggregates of related nodes.

import {
  GraphQLBoolean,
  GraphQLEnumType,
  GraphQLFloat,
  GraphQLInputObjectType,
  GraphQLInt,
  GraphQLString,
  type GraphQLInputFieldConfigMap,
  type GraphQLInputType,
  GraphQLScalarType,
} from "graphql";
import type {
  FilterOperator,
  GraphModel,
  NodeType,
  RelationshipField,
  RelationshipPropertiesType,
  ScalarField,
} from "../model/types.js";
import { aggregatable, isNumeric } from "./aggregates.js";
import { listOf, nonNull, type BaseTypes } from "./base.js";
import { names } from "./names.js";

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

export function buildFilters(model: GraphModel, base: BaseTypes) {
  const { countFilter, scalarType, baseType } = base;

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

  return { inputTypeOf, whereOf, polymorphic, connectionWhere };
}

export type Filters = ReturnType<typeof buildFilters>;
