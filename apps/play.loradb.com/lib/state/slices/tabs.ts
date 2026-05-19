/**
 * Tabs slice — owns the master list of editor tabs.
 *
 * The slice no longer carries an `activeTabId` field. "Which tab is the
 * user looking at" is derived from the workspace tree (see
 * `lib/state/selectors.ts#useActiveTabId`), so the only state we keep
 * here is the tab records themselves. Tab membership in editor panes
 * is owned by the layout slice's per-view `tabIds` strips.
 *
 * Tab IDs are ULIDs so they sort chronologically. Newly opened tabs
 * default to clean (`dirty: false`); any subsequent `setBody` flips the
 * dirty flag. The slice has no knowledge of saved-query backing —
 * `markClean` is called by the action that performs the save.
 */

import type { StateCreator } from "zustand";

import { ulid } from "@/lib/util/id";
import {
  gcClosedTab as gcClosedTabInTree,
  type PanelNode,
} from "@/lib/state/workspace/tree";

export interface EditorTab {
  id: string;
  name: string;
  body: string;
  /**
   * Raw JSON source for the per-tab `$param` payload. Stored as a
   * string (not a parsed object) so partial edits don't get
   * round-tripped or lost mid-keystroke. Defaults to `"{}"`.
   *
   * The driver consumes this only at run time — `runActiveTab`
   * parses it, validates against the editor's detected params,
   * and surfaces a confirm toast for missing-required.
   */
  params: string;
  dirty: boolean;
  savedQueryId?: string;
  createdAt: number;
}

export interface SerializedTab {
  id: string;
  name: string;
  body: string;
  /** Hydration normalises a missing field to `"{}"`. */
  params?: string;
  savedQueryId?: string;
  createdAt: number;
}

export interface TabsSlice {
  tabs: EditorTab[];
  openTab(input?: {
    name?: string;
    body?: string;
    params?: string;
    savedQueryId?: string;
  }): string;
  closeTab(id: string): void;
  renameTab(id: string, name: string): void;
  setBody(id: string, body: string): void;
  /** Replace the raw JSON params source for a tab. Flips `dirty`. */
  setParams(id: string, params: string): void;
  markClean(id: string): void;
  bindSavedQueryId(id: string, savedQueryId: string | undefined): void;
  reorderTab(fromIndex: number, toIndex: number): void;
  hydrateTabs(tabs: SerializedTab[]): void;
}

/** Default empty payload — kept as a constant so call sites are explicit. */
export const DEFAULT_PARAMS = "{}";

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
> = (set) => ({
  tabs: [],

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
        params: input?.params ?? DEFAULT_PARAMS,
        dirty: false,
        savedQueryId: input?.savedQueryId,
        createdAt: Date.now(),
      };
      state.tabs.push(tab);
    });
    return id;
  },

  closeTab(id) {
    set((state) => {
      const index = state.tabs.findIndex((t) => t.id === id);
      if (index === -1) return;
      state.tabs.splice(index, 1);
      // Both slices share the merged immer draft at runtime, but the
      // StateCreator generic is narrowed to TabsSlice — the cast is the
      // cheapest way to reach the sibling records without coupling the
      // slice types. We drop the result entry and GC every workspace
      // reference (editor strips + pinned result views) so closing a tab
      // here can never leave dangling pointers.
      const sibling = state as unknown as {
        results?: Record<string, unknown>;
        workspace?: PanelNode;
        paramsByTab?: Record<string, unknown>;
      };
      if (sibling.results && id in sibling.results) {
        delete sibling.results[id];
      }
      if (sibling.paramsByTab && id in sibling.paramsByTab) {
        delete sibling.paramsByTab[id];
      }
      if (sibling.workspace) {
        const next = gcClosedTabInTree(sibling.workspace, id);
        if (next) sibling.workspace = next;
      }
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

  setParams(id, params) {
    set((state) => {
      const tab = state.tabs.find((t) => t.id === id);
      if (!tab) return;
      if (tab.params === params) return;
      tab.params = params;
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

  hydrateTabs(tabs) {
    set((state) => {
      state.tabs = tabs.map<EditorTab>((t) => ({
        id: t.id,
        name: t.name,
        body: t.body,
        params: t.params ?? DEFAULT_PARAMS,
        dirty: false,
        savedQueryId: t.savedQueryId,
        createdAt: t.createdAt,
      }));
    });
  },
});
