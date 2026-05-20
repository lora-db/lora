"use client";

/**
 * Imperative actions for the Schema Design surface.
 *
 * - `refreshSchemaDesign()` pulls a fresh catalog snapshot. Safe to
 *   call concurrently — the last resolution wins.
 * - `createIndex` / `createConstraint` / `dropIndex` / `dropConstraint`
 *   run the DDL and refresh on success, surfacing friendly errors via
 *   the notifications channel on failure.
 *
 * `attachSchemaDesignMutationListener` mirrors the schema-introspection
 * one: re-fetches the catalog (debounced) on `loradb:mutation` so a
 * `CREATE INDEX` typed manually in the editor immediately reflects in
 * the designer panel.
 */

import { notifications } from "@mantine/notifications";

import { useStore } from "@/lib/state/store";
import { debounce } from "@/lib/util/async";
import { LORADB_MUTATION_EVENT } from "@/lib/actions/runActiveTab";
import {
  fetchSchemaDesignSnapshot,
  runDDL,
  SchemaDesignError,
} from "@/lib/db/schemaDesign";
import {
  buildCreateConstraintDDL,
  buildCreateIndexDDL,
  buildDropConstraintDDL,
  buildDropIndexDDL,
} from "@/lib/schemaDesign/ddl";
import { translateError } from "@/lib/schemaDesign/errorTranslate";
import type { ConstraintDraft, IndexDraft } from "@/lib/schemaDesign/types";

let inFlight = 0;

/** Re-introspect indexes + constraints and push into the store. */
export async function refreshSchemaDesign(): Promise<void> {
  const state = useStore.getState();
  state.setSchemaDesignRefreshing(true);
  const ticket = ++inFlight;
  try {
    const snap = await fetchSchemaDesignSnapshot();
    // Drop the result if a newer fetch superseded us — avoids the
    // older response clobbering fresh data.
    if (ticket !== inFlight) return;
    useStore.getState().setSchemaDesign(snap);
  } catch (err) {
    if (ticket !== inFlight) return;
    useStore.getState().setSchemaDesignError();
    const message = err instanceof Error ? err.message : String(err);
    notifications.show({
      color: "red",
      title: "Couldn't load the schema catalog",
      message,
    });
  } finally {
    if (ticket === inFlight) {
      useStore.getState().setSchemaDesignRefreshing(false);
    }
  }
}

function reportEngineFailure(err: unknown, fallbackTitle: string): void {
  const message = err instanceof Error ? err.message : String(err);
  const friendly = translateError(message);
  notifications.show({
    color: "red",
    title: friendly.title || fallbackTitle,
    message: friendly.body,
  });
}

/** Run a draft's CREATE INDEX statement and refresh on success. */
export async function createIndex(draft: IndexDraft): Promise<boolean> {
  const ddl = buildCreateIndexDDL(draft);
  try {
    await runDDL(ddl);
    notifications.show({
      color: "green",
      title: "Index created",
      message: `“${draft.name}” is online.`,
    });
    await refreshSchemaDesign();
    return true;
  } catch (err) {
    reportEngineFailure(err, "Couldn't create the index");
    return false;
  }
}

/** Run a draft's CREATE CONSTRAINT statement and refresh on success. */
export async function createConstraint(
  draft: ConstraintDraft,
): Promise<boolean> {
  const ddl = buildCreateConstraintDDL(draft);
  try {
    await runDDL(ddl);
    notifications.show({
      color: "green",
      title: "Constraint created",
      message: `“${draft.name}” is active.`,
    });
    await refreshSchemaDesign();
    return true;
  } catch (err) {
    reportEngineFailure(err, "Couldn't create the constraint");
    return false;
  }
}

/**
 * Apply edits to an existing index by dropping it and recreating it
 * with the new draft. Engine has no `ALTER INDEX`, so this is the only
 * path. The two statements aren't a single transaction — if recreation
 * fails the original is already gone, so we surface a louder error
 * pointing at the broken state.
 */
export async function updateIndex(
  oldName: string,
  draft: IndexDraft,
): Promise<boolean> {
  try {
    await runDDL(buildDropIndexDDL(oldName, true));
  } catch (err) {
    reportEngineFailure(err, "Couldn't update the index");
    return false;
  }
  try {
    await runDDL(buildCreateIndexDDL(draft));
    notifications.show({
      color: "green",
      title: "Index updated",
      message: `“${oldName}” was replaced with “${draft.name}”.`,
    });
    await refreshSchemaDesign();
    return true;
  } catch (err) {
    notifications.show({
      color: "red",
      title: "Index update partially failed",
      message: `“${oldName}” was dropped, but the replacement couldn't be created. ${
        err instanceof Error ? err.message : String(err)
      }`,
      autoClose: false,
    });
    await refreshSchemaDesign();
    return false;
  }
}

/** Same idea as {@link updateIndex} for constraints. */
export async function updateConstraint(
  oldName: string,
  draft: ConstraintDraft,
): Promise<boolean> {
  try {
    await runDDL(buildDropConstraintDDL(oldName, true));
  } catch (err) {
    reportEngineFailure(err, "Couldn't update the constraint");
    return false;
  }
  try {
    await runDDL(buildCreateConstraintDDL(draft));
    notifications.show({
      color: "green",
      title: "Constraint updated",
      message: `“${oldName}” was replaced with “${draft.name}”.`,
    });
    await refreshSchemaDesign();
    return true;
  } catch (err) {
    notifications.show({
      color: "red",
      title: "Constraint update partially failed",
      message: `“${oldName}” was dropped, but the replacement couldn't be created. ${
        err instanceof Error ? err.message : String(err)
      }`,
      autoClose: false,
    });
    await refreshSchemaDesign();
    return false;
  }
}

/** Drop an index by name; refresh on success. */
export async function dropIndex(name: string): Promise<boolean> {
  try {
    await runDDL(buildDropIndexDDL(name, true));
    notifications.show({
      color: "green",
      title: "Index dropped",
      message: `“${name}” was removed.`,
    });
    await refreshSchemaDesign();
    return true;
  } catch (err) {
    reportEngineFailure(err, "Couldn't drop the index");
    return false;
  }
}

/** Drop a constraint by name; refresh on success. */
export async function dropConstraint(name: string): Promise<boolean> {
  try {
    await runDDL(buildDropConstraintDDL(name, true));
    notifications.show({
      color: "green",
      title: "Constraint dropped",
      message: `“${name}” was removed.`,
    });
    await refreshSchemaDesign();
    return true;
  } catch (err) {
    reportEngineFailure(err, "Couldn't drop the constraint");
    return false;
  }
}

/** Convenience: re-fetch on the WASM mutation event, debounced. */
export function attachSchemaDesignMutationListener(): () => void {
  if (typeof window === "undefined") return () => {};
  const debounced = debounce(() => {
    void refreshSchemaDesign();
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

export { SchemaDesignError };
