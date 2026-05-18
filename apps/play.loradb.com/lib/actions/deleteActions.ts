"use client";

/**
 * Executes a node/relationship deletion against the in-memory LoraDB
 * via a single Cypher transaction, so the canvas's "Remove" confirm in
 * `ConfirmDeleteDialog` actually mutates the database instead of only
 * the on-screen view.
 *
 * Canvas items with non-numeric ids (e.g. nodes added via the canvas's
 * `add-node` tool that were never persisted) are ignored for the DB
 * transaction — there's nothing to delete — but the canvas still gets
 * the green light to remove them from its in-memory view.
 *
 * On success we dispatch `loradb:mutation` so `useDbStatus`, schema,
 * and any other listeners refresh their counts. We deliberately do not
 * re-run the active tab here: the canvas drops the deleted nodes from
 * its uncontrolled view as soon as the guard resolves, and a re-run
 * would mint a new `runId` and remount `GraphView`, reseeding the
 * physics simulation and snapping the layout. Other result panes
 * (table / JSON) stay on the pre-delete snapshot until the user reruns
 * the query explicitly.
 */

import { notifications } from "@mantine/notifications";
import type {
  LinkObject,
  NodeObject,
} from "@loradb/lora-graph-canvas";
import type { TransactionStatement } from "@loradb/lora-wasm";
import { getDb } from "@/lib/db/client";
import { LORADB_MUTATION_EVENT } from "@/lib/actions/runActiveTab";

export interface DeleteSelectionResult {
  ok: boolean;
  /** Number of DB-resident nodes the transaction targeted. */
  dbNodes: number;
  /** Number of DB-resident relationships the transaction targeted. */
  dbLinks: number;
  /** Error message when `ok === false`. */
  message?: string;
}

/** Filter to ids that look like LoraDB internal ids (numeric).
 *  Canvas-local entities created by the add-node / add-link tools use
 *  string ids (`n-<n>` / `l-<n>`) and don't exist in the database. */
function numericIds(ids: Array<string | number | undefined>): number[] {
  const out: number[] = [];
  for (const id of ids) {
    if (typeof id === "number" && Number.isFinite(id)) {
      out.push(id);
    }
  }
  return out;
}

export async function deleteFromGraph(
  nodes: NodeObject[],
  links: LinkObject[],
): Promise<DeleteSelectionResult> {
  const nodeIds = numericIds(nodes.map((n) => n.id));
  const linkIds = numericIds(links.map((l) => l.id));

  if (nodeIds.length === 0 && linkIds.length === 0) {
    return { ok: true, dbNodes: 0, dbLinks: 0 };
  }

  const statements: TransactionStatement[] = [];
  // Relationship deletes first — when the node `DETACH DELETE` runs it
  // will also drop the relationships, but routing the explicit
  // selection through its own statement gives us a precise count and
  // means a user who only selected relationships still gets an
  // accurate post-run dispatch.
  if (linkIds.length > 0) {
    statements.push({
      query: "MATCH ()-[r]-() WHERE id(r) IN $ids DELETE r",
      params: { ids: linkIds },
    });
  }
  if (nodeIds.length > 0) {
    statements.push({
      query: "MATCH (n) WHERE id(n) IN $ids DETACH DELETE n",
      params: { ids: nodeIds },
    });
  }

  try {
    const db = await getDb();
    await db.transaction(statements, "read_write");
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    notifications.show({
      color: "red",
      title: "Delete failed",
      message,
    });
    return {
      ok: false,
      dbNodes: nodeIds.length,
      dbLinks: linkIds.length,
      message,
    };
  }

  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent(LORADB_MUTATION_EVENT));
  }
  return { ok: true, dbNodes: nodeIds.length, dbLinks: linkIds.length };
}
