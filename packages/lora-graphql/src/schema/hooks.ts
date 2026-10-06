// What the schema asks of its host: the resolver hooks buildSchema wires
// into the generated fields, and the event a subscription delivers.

import {
  type GraphQLFieldResolver,
  type GraphQLResolveInfo,
  type GraphQLScalarType,
} from "graphql";
import type { RootKind } from "../compile/read.js";
import type {
  AbstractType,
  CustomField,
  CypherField,
  NodeType,
  ScalarField,
  SearchIndex,
} from "../model/types.js";
import type { MutationResolver } from "./mutations.js";

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
