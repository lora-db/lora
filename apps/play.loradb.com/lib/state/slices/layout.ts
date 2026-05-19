/**
 * Layout slice — workbench chrome + the recursive workspace tree.
 *
 * The tree is the source of truth for which views are visible, how they
 * are split, and which inner Result tab (Graph/Table/JSON/Plan) each
 * Result view shows. Sidebar visibility/width and the activity-bar
 * selection live alongside as plain fields.
 *
 * Mutators forward to the pure helpers in `../workspace/tree.ts` and
 * keep `activePaneId` honest after each transformation.
 */

import type { StateCreator } from "zustand";

import {
  buildDefaultWorkspace,
  DEFAULT_SIDEBAR_WIDTH,
  MAX_SIDEBAR_WIDTH,
  MIN_SIDEBAR_WIDTH,
  migrateLayout,
} from "@/lib/state/workspace/default";
import {
  addTabToEditorView,
  closeLeaf,
  countQueryViews,
  findLeaf,
  findViewLeaf,
  firstLeaf,
  gcClosedTab,
  insertView,
  makeView,
  moveView,
  removeTabFromEditorView,
  removeView,
  reorderTabInEditorView,
  setActiveView,
  setGroupDirection,
  setGroupSizes,
  setViewEditorSizePct,
  setViewParamsPanelOpen,
  setViewParamsPanelSize,
  setViewResultMinimized,
  setViewResultTab,
  setViewTabId,
  splitLeaf,
  type PanelKind,
  type PanelNode,
  type PanelView,
  type Placement,
  type SplitDirection,
} from "@/lib/state/workspace/tree";

export type ActivitySection =
  | "queries"
  | "schema"
  | "snapshots"
  | "history"
  | "settings";

export type ResultTab = "graph" | "table" | "json" | "plan";

export interface SerializedLayout {
  activitySection: ActivitySection;
  sidebarOpen: boolean;
  sidebarWidth: number;
  workspace: PanelNode;
  activePaneId: string;
}

/** Default width of the Params panel column, as a percentage of the editor row. */
export const DEFAULT_PARAMS_PANEL_SIZE = 30;

export interface LayoutSlice extends SerializedLayout {
  setActivitySection(section: ActivitySection): void;
  toggleSidebar(): void;
  setSidebarWidth(px: number): void;
  /** Toggle the Params panel sidecar inside a single editor view. */
  setParamsPanelOpenForView(viewId: string, open: boolean): void;
  /** Adjust the Params panel column width for a single view. */
  setParamsPanelSizeForView(viewId: string, pct: number): void;
  setActivePane(paneId: string): void;
  setActiveView(paneId: string, viewId: string): void;
  splitPane(
    paneId: string,
    direction: SplitDirection,
    placement?: Placement,
    newView?: PanelView,
  ): string | null;
  closePane(paneId: string): void;
  setGroupDirection(idOnOrInGroup: string, direction: SplitDirection): void;
  toggleRootDirection(): void;
  setGroupSizes(groupId: string, sizes: number[]): void;
  setResultTabForView(viewId: string, resultTab: ResultTab): void;
  setViewTabId(viewId: string, tabId: string | undefined): void;
  addView(paneId: string, kind: PanelKind, opts?: { tabId?: string; resultTab?: ResultTab }): string;
  removeView(viewId: string): void;
  moveView(viewId: string, toPaneId: string, toIndex?: number): void;
  gcClosedTab(tabId: string): void;
  addTabToEditorView(viewId: string, tabId: string, index?: number): void;
  removeTabFromEditorView(viewId: string, tabId: string): void;
  reorderTabInEditorView(viewId: string, fromIndex: number, toIndex: number): void;
  setResultMinimizedForView(viewId: string, minimized: boolean): void;
  setEditorSizePctForView(viewId: string, pct: number): void;
  /**
   * Replace the workspace tree with a freshly-built default layout
   * (one editor pane on top, one result pane below). Existing tabs
   * are re-attached to the new editor view's strip so the user
   * doesn't lose any open queries.
   */
  resetLayout(): void;
  /**
   * Replace the workspace tree atomically. Used by higher-level
   * compositional actions (e.g. workspace-level split that creates a
   * paired editor+result column in one step).
   */
  replaceWorkspace(tree: PanelNode, opts?: { activePaneId?: string }): void;
  hydrateLayout(layout: SerializedLayout): void;
}

const seed = buildDefaultWorkspace();

export const DEFAULT_LAYOUT: SerializedLayout = {
  activitySection: "queries",
  sidebarOpen: true,
  sidebarWidth: DEFAULT_SIDEBAR_WIDTH,
  workspace: seed.workspace,
  activePaneId: seed.editorLeafId,
};

