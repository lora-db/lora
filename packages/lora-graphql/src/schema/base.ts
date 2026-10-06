// The types every part of the schema builds on: the fixed ones (sort
// direction, page info, points), the model's enums and custom scalars,
// and the GraphQL type of a stored field.

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
  GraphQLString,
  type GraphQLInputType,
  type GraphQLOutputType,
  GraphQLScalarType,
} from "graphql";
import type { GraphModel, ScalarField, ScalarType } from "../model/types.js";
import type { SchemaHooks } from "./hooks.js";
import { CUSTOM_SCALARS } from "./scalars.js";

export const nonNull = <T extends GraphQLOutputType | GraphQLInputType>(t: T) =>
  new GraphQLNonNull(t);
export const listOf = <T extends GraphQLOutputType | GraphQLInputType>(t: T) =>
  new GraphQLList(t);

export function buildBaseTypes(model: GraphModel, hooks: SchemaHooks) {
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

  return {
    sortDirection,
    pageInfo,
    countFilter,
    enums,
    nodeInterface,
    customScalar,
    scalarType,
    baseType,
    scalarOutput,
  };
}

export type BaseTypes = ReturnType<typeof buildBaseTypes>;
