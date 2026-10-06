// The Subscription fields: `<type>Changed` per @node type with
// @subscription, its change event, and the previous state of the node.

import {
  GraphQLEnumType,
  GraphQLObjectType,
  GraphQLString,
  type GraphQLFieldConfigMap,
  type GraphQLInputType,
  type GraphQLOutputType,
} from "graphql";
import { lowerFirst } from "../model/build.js";
import type { GraphModel, ScalarField } from "../model/types.js";
import { listOf, nonNull, type BaseTypes } from "./base.js";
import type { Filters } from "./filters.js";
import type { ChangeEvent, SchemaHooks } from "./hooks.js";
import type { ObjectTypes } from "./objects.js";

export function buildSubscriptions(
  model: GraphModel,
  hooks: SchemaHooks,
  base: BaseTypes,
  filters: Filters,
  objectTypes: ObjectTypes,
) {
  const { scalarType, scalarOutput } = base;
  const { whereOf } = filters;
  const { objects } = objectTypes;

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

  return subscription;
}
