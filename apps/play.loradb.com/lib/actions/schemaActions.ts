"use client";

/**
 * Imperative schema actions. The Sidebar panel and the editor pane
 * call these without importing the introspection module directly,
 * so the store-mutation contract stays in one place.
 *
 * `attachSchemaMutationListener` glues the `loradb:mutation` window
 * event (emitted by `runActiveTab` after mutating queries) to a
 * debounced refresh — multiple back-to-back mutations coalesce into
 * a single re-introspection.
 */

import { introspect } from "@/lib/db/schema";
import { useStore } from "@/lib/state/store";
import { debounce } from "@/lib/util/async";
import { LORADB_MUTATION_EVENT } from "@/lib/actions/runActiveTab";

/**
 * Re-introspect the database and push the resulting snapshot into the
 * schema slice. Sets `refreshing` true while in flight and always
 * clears it afterwards (even on failure). Safe to call concurrently —
 * the last resolution wins.
 */
export async function refreshSchema(): Promise<void> {
  const state = useStore.getState();
  state.setRefreshing(true);
  try {
    const snap = await introspect();
    useStore.getState().setSchema(snap);
  } catch {
    // Introspection promises are non-throwing today, but guard anyway
    // so the slice doesn't end up wedged in `refreshing: true`.
    useStore.getState().setSchema(null);
  } finally {
    useStore.getState().setRefreshing(false);
  }
}

/**
 * Subscribe `refreshSchema` (debounced 300ms) to the `loradb:mutation`
 * window event. Returns a detach function — typically wired to a React
 * effect cleanup. No-ops on the server.
 */
export function attachSchemaMutationListener(): () => void {
  if (typeof window === "undefined") return () => {};

  const debounced = debounce(() => {
    void refreshSchema();
  }, 300);

  const handler = (): void => {
    debounced();
  };
  window.addEventListener(LORADB_MUTATION_EVENT, handler);
  return () => {
    window.removeEventListener(LORADB_MUTATION_EVENT, handler);
    debounced.cancel();
  };
}
