// Resolvers shared by the generated types: values are read under their
// response keys (the compiler projects each alias), failing on what a
// READ rule refused, and connection pages come out in the requested order.

import type { GraphQLFieldResolver, GraphQLResolveInfo } from "graphql";
import type { RawConnection, RawEdge } from "../compile/read.js";
import { requestError } from "../errors.js";
import { assertReadable } from "./guard.js";

export type SortedEdge = RawEdge & { __sort: string };

/** The rows of a page in the requested order. */
export function pageOf(src: RawConnection): RawEdge[] {
  const page = src.__rows.slice(0, src.__first);
  return src.__backward ? page.reverse() : page;
}

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
export const byResponseKey: GraphQLFieldResolver<
  Record<string, unknown>,
  unknown
> = (source, _args, _ctx, info) => {
  const value = source[info.path.key as string];
  assertFieldReadable(value, info);
  return value;
};

/** A stored VECTOR as its numbers. */
export const vectorByResponseKey: GraphQLFieldResolver<
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
export const nodesByResponseKey: GraphQLFieldResolver<
  Record<string, unknown>,
  unknown
> = (source, _args, _ctx, info) => {
  const value = source[info.path.key as string];
  assertFieldReadable(value, info);
  return assertReadable(value);
};