function clampSidebar(n: number): number {
  return Math.min(MAX_SIDEBAR_WIDTH, Math.max(MIN_SIDEBAR_WIDTH, Math.round(n)));
}

function nextActivePaneAfterClose(tree: PanelNode | null, removedId: string, current: string): string | null {
  if (!tree) return null;
  if (current !== removedId && findLeaf(tree, current)) return current;
  // Prefer the first remaining leaf id.
  return firstLeaf(tree).id;
}

/**
 * Pick a query view that survives after `closingLeafId` is removed.
 * Closing the leaf strands every tab in its strip, so we move them to
 * a sibling pane to keep them reachable.
 */
function findSurvivorQueryViewId(tree: PanelNode, closingLeafId: string): string | null {
  function walk(node: PanelNode): string | null {
    if (node.type === "leaf") {
      if (node.id === closingLeafId) return null;
      const v = node.views.find((view) => view.kind === "query");
      return v ? v.id : null;
    }
    for (const child of node.children) {
      const found = walk(child);
      if (found) return found;
    }
    return null;
  }
  return walk(tree);
}

/** True iff `tabId` appears in any query view OTHER than `viewId`. */
function tabIsOpenInOtherQueryView(tree: PanelNode, viewId: string, tabId: string): boolean {
  function walk(node: PanelNode): boolean {
    if (node.type === "leaf") {
      for (const v of node.views) {
        if (v.id === viewId) continue;
        if ((v.tabIds ?? []).includes(tabId)) return true;
      }
      return false;
    }
    for (const child of node.children) if (walk(child)) return true;
    return false;
  }
  return walk(tree);
}

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

  toggleSidebar() {
    set((state) => {
      state.sidebarOpen = !state.sidebarOpen;
    });
  },

  setSidebarWidth(px) {
    set((state) => {
      state.sidebarWidth = clampSidebar(px);
    });
  },

  setParamsPanelOpenForView(viewId, open) {
    set((state) => {
      state.workspace = setViewParamsPanelOpen(state.workspace, viewId, open);
    });
  },

  setParamsPanelSizeForView(viewId, pct) {
    set((state) => {
      state.workspace = setViewParamsPanelSize(state.workspace, viewId, pct);
    });
  },

  setActivePane(paneId) {
    set((state) => {
      const leaf = findLeaf(state.workspace, paneId);
      if (!leaf) return;
      state.activePaneId = paneId;
    });
  },

  setActiveView(paneId, viewId) {
    set((state) => {
      state.workspace = setActiveView(state.workspace, paneId, viewId);
      state.activePaneId = paneId;
    });
  },

  splitPane(paneId, direction, placement = "after", newView) {
    let newLeafId: string | null = null;
    set((state) => {
      const result = splitLeaf(state.workspace, paneId, direction, placement, newView);
      if (!result) return;
      state.workspace = result.tree;
      state.activePaneId = result.newLeafId;
      newLeafId = result.newLeafId;
    });
    return newLeafId;
  },

  closePane(paneId) {
    set((state) => {
      const leaf = findLeaf(state.workspace, paneId);
      if (!leaf) return;
      // The workspace must always carry at least one query pane.
      const closing = leaf.views.filter((v) => v.kind === "query").length;
      if (closing > 0 && countQueryViews(state.workspace) - closing < 1) {
        return;
      }

      // Move tabs that only live in this pane's strip to a survivor so
      // the user doesn't lose access to them when the leaf collapses.
      const survivorViewId = findSurvivorQueryViewId(state.workspace, leaf.id);
      if (survivorViewId) {
        const orphaned: string[] = [];
        for (const view of leaf.views) {
          for (const tabId of view.tabIds ?? []) {
            if (!tabIsOpenInOtherQueryView(state.workspace, view.id, tabId)) {
              orphaned.push(tabId);
            }
          }
        }
        for (const tabId of orphaned) {
          state.workspace = addTabToEditorView(state.workspace, survivorViewId, tabId);
        }
      }

      const next = closeLeaf(state.workspace, paneId);
      if (!next) return;
      state.workspace = next;
      const reassigned = nextActivePaneAfterClose(next, paneId, state.activePaneId);
      if (reassigned) state.activePaneId = reassigned;
    });
  },

  setGroupDirection(idOnOrInGroup, direction) {
    set((state) => {
      state.workspace = setGroupDirection(state.workspace, idOnOrInGroup, direction);
    });
  },

  toggleRootDirection() {
    set((state) => {
      if (state.workspace.type !== "group") return;
      const next: SplitDirection = state.workspace.direction === "row" ? "column" : "row";
      state.workspace = { ...state.workspace, direction: next };
    });
  },

  setGroupSizes(groupId, sizes) {
    set((state) => {
      state.workspace = setGroupSizes(state.workspace, groupId, sizes);
    });
  },

  setResultTabForView(viewId, resultTab) {
    set((state) => {
      state.workspace = setViewResultTab(state.workspace, viewId, resultTab);
    });
  },

  setViewTabId(viewId, tabId) {
    set((state) => {
      state.workspace = setViewTabId(state.workspace, viewId, tabId);
    });
  },

  addView(paneId, kind, opts) {
    const view = makeView({ kind, ...(opts ?? {}) });
    set((state) => {
      state.workspace = insertView(state.workspace, paneId, view);
      state.activePaneId = paneId;
    });
    return view.id;
  },

  removeView(viewId) {
    set((state) => {
      const sourceLeaf = findViewLeaf(state.workspace, viewId);
      if (!sourceLeaf) return;
      const target = sourceLeaf.views.find((v) => v.id === viewId);
      if (!target) return;
      // Workspace must always have at least one query pane.
      if (countQueryViews(state.workspace) <= 1) return;
      const next = removeView(state.workspace, viewId);
      if (!next) return;
      state.workspace = next;
      if (!findLeaf(next, sourceLeaf.id)) {
        state.activePaneId = firstLeaf(next).id;
      }
    });
  },

  moveView(viewId, toPaneId, toIndex) {
    set((state) => {
      const next = moveView(state.workspace, viewId, toPaneId, toIndex);
      if (!next) return;
      state.workspace = next;
      if (findLeaf(next, toPaneId)) state.activePaneId = toPaneId;
    });
  },

  gcClosedTab(tabId) {
    set((state) => {
      const next = gcClosedTab(state.workspace, tabId);
      if (!next) return; // workspace would collapse to nothing — keep current
      state.workspace = next;
      const reassigned = nextActivePaneAfterClose(next, state.activePaneId, state.activePaneId);
      if (reassigned) state.activePaneId = reassigned;
    });
  },

  addTabToEditorView(viewId, tabId, index) {
    set((state) => {
      state.workspace = addTabToEditorView(state.workspace, viewId, tabId, index);
    });
  },

  removeTabFromEditorView(viewId, tabId) {
    set((state) => {
      state.workspace = removeTabFromEditorView(state.workspace, viewId, tabId);
    });
  },

  reorderTabInEditorView(viewId, fromIndex, toIndex) {
    set((state) => {
      state.workspace = reorderTabInEditorView(state.workspace, viewId, fromIndex, toIndex);
    });
  },

  setResultMinimizedForView(viewId, minimized) {
    set((state) => {
      state.workspace = setViewResultMinimized(state.workspace, viewId, minimized);
    });
  },

  setEditorSizePctForView(viewId, pct) {
    set((state) => {
      state.workspace = setViewEditorSizePct(state.workspace, viewId, pct);
    });
  },

  replaceWorkspace(tree, opts) {
    set((state) => {
      state.workspace = tree;
      const target = opts?.activePaneId;
      if (target && findLeaf(tree, target)) {
        state.activePaneId = target;
      } else if (!findLeaf(tree, state.activePaneId)) {
        state.activePaneId = firstLeaf(tree).id;
      }
    });
  },

  resetLayout() {
    set((state) => {
      // Carry every existing tab record over into the new pane's
      // strip so the user doesn't lose any open queries on reset.
      const sibling = state as unknown as { tabs?: { id: string }[] };
      const tabIds: string[] = (sibling.tabs ?? []).map((t) => t.id);
      const seed = buildDefaultWorkspace({
        editorTabIds: tabIds,
        ...(tabIds[0] !== undefined ? { editorActiveTabId: tabIds[0] } : {}),
      });
      state.workspace = seed.workspace;
      state.activePaneId = seed.editorLeafId;
      state.sidebarWidth = DEFAULT_SIDEBAR_WIDTH;
    });
  },

  hydrateLayout(layout) {
    set((state) => {
      const migrated = migrateLayout(layout);
      state.activitySection = migrated.activitySection;
      state.sidebarOpen = migrated.sidebarOpen;
      state.sidebarWidth = clampSidebar(migrated.sidebarWidth);
      state.workspace = migrated.workspace;
      state.activePaneId = migrated.activePaneId;
    });
  },
});

// Re-export the surfaces other modules consume.
export type {
  PanelGroup,
  PanelKind,
  PanelLeaf,
  PanelNode,
  PanelView,
  Placement,
  SplitDirection,
} from "@/lib/state/workspace/tree";
export { flatLeafIds, findLeaf, firstLeaf } from "@/lib/state/workspace/tree";
