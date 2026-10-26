// `@cacheControl`: how long a result over a type or a field may be served
// from a cache (read by `operationCachePolicy`).

import {
  isInterfaceType,
  isObjectType,
  type GraphQLDirective,
  type GraphQLNamedType,
} from "graphql";
import type { CacheHint, CacheHints } from "../types.js";
import { directive } from "./directive-args.js";

export function readCacheHints(
  userTypes: readonly GraphQLNamedType[],
  def: GraphQLDirective,
  at: (type: string) => (message: string) => void,
): CacheHints {
  const types = new Map<string, CacheHint>();
  const fields = new Map<string, CacheHint>();
  const hint = (
    node: { astNode?: unknown; extensionASTNodes?: unknown },
    where: string,
    report: (message: string) => void,
  ): CacheHint | undefined => {
    const args = directive(def, node, report);
    if (!args) return undefined;
    const maxAge = args["maxAge"] as number;
    if (maxAge < 0) {
      report(`@cacheControl on ${where}: maxAge must be 0 or more seconds`);
      return undefined;
    }
    return {
      maxAge,
      scope: (args["scope"] as CacheHint["scope"] | undefined) ?? "PUBLIC",
    };
  };
  for (const type of userTypes) {
    const report = at(type.name);
    const own = hint(type, type.name, report);
    if (own) types.set(type.name, own);
    if (!isObjectType(type) && !isInterfaceType(type)) continue;
    for (const field of Object.values(type.getFields())) {
      const where = `${type.name}.${field.name}`;
      const fieldHint = hint(field, where, report);
      if (fieldHint) fields.set(where, fieldHint);
    }
  }
  return { types, fields };
}
