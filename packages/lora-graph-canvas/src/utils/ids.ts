/** Returns a monotonically-increasing, prefixed string ID that's unique
 *  for the lifetime of the page. Dep-free (we don't pull in nanoid for
 *  one call site). The counter starts from a small random offset so
 *  multiple instances on a page don't collide on the first IDs. */
const counters = new Map<string, number>();
const SEED = Math.floor(Math.random() * 1000);

export function createId(prefix = "n"): string {
  const next = (counters.get(prefix) ?? SEED) + 1;
  counters.set(prefix, next);
  return `${prefix}_${next.toString(36)}`;
}

/** Reset all counters. Intended only for tests so they observe stable IDs. */
export function __resetIdCounters(): void {
  counters.clear();
}
