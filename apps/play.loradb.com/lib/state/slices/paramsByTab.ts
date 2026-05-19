/**
 * Params bridge slice — caches the list of `$param` names the Cypher
 * editor's outline reports for each open tab.
 *
 * The editor pane pushes here via `setDetectedParams` whenever its
 * outline updates; consumers (ParamsPanel, runActiveTab, status bar)
 * read by `tabId`. The slice intentionally lives outside of `tabs`
 * because the data is derived (not user-authored) — keeping it
 * separate avoids dirty-flagging a tab whenever the WASM analyser
 * re-runs.
 *
 * Not persisted: the editor re-derives on hydration anyway.
 */

import type { StateCreator } from "zustand";

export interface ParamsByTabSlice {
  /** Map of tabId → detected `$param` names (in declaration order). */
  paramsByTab: Record<string, readonly string[]>;
  /** Replace the cached params list for a tab. No-op when unchanged. */
  setDetectedParams(tabId: string, params: readonly string[]): void;
  /** Drop a tab's entry (called when a tab closes). */
  clearDetectedParams(tabId: string): void;
}

function sameList(a: readonly string[], b: readonly string[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

export const createParamsByTabSlice: StateCreator<
  ParamsByTabSlice,
  [["zustand/immer", never]],
  [],
  ParamsByTabSlice
> = (set) => ({
  paramsByTab: {},

  setDetectedParams(tabId, params) {
    set((state) => {
      const prev = state.paramsByTab[tabId];
      if (prev && sameList(prev, params)) return;
      state.paramsByTab[tabId] = [...params];
    });
  },

  clearDetectedParams(tabId) {
    set((state) => {
      if (tabId in state.paramsByTab) {
        delete state.paramsByTab[tabId];
      }
    });
  },
});
