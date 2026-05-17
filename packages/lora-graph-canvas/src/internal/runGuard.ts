import type { DeletionGuard, DeletionSource } from "../types";

/** Runs an optional deletion guard, returning `true` when the mutation
 *  should proceed. Empty batches short-circuit to `true` so callers can
 *  use the same code path whether or not anything is actually selected.
 *  A thrown error in the guard is swallowed as a cancel — a throwing
 *  host should not silently destroy data. */
export async function runGuard<T>(
  guard: DeletionGuard<T> | undefined,
  items: T[],
  source: DeletionSource,
): Promise<boolean> {
  if (items.length === 0) return true;
  if (!guard) return true;
  try {
    return await guard(items, { source });
  } catch {
    return false;
  }
}
