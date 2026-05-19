"use client";

/**
 * Imperative actions over the `history` IDB store. The Sidebar
 * history panel and the post-run hook in `runActiveTab` call these
 * instead of poking persistence directly so the panel can refresh
 * off a single `loradb:history` window event.
 */

import * as history from "@/lib/persistence/history";
import { openTabInCell } from "@/lib/actions/tabActions";

export const HISTORY_EVENT = "loradb:history";

function emitChange(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent(HISTORY_EVENT));
}

/** Append a history record. Dispatches `loradb:history` on success. */
export async function appendHistoryEntry(
  entry: Omit<history.HistoryEntry, "id">,
): Promise<void> {
  await history.append(entry);
  emitChange();
}

/** Returns history entries newest-first, capped at `limit` (default 200). */
export async function listHistory(
  limit?: number,
): Promise<history.HistoryEntry[]> {
  return history.list(limit);
}

/** Empties the history store and notifies listeners. */
export async function clearHistory(): Promise<void> {
  await history.clear();
  emitChange();
}

/**
 * Open the body of a history entry in a brand-new tab. The new tab is
 * named "From history" so the user sees at a glance where it came from
 * — they can rename it later via the tab strip.
 *
 * No-ops silently if the entry cannot be located (it may have been
 * evicted by the rolling 1000-entry cap between hover and click).
 */
export async function openHistoryEntryInNewTab(
  entryId: string,
): Promise<void> {
  // We can't `getById` without modifying persistence, so list and find.
  // The list is capped at 200 by default — bump the limit so older
  // entries the user has scrolled to remain openable.
  const entries = await history.list(1000);
  const found = entries.find((e) => e.id === entryId);
  if (!found) return;
  openTabInCell({
    name: "From history",
    body: found.body,
    // Restore the exact param payload the user ran with, so the
    // re-opened tab is a one-click replay rather than a body-only
    // snapshot.
    params: found.params,
  });
}
