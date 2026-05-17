/**
 * Layout slice — UI chrome state (active activity-bar section, panel sizes,
 * which result tab is in front, sidebar visibility).
 *
 * The shape is reused as `SerializedLayout` so persistence can store it
 * verbatim with no transform.
 */

import type { StateCreator } from "zustand";

export type ActivitySection =
  | "queries"
  | "schema"
  | "snapshots"
  | "history"
  | "settings";

export type ResultTab = "graph" | "table" | "json" | "plan";

export interface SerializedLayout {
  activitySection: ActivitySection;
  panelSizes: Record<string, number>;
  resultTab: ResultTab;
  sidebarOpen: boolean;
}

export interface LayoutSlice extends SerializedLayout {
  setActivitySection(section: ActivitySection): void;
  setPanelSize(key: string, size: number): void;
  setResultTab(tab: ResultTab): void;
  toggleSidebar(): void;
  hydrateLayout(layout: SerializedLayout): void;
}

export const DEFAULT_LAYOUT: SerializedLayout = {
  activitySection: "queries",
  panelSizes: {},
  resultTab: "graph",
  sidebarOpen: true,
};

export const createLayoutSlice: StateCreator<
  LayoutSlice,
  [["zustand/immer", never]],
  [],
  LayoutSlice
> = (set) => ({
  ...DEFAULT_LAYOUT,

  setActivitySection(section) {
    set((state) => {
      state.activitySection = section;
    });
  },

  setPanelSize(key, size) {
    set((state) => {
      state.panelSizes[key] = size;
    });
  },

  setResultTab(tab) {
    set((state) => {
      state.resultTab = tab;
    });
  },

  toggleSidebar() {
    set((state) => {
      state.sidebarOpen = !state.sidebarOpen;
    });
  },

  hydrateLayout(layout) {
    set((state) => {
      state.activitySection = layout.activitySection;
      state.panelSizes = { ...layout.panelSizes };
      state.resultTab = layout.resultTab;
      state.sidebarOpen = layout.sidebarOpen;
    });
  },
});
