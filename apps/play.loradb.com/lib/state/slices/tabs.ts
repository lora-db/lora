/**
 * Tabs slice — owns the list of open editor tabs and which one is active.
 *
 * Tab IDs are ULIDs so they sort chronologically. Newly opened tabs default
 * to clean (`dirty: false`); any subsequent `setBody` flips the dirty flag.
 * The slice has no knowledge of saved-query backing — `markClean` is called
 * by the action that performs the save.
 */

import type { StateCreator } from "zustand";

import { ulid } from "@/lib/util/id";

export interface EditorTab {
  id: string;
  name: string;
  body: string;
  dirty: boolean;
  savedQueryId?: string;
  createdAt: number;
}

export interface SerializedTab {
  id: string;
  name: string;
  body: string;
  savedQueryId?: string;
  createdAt: number;
}

export interface TabsSlice {
  tabs: EditorTab[];
  activeTabId: string | null;
  openTab(input?: { name?: string; body?: string; savedQueryId?: string }): string;
  closeTab(id: string): void;
  setActiveTab(id: string): void;
  renameTab(id: string, name: string): void;
  setBody(id: string, body: string): void;
  markClean(id: string): void;
  bindSavedQueryId(id: string, savedQueryId: string | undefined): void;
  reorderTab(fromIndex: number, toIndex: number): void;
  hydrateTabs(tabs: SerializedTab[], activeTabId: string | null): void;
}

/**
 * Returns the next unused `"Query N"` name for an untitled tab, scanning the
 * existing list so we never reuse a number while it's still on screen.
 */
function nextUntitledName(tabs: ReadonlyArray<EditorTab>): string {
  let max = 0;
  for (const tab of tabs) {
    const match = /^Query (\d+)$/.exec(tab.name);
    if (match) {
      const n = Number.parseInt(match[1] ?? "0", 10);
      if (Number.isFinite(n) && n > max) max = n;
    }
  }
  return `Query ${max + 1}`;
}

export const createTabsSlice: StateCreator<
  TabsSlice,
  [["zustand/immer", never]],
  [],
  TabsSlice
> = (set, get) => ({
  tabs: [],
  activeTabId: null,

  openTab(input) {
    const id = ulid();
    set((state) => {
      const name =
        input?.name && input.name.length > 0
          ? input.name
          : nextUntitledName(state.tabs);
      const tab: EditorTab = {
        id,
        name,
        body: input?.body ?? "",
        dirty: false,
        savedQueryId: input?.savedQueryId,
        createdAt: Date.now(),
      };
      state.tabs.push(tab);
      state.activeTabId = id;
    });
    return id;
  },

  closeTab(id) {
    set((state) => {
      const index = state.tabs.findIndex((t) => t.id === id);
      if (index === -1) return;
      state.tabs.splice(index, 1);
      // Drop the orphaned result entry. Both slices share the merged immer
      // draft at runtime, but the StateCreator generic is narrowed to
      // TabsSlice — the cast is the cheapest way to reach the sibling
      // record without coupling the slice types.
      const sibling = state as unknown as { results?: Record<string, unknown> };
      if (sibling.results && id in sibling.results) {
        delete sibling.results[id];
      }
      if (state.activeTabId === id) {
        // Prefer the next tab to the right; fall back to the previous one.
        const next = state.tabs[index] ?? state.tabs[index - 1] ?? null;
        state.activeTabId = next ? next.id : null;
      }
    });
  },

  setActiveTab(id) {
    const exists = get().tabs.some((t) => t.id === id);
    if (!exists) return;
    set((state) => {
      state.activeTabId = id;
    });
  },

  renameTab(id, name) {
    set((state) => {
      const tab = state.tabs.find((t) => t.id === id);
      if (tab) tab.name = name;
    });
  },

  setBody(id, body) {
    set((state) => {
      const tab = state.tabs.find((t) => t.id === id);
      if (!tab) return;
      // CodeMirror occasionally emits identical-value transactions
      // (composition end, IME flush, undo of a no-op). Skipping the
      // mutation here keeps `dirty` honest and avoids spurious
      // persistence writes.
      if (tab.body === body) return;
      tab.body = body;
      // The slice has no diff-against-saved knowledge; callers signal clean
      // state explicitly via `markClean` after a successful save.
      tab.dirty = true;
    });
  },

  markClean(id) {
    set((state) => {
      const tab = state.tabs.find((t) => t.id === id);
      if (tab) tab.dirty = false;
    });
  },

  bindSavedQueryId(id, savedQueryId) {
    set((state) => {
      const tab = state.tabs.find((t) => t.id === id);
      if (!tab) return;
      tab.savedQueryId = savedQueryId;
    });
  },

  reorderTab(fromIndex, toIndex) {
    set((state) => {
      const len = state.tabs.length;
      if (
        fromIndex === toIndex ||
        fromIndex < 0 ||
        fromIndex >= len ||
        toIndex < 0 ||
        toIndex >= len
      ) {
        return;
      }
      const [moved] = state.tabs.splice(fromIndex, 1);
      if (!moved) return;
      state.tabs.splice(toIndex, 0, moved);
    });
  },

  hydrateTabs(tabs, activeTabId) {
    set((state) => {
      state.tabs = tabs.map<EditorTab>((t) => ({
        id: t.id,
        name: t.name,
        body: t.body,
        dirty: false,
        savedQueryId: t.savedQueryId,
        createdAt: t.createdAt,
      }));
      // Only honour the persisted active id if it still resolves to a tab.
      const stillExists =
        activeTabId !== null && state.tabs.some((t) => t.id === activeTabId);
      state.activeTabId = stillExists
        ? activeTabId
        : (state.tabs[0]?.id ?? null);
    });
  },
});
