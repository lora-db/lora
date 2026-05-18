"use client";

/**
 * Imperative actions that bridge the editor tabs slice with the
 * `savedQueries` IDB store. The Sidebar panel and Spotlight hotkeys
 * call these without ever talking to persistence directly.
 *
 * Every IDB-mutating action ends with a `loradb:savedQueries` window
 * event so the panel can refresh its in-memory list without polling.
 */

import { useStore } from "@/lib/state/store";
import * as savedQueries from "@/lib/persistence/savedQueries";
import { openTabInCell } from "@/lib/actions/tabActions";
import { focusTabInWorkspace, getActiveTabId } from "@/lib/actions/workspaceActions";

export const SAVED_QUERIES_EVENT = "loradb:savedQueries";

function emitChange(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent(SAVED_QUERIES_EVENT));
}

/**
 * Save the active tab's body. If the tab already has a `savedQueryId`,
 * updates that record in place and marks the tab clean. If the tab is
 * not yet bound to a saved query, returns `null` — the caller is
 * expected to open the SaveQueryDialog and call `saveActiveTabAs`.
 */
export async function saveActiveTab(): Promise<savedQueries.SavedQuery | null> {
  const tabId = getActiveTabId();
  if (tabId === null) return null;
  const state = useStore.getState();
  const tab = state.tabs.find((t) => t.id === tabId);
  if (!tab) return null;

  if (!tab.savedQueryId) return null;

  const record = await savedQueries.update(tab.savedQueryId, {
    body: tab.body,
  });
  useStore.getState().markClean(tab.id);
  emitChange();
  return record;
}

/**
 * Save the active tab as a brand-new saved query under `name`. Binds
 * the resulting `savedQueryId` to the tab, renames the tab to match,
 * and marks it clean. Throws if there is no active tab.
 */
export async function saveActiveTabAs(
  name: string,
  tags?: string[],
): Promise<savedQueries.SavedQuery> {
  const tabId = getActiveTabId();
  if (tabId === null) {
    throw new Error("No active tab to save");
  }
  const state = useStore.getState();
  const tab = state.tabs.find((t) => t.id === tabId);
  if (!tab) {
    throw new Error("Active tab not found");
  }

  const record = await savedQueries.create({
    name,
    body: tab.body,
    tags: tags ?? [],
  });

  const store = useStore.getState();
  store.bindSavedQueryId(tab.id, record.id);
  store.renameTab(tab.id, record.name);
  store.markClean(tab.id);
  emitChange();
  return record;
}

/**
 * Open a saved query in a new tab — or, if any open tab is already
 * bound to it, focus that one instead. The tab always appears in the
 * active cell's editor strip via `openTabInCell`.
 */
export async function openSavedQuery(id: string): Promise<void> {
  const state = useStore.getState();
  const existing = state.tabs.find((t) => t.savedQueryId === id);
  if (existing) {
    focusTabInWorkspace(existing.id);
    return;
  }
  const record = await savedQueries.get(id);
  if (!record) return;
  openTabInCell({
    name: record.name,
    body: record.body,
    savedQueryId: record.id,
  });
}

/**
 * Rename a saved query. Any open tab bound to it is renamed in
 * lock-step so the editor tab strip stays in sync.
 */
export async function renameSavedQuery(
  id: string,
  name: string,
): Promise<void> {
  await savedQueries.update(id, { name });
  const state = useStore.getState();
  for (const tab of state.tabs) {
    if (tab.savedQueryId === id) {
      state.renameTab(tab.id, name);
    }
  }
  emitChange();
}

/**
 * Delete a saved query. Any open tab bound to it has its binding
 * cleared (the tab itself stays open and keeps its dirty state).
 */
export async function deleteSavedQuery(id: string): Promise<void> {
  await savedQueries.remove(id);
  const state = useStore.getState();
  for (const tab of state.tabs) {
    if (tab.savedQueryId === id) {
      state.bindSavedQueryId(tab.id, undefined);
    }
  }
  emitChange();
}

/**
 * Create a copy of an existing saved query under a `(copy)` suffix.
 * Returns the new record. Does not open it.
 */
export async function duplicateSavedQuery(
  id: string,
): Promise<savedQueries.SavedQuery | null> {
  const source = await savedQueries.get(id);
  if (!source) return null;
  const record = await savedQueries.create({
    name: `${source.name} (copy)`,
    body: source.body,
    tags: source.tags,
  });
  emitChange();
  return record;
}
