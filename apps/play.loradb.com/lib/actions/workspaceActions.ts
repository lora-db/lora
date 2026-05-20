"use client";

/**
 * Imperative wrappers for window-management actions — split, close,
 * reorient, move-view-between-panes, etc. Callable from hotkeys,
 * Spotlight commands, and context menus.
 */

import { useStore } from "@/lib/state/store";
import type {
  Placement,
  ResultTab,
  SplitDirection,
} from "@/lib/state/slices/layout";
import {
  findLeaf,
  findViewLeaf,
  flatLeafIds,
  iterLeaves,
  makeView,
  resolveActiveTabId,
} from "@/lib/state/workspace/tree";

export function getActivePaneId(): string {
  return useStore.getState().activePaneId;
}

export function focusPane(paneId: string): void {
  useStore.getState().setActivePane(paneId);
}

export function focusNextPane(): void {
  const state = useStore.getState();
  const ids = flatLeafIds(state.workspace);
  if (ids.length <= 1) return;
  const idx = ids.indexOf(state.activePaneId);
  const next = ids[(idx + 1 + ids.length) % ids.length];
  if (next) state.setActivePane(next);
}

export function focusPrevPane(): void {
  const state = useStore.getState();
  const ids = flatLeafIds(state.workspace);
  if (ids.length <= 1) return;
  const idx = ids.indexOf(state.activePaneId);
  const prev = ids[(idx - 1 + ids.length) % ids.length];
  if (prev) state.setActivePane(prev);
}

/**
 * Cycle the active view inside the focused pane (e.g. switch from an
 * editor view to its sibling result view). No-op when the pane has
 * only one view.
 */
export function cycleViewInActivePane(): void {
  const state = useStore.getState();
  const leaf = findLeaf(state.workspace, state.activePaneId);
  if (!leaf || leaf.views.length <= 1) return;
  const idx = leaf.views.findIndex((v) => v.id === leaf.activeViewId);
  const next = leaf.views[(idx + 1 + leaf.views.length) % leaf.views.length];
  if (next) state.setActiveView(leaf.id, next.id);
}

/**
 * Split the workspace, creating a new self-contained query pane
 * (editor + result, bound together) alongside the existing one.
 *
 * `direction`:
 *   - "row"    → new pane sits to the right of the current one
 *   - "column" → new pane stacks below the current one
 *
 * The new pane gets its own fresh tab (cloned body from the source
 * tab) so the two panes don't share editor state. Returns the new
 * leaf id so the caller can focus it.
 */
export function splitActivePane(
  direction: SplitDirection,
  placement: Placement = "after",
): string | null {
  const state = useStore.getState();
  const paneId = state.activePaneId;

  // Source tab for body cloning — prefer the active pane's active view.
  const activeLeaf = findLeaf(state.workspace, paneId);
  const activeView = activeLeaf?.views.find(
    (v) => v.id === activeLeaf.activeViewId,
  );
  const sourceTabId = activeView?.tabId ?? getActiveTabId();
  const sourceTab = sourceTabId
    ? state.tabs.find((t) => t.id === sourceTabId)
    : null;

  const newTabId = state.openTab({ body: sourceTab?.body ?? "" });
  const newView = makeView({
    kind: "query",
    tabId: newTabId,
    tabIds: [newTabId],
    resultTab: "graph",
  });
  return state.splitPane(paneId, direction, placement, newView);
}

export function closeActivePane(): void {
  closePaneById(getActivePaneId());
}

/**
 * Close `paneId` and shift keyboard focus to the survivor editor so
 * the user isn't left typing into nothing. Safe to call from buttons,
 * menus, and hotkeys.
 */
export function closePaneById(paneId: string): void {
  const state = useStore.getState();
  const ids = flatLeafIds(state.workspace);
  if (ids.length <= 1) return; // refuse to empty
  state.closePane(paneId);
  // The DOM hasn't repainted yet — defer focus to the next frame so
  // CodeMirror has remounted in the new active leaf before we focus.
  if (typeof window !== "undefined") {
    requestAnimationFrame(() => {
      const el = document.querySelector<HTMLElement>(".cm-content");
      if (el) el.focus();
    });
  }
}

