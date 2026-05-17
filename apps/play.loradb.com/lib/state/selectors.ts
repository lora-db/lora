"use client";

/**
 * Narrow hooks built on top of `useStore` so components subscribe only to
 * the slice of state they actually render. Using these in place of
 * `useStore(s => s.someField)` keeps re-renders contained.
 */

import { useStore, type Store } from "./store";
import type { EditorTab } from "./slices/tabs";
import type { TabResult } from "./slices/results";
import type { ActivitySection, ResultTab } from "./slices/layout";

/** Returns the currently-active editor tab, or `null` if there isn't one. */
export function useActiveTab(): EditorTab | null {
  return useStore((s: Store) => {
    if (s.activeTabId === null) return null;
    return s.tabs.find((t) => t.id === s.activeTabId) ?? null;
  });
}

/** Returns the result associated with the active tab, if any. */
export function useActiveResult(): TabResult {
  return useStore((s: Store) => {
    if (s.activeTabId === null) return undefined;
    return s.results[s.activeTabId];
  });
}

/** Returns the list of tab IDs in their current order. */
export function useTabIds(): string[] {
  return useStore((s: Store) => s.tabs.map((t) => t.id));
}

export function useResultTab(): ResultTab {
  return useStore((s: Store) => s.resultTab);
}

export function useActivitySection(): ActivitySection {
  return useStore((s: Store) => s.activitySection);
}

export function useSidebarOpen(): boolean {
  return useStore((s: Store) => s.sidebarOpen);
}
