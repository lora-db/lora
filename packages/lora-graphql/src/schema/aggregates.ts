// Aggregates: which fields take part, the `{ min max avg sum }` type per
// scalar, and the aggregate and group types of @query(aggregate: true)
// nodes.

import {
  GraphQLEnumType,
  GraphQLFloat,
  GraphQLInt,
  GraphQLObjectType,
  type GraphQLFieldConfigMap,
  GraphQLScalarType,
} from "graphql";
import type { GraphModel, ScalarField } from "../model/types.js";
import { nonNull, type BaseTypes } from "./base.js";
import { names } from "./names.js";

/** Fields whose values can be aggregated: ordered scalars, not lists. */
export const aggregatable = (f: ScalarField) =>
  !f.list &&
  !f.private &&
  f.selectableOn.aggregate &&
  f.selectableOn.read &&
  !["Boolean", "Enum", "Point", "CartesianPoint"].includes(f.type);
export const isNumeric = (f: ScalarField) =>
  f.type === "Int" || f.type === "Float" || f.type === "BigInt";

/** The memoized `<Scalar>Aggregate` type of a field, shared by every aggregate. */
export function buildAggregateFieldType(base: BaseTypes) {
  const { scalarType } = base;

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

  return aggregateFieldType;
}

export type AggregateFieldType = ReturnType<typeof buildAggregateFieldType>;

export function buildAggregates(
  model: GraphModel,
  base: BaseTypes,
  aggregateFieldType: AggregateFieldType,
) {
  const { scalarType } = base;

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

  return { aggregates, groups };
}

export type AggregateTypes = ReturnType<typeof buildAggregates>;