/** Toggle the orientation of the root group (or, if the root is a leaf, no-op). */
export function toggleRootOrientation(): void {
  useStore.getState().toggleRootDirection();
}

/** Toggle the orientation of the group that immediately contains the given pane. */
export function toggleParentOrientation(paneId: string): void {
  const state = useStore.getState();
  if (state.workspace.type !== "group") return;
  // Find the immediate parent group of paneId, then flip its direction.
  function findParent(
    node: typeof state.workspace,
    target: string,
  ): { id: string; direction: SplitDirection } | null {
    if (node.type === "leaf") return null;
    for (const child of node.children) {
      if (child.type === "leaf" && child.id === target) {
        return { id: node.id, direction: node.direction };
      }
      const nested = findParent(child as typeof node, target);
      if (nested) return nested;
    }
    return null;
  }
  const parent = findParent(state.workspace, paneId);
  if (!parent) return;
  state.setGroupDirection(
    parent.id,
    parent.direction === "row" ? "column" : "row",
  );
}

/**
 * Set the result inner-tab on the active pane's view, falling back to
 * the first query view anywhere if the active pane can't be found.
 */
export function setActiveResultTab(resultTab: ResultTab): string | null {
  const state = useStore.getState();
  const activeLeaf = findLeaf(state.workspace, state.activePaneId);
  const target =
    activeLeaf?.views.find((v) => v.id === activeLeaf.activeViewId) ??
    activeLeaf?.views.find((v) => v.kind === "query");
  if (target) {
    state.setResultTabForView(target.id, resultTab);
    return target.id;
  }
  for (const leaf of iterLeaves(state.workspace)) {
    const v = leaf.views.find((view) => view.kind === "query");
    if (v) {
      state.setResultTabForView(v.id, resultTab);
      return v.id;
    }
  }
  return null;
}

/** Toggle (or set) the active pane's result region minimize state. */
export function toggleActiveResultMinimized(): void {
  const state = useStore.getState();
  const leaf = findLeaf(state.workspace, state.activePaneId);
  if (!leaf) return;
  const view =
    leaf.views.find((v) => v.id === leaf.activeViewId) ?? leaf.views[0];
  if (!view) return;
  state.setResultMinimizedForView(view.id, !(view.resultMinimized ?? false));
}

/**
 * Workspace-derived "the tab the user is currently looking at."
 * Thin wrapper around the pure resolver in `lib/state/workspace/tree.ts`.
 */
export function getActiveTabId(): string | null {
  const state = useStore.getState();
  return resolveActiveTabId(state.workspace, state.activePaneId);
}

/**
 * Activate `tabId` in some query pane. Preference order:
 *   1. The active pane if it already contains the tab.
 *   2. Any other pane whose strip already lists the tab.
 *   3. The first query pane (the tab is added to its strip).
 */
export function focusTabInWorkspace(tabId: string): void {
  const state = useStore.getState();
  const activeLeaf = findLeaf(state.workspace, state.activePaneId);
  if (activeLeaf) {
    for (const v of activeLeaf.views) {
      if ((v.tabIds ?? []).includes(tabId)) {
        state.setViewTabId(v.id, tabId);
        return;
      }
    }
  }
  for (const leaf of iterLeaves(state.workspace)) {
    for (const v of leaf.views) {
      if ((v.tabIds ?? []).includes(tabId)) {
        state.setActivePane(leaf.id);
        state.setViewTabId(v.id, tabId);
        return;
      }
    }
  }
  for (const leaf of iterLeaves(state.workspace)) {
    const v = leaf.views.find((view) => view.kind === "query");
    if (v) {
      state.setActivePane(leaf.id);
      state.addTabToEditorView(v.id, tabId);
      return;
    }
  }
}

/** GC any workspace views pinned to a tab that no longer exists. */
export function gcWorkspaceForTab(tabId: string): void {
  useStore.getState().gcClosedTab(tabId);
}

export { findViewLeaf };
