// GraphModel → executable GraphQLSchema. Generates only what the model
// opts into: filters for @filterable fields, sorts for @sortable ones,
// aggregates for @query(aggregate: true), mutations for @mutation.

import {
  GraphQLObjectType,
  GraphQLSchema,
  type GraphQLFieldConfigMap,
} from "graphql";
import { ModelError } from "../errors.js";
import type { GraphModel } from "../model/types.js";
import { buildAggregateFieldType, buildAggregates } from "./aggregates.js";
import { buildBaseTypes } from "./base.js";
import { buildFilters } from "./filters.js";
import type { SchemaHooks } from "./hooks.js";
import { buildMutations } from "./mutations.js";
import { buildObjects } from "./objects.js";
import { buildQuery } from "./query.js";
import { buildSorts } from "./sorts.js";
import { buildSubscriptions } from "./subscriptions.js";

export type {
  ChangeEvent,
  CypherRootResolver,
  NodeResolver,
  RootResolver,
  SchemaHooks,
} from "./hooks.js";

export function buildSchema(
  model: GraphModel,
  hooks: SchemaHooks,
): GraphQLSchema {
  const base = buildBaseTypes(model, hooks);
  const filters = buildFilters(model, base);
  const { inputTypeOf, whereOf } = filters;
  const sorts = buildSorts(model, base);
  // Made here, before the objects whose connection aggregates use it.
  const aggregateFieldType = buildAggregateFieldType(base);
  const objectTypes = buildObjects(
    model,
    hooks,
    base,
    filters,
    sorts,
    aggregateFieldType,
  );
  const { objects, abstractTypes, plainObjects, cypherOutput, cypherArgs } =
    objectTypes;
  const aggregateTypes = buildAggregates(model, base, aggregateFieldType);

  const query = buildQuery(
    model,
    hooks,
    base,
    filters,
    objectTypes,
    aggregateTypes,
  );

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

  const subscription = buildSubscriptions(
    model,
    hooks,
    base,
    filters,
    objectTypes,
  );

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
