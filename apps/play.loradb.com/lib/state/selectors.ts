"use client";

/**
 * Narrow hooks built on top of `useStore` so components subscribe only
 * to the slice of state they actually render.
 *
 * "Active tab" is derived from the workspace tree — there is no
 * `activeTabId` field anymore. The active editor view's `tabId`
 * (resolved through the active pane's enclosing cell) is the single
 * source of truth.
 */

import { useMemo } from "react";

import { useStore, type Store } from "./store";
import type { EditorTab } from "./slices/tabs";
import type { TabResult } from "./slices/results";
import type { ActivitySection, PanelView } from "./slices/layout";
import {
  findLeaf,
  findViewLeaf,
  firstLeaf,
  resolveActiveTabId,
} from "./workspace/tree";

function resolveActiveTabIdFromStore(s: Store): string | null {
  return resolveActiveTabId(s.workspace, s.activePaneId);
}

export function useActiveTabId(): string | null {
  return useStore(resolveActiveTabIdFromStore);
}

/** Returns the currently-active editor tab, or `null` if there isn't one. */
export function useActiveTab(): EditorTab | null {
  return useStore((s: Store) => {
    const id = resolveActiveTabIdFromStore(s);
    if (!id) return null;
    return s.tabs.find((t) => t.id === id) ?? null;
  });
}

/** Returns the result associated with the active tab, if any. */
export function useActiveResult(): TabResult {
  return useStore((s: Store) => {
    const id = resolveActiveTabIdFromStore(s);
    if (!id) return undefined;
    return s.results[id];
  });
}

/** Returns a tab by id (or null if missing). Tracks only the matching tab. */
export function useTabById(tabId: string | undefined): EditorTab | null {
  return useStore((s: Store) => {
    if (!tabId) return null;
    return s.tabs.find((t) => t.id === tabId) ?? null;
  });
}

/**
 * Resolve the tab a view binds to. Editor views always carry an
 * explicit `tabId`; result views may be unpinned in legacy layouts,
 * in which case we fall back to the workspace-active tab.
 */
export function useViewTab(view: PanelView | null): EditorTab | null {
  return useStore((s: Store) => {
    if (!view) return null;
    const id = view.tabId ?? resolveActiveTabIdFromStore(s);
    if (!id) return null;
    return s.tabs.find((t) => t.id === id) ?? null;
  });
}

/** Returns the result associated with a view's resolved tab id. */
export function useViewResult(view: PanelView | null): TabResult {
  return useStore((s: Store) => {
    if (!view) return undefined;
    const id = view.tabId ?? resolveActiveTabIdFromStore(s);
    if (!id) return undefined;
    return s.results[id];
  });
}

/** Returns the list of tab IDs in their current order. */
export function useTabIds(): string[] {
  return useStore((s: Store) => s.tabs.map((t) => t.id));
}

export function useActivitySection(): ActivitySection {
  return useStore((s: Store) => s.activitySection);
}

export function useSidebarOpen(): boolean {
  return useStore((s: Store) => s.sidebarOpen);
}

export function useActivePaneId(): string {
  return useStore((s: Store) => s.activePaneId);
}

/** Returns the currently-active leaf, or the first leaf as fallback. */
export function useActiveLeaf() {
  const workspace = useStore((s: Store) => s.workspace);
  const activeId = useStore((s: Store) => s.activePaneId);
  return useMemo(() => findLeaf(workspace, activeId) ?? firstLeaf(workspace), [workspace, activeId]);
}

/** Find the leaf that hosts the given view id. */
export function useLeafForView(viewId: string | null) {
  const workspace = useStore((s: Store) => s.workspace);
  return useMemo(() => (viewId ? findViewLeaf(workspace, viewId) : null), [workspace, viewId]);
}
