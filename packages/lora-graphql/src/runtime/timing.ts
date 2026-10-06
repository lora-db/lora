// Request timing (the `timing` option): per-root-field entries collected
// while a request runs, reported as `extensions.timing`.

import type { GraphQLResolveInfo } from "graphql";

/** A request's timing entry for a root field, created on first use. */
export function timingEntry(
  timing: Map<string, { totalMs: number; databaseMs: number }>,
  key: string,
): { totalMs: number; databaseMs: number } {
  let entry = timing.get(key);
  if (!entry) {
    entry = { totalMs: 0, databaseMs: 0 };
    timing.set(key, entry);
  }
  return entry;
}

/** The response key of the root field `info` belongs to. */
export function rootKey(
  info: GraphQLResolveInfo | undefined,
): string | undefined {
  let path = info?.path;
  if (!path) return undefined;
  while (path.prev) path = path.prev;
  return String(path.key);
}

/** Milliseconds, to a hundredth. */
export const ms = (value: number) => Math.round(value * 100) / 100;
