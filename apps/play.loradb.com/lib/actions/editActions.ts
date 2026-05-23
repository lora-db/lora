"use client";

/**
 * Property-edit actions wired up by the inspector's edit mode.
 *
 * Mirrors `deleteActions` in shape: run a single `read_write`
 * transaction directly against the WASM database, surface failures via
 * Mantine notifications, and dispatch the global mutation event so
 * other listeners (schema introspection, db-status counts) refresh.
 *
 * The mutation is intentionally a whole-property-map overwrite
 * (`SET n = $props`) so add / remove / change all collapse into one
 * round-trip with no diffing required.
 */

import { notifications } from "@mantine/notifications";

import type { LoraParams, TransactionStatement } from "@loradb/lora-wasm";
import { getDb } from "@/lib/db/client";
import { LORADB_MUTATION_EVENT } from "@/lib/actions/runActiveTab";

export interface UpdatePropertiesResult {
  ok: boolean;
  /** Error message when `ok === false`. */
  message?: string;
}

function isPlainProperties(
  v: Record<string, unknown>,
): v is Record<string, LoraParams[string]> {
  // The driver accepts the same primitive set everywhere — we keep the
  // check shallow because the inputs are already validated by the
  // editor's per-row parser. Anything that slips through is rejected
  // with a clear error by the engine, not silently coerced.
  for (const k of Object.keys(v)) {
    if (k.length === 0) return false;
  }
  return true;
}

async function runUpdate(
  query: string,
  params: Record<string, unknown>,
): Promise<UpdatePropertiesResult> {
  try {
    const db = await getDb();
    const statements: TransactionStatement[] = [
      { query, params: params as LoraParams },
    ];
    await db.transaction(statements, "read_write");
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    notifications.show({
      color: "red",
      title: "Save failed",
      message,
    });
    return { ok: false, message };
  }

  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent(LORADB_MUTATION_EVENT));
  }
  return { ok: true };
}

/**
 * Overwrite the properties of the node identified by `id` with the
 * supplied map. Labels are not touched. Numeric ids only — string ids
 * belong to canvas-local nodes that don't exist in the database.
 */
export async function updateNodeProperties(
  id: number,
  properties: Record<string, unknown>,
): Promise<UpdatePropertiesResult> {
  if (!Number.isFinite(id)) {
    return { ok: false, message: "Node has no database id." };
  }
  if (!isPlainProperties(properties)) {
    return { ok: false, message: "Property names must be non-empty." };
  }
  return runUpdate("MATCH (n) WHERE id(n) = $id SET n = $props", {
    id,
    props: properties,
  });
}

/**
 * Overwrite the properties of the relationship identified by `id` with
 * the supplied map. Type and endpoints are not touched.
 */
export async function updateRelationshipProperties(
  id: number,
  properties: Record<string, unknown>,
): Promise<UpdatePropertiesResult> {
  if (!Number.isFinite(id)) {
    return { ok: false, message: "Relationship has no database id." };
  }
  if (!isPlainProperties(properties)) {
    return { ok: false, message: "Property names must be non-empty." };
  }
  return runUpdate("MATCH ()-[r]-() WHERE id(r) = $id SET r = $props", {
    id,
    props: properties,
  });
}
