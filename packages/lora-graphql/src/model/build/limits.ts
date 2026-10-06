// Page limits: the model options that set them, the global default and
// maximum, and what `@limit` resolves to on a type or a list field.

import { ModelError, type ModelProblem } from "../../errors.js";
import type { PageLimit } from "../types.js";

export interface ModelOptions {
  /** Page size used when a list or connection gets no `limit` / `first`. */
  defaultLimit?: number;
  /** Hard cap on any page size; `@limit(max:)` may only lower it. */
  maxLimit?: number;
  /**
   * Sign cursors with HMAC-SHA-256 under this secret, and reject cursors
   * whose signature does not match. Without it cursors are only tagged
   * with their sort. Changing the secret invalidates every cursor.
   */
  cursorSecret?: string;
}

export const DEFAULT_LIMIT = 25;
export const MAX_LIMIT = 100;

export function resolveGlobalLimit(options: ModelOptions): PageLimit {
  const max = options.maxLimit ?? MAX_LIMIT;
  const def = Math.min(options.defaultLimit ?? DEFAULT_LIMIT, max);
  if (!(max >= 1) || !(def >= 1)) {
    throw new ModelError([
      { message: "defaultLimit and maxLimit must be at least 1" },
    ]);
  }
  return { default: def, max };
}

export function resolveLimit(
  args: Record<string, unknown> | undefined,
  global: PageLimit,
  type: string,
  field: string | undefined,
  problems: ModelProblem[],
): PageLimit {
  if (!args) return global;
  const max = (args["max"] as number | undefined) ?? global.max;
  const def =
    (args["default"] as number | undefined) ?? Math.min(global.default, max);
  const at = (message: string) =>
    problems.push(field ? { type, field, message } : { type, message });
  if (max < 1 || def < 1) at("@limit values must be at least 1");
  if (max > global.max)
    at(`@limit(max: ${max}) exceeds the global maximum ${global.max}`);
  if (def > max) at(`@limit(default: ${def}) exceeds max ${max}`);
  return {
    default: Math.min(def, max, global.max),
    max: Math.min(max, global.max),
  };
}
