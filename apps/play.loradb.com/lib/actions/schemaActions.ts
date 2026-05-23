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

import { notifications } from "@mantine/notifications";

import { run } from "@/lib/db/client";
import { introspect } from "@/lib/db/schema";
import { useStore } from "@/lib/state/store";
import { debounce } from "@/lib/util/async";
import { LORADB_MUTATION_EVENT } from "@/lib/actions/runActiveTab";
import { labelDeleteAll, relTypeDeleteAll } from "@/lib/snippets/cypher";

let inFlight = 0;

/**
 * Re-introspect the database and push the resulting snapshot into the
 * schema slice. Sets `refreshing` true while in flight and always
 * clears it afterwards (even on failure). Safe to call concurrently —
 * the last resolution wins.
 */
export async function refreshSchema(): Promise<void> {
  const state = useStore.getState();
  state.setRefreshing(true);
  const ticket = ++inFlight;
  try {
    const snap = await introspect();
    if (ticket !== inFlight) return;
    useStore.getState().setSchema(snap);
  } catch (err) {
    if (ticket !== inFlight) return;
    // Introspection promises are non-throwing today, but guard anyway
    // so the slice doesn't end up wedged in `refreshing: true`.
    console.warn("schema refresh failed", err);
    useStore.getState().setSchema(null);
  } finally {
    if (ticket === inFlight) {
      useStore.getState().setRefreshing(false);
    }
  }
}

export interface RemoveAllOutcome {
  ok: boolean;
  /** Server-reported message when `ok === false`. */
  message?: string;
}

/**
 * Drop every node carrying `label` (with `DETACH` so attached
 * relationships go too). Surfaces a notification on success/failure
 * and dispatches `loradb:mutation` so the schema slice re-introspects.
 */
export async function deleteAllOfLabel(
  label: string,
): Promise<RemoveAllOutcome> {
  return runRemoval(labelDeleteAll(label), `nodes labelled ${label}`);
}

/**
 * Drop every relationship of `relType`. Endpoints stay in place —
 * removing rels never cascades to nodes by Cypher semantics.
 */
export async function deleteAllOfRelType(
  relType: string,
): Promise<RemoveAllOutcome> {
  return runRemoval(relTypeDeleteAll(relType), `relationships :${relType}`);
}

async function runRemoval(
  cypher: string,
  description: string,
): Promise<RemoveAllOutcome> {
  const outcome = await run(cypher);
  if (outcome.state !== "ok") {
    notifications.show({
      color: "red",
      title: "Remove failed",
      message: outcome.message,
    });
    return { ok: false, message: outcome.message };
  }
  notifications.show({
    color: "green",
    title: "Removed",
    message: `All ${description} removed from the in-memory session.`,
    autoClose: 2400,
  });
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent(LORADB_MUTATION_EVENT));
  }
  return { ok: true };
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
