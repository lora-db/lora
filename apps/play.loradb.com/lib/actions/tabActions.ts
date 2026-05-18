"use client";

/**
 * Imperative tab-management actions. These are tiny wrappers over the
 * store so the hotkey + spotlight surfaces can call them without
 * pulling the zustand selector machinery into every binding.
 */

import { modals } from "@mantine/modals";

import { useStore } from "@/lib/state/store";

const DEFAULT_BODY = "MATCH (n)\nOPTIONAL MATCH (n)-[r]->(m)\nRETURN n, r, m";

export function newTab(): string {
  const state = useStore.getState();
  return state.openTab({ body: DEFAULT_BODY });
}

/**
 * Close a tab, prompting first if it has unsaved changes. Use this from
 * UI surfaces (X-click, middle-click, hotkey, spotlight) instead of
 * `closeTab` so the confirm guard is consistent.
 */
export function requestCloseTab(id: string): void {
  const state = useStore.getState();
  const tab = state.tabs.find((t) => t.id === id);
  if (!tab) return;
  if (!tab.dirty) {
    state.closeTab(id);
    return;
  }
  modals.openConfirmModal({
    title: "Discard unsaved changes?",
    children: `"${tab.name}" has unsaved edits that will be lost.`,
    labels: { confirm: "Discard", cancel: "Keep editing" },
    confirmProps: { color: "red" },
    onConfirm: () => {
      // Re-resolve the tab — the user may have edited or renamed it
      // while the modal was open, but we still close by id.
      useStore.getState().closeTab(id);
    },
  });
}

export function closeActiveTab(): void {
  const state = useStore.getState();
  const id = state.activeTabId;
  if (id === null) return;
  requestCloseTab(id);
}

export function nextTab(): void {
  const state = useStore.getState();
  if (state.tabs.length === 0) return;
  const idx = state.tabs.findIndex((t) => t.id === state.activeTabId);
  if (idx === -1) {
    const first = state.tabs[0];
    if (first) state.setActiveTab(first.id);
    return;
  }
  const nextIdx = (idx + 1) % state.tabs.length;
  const next = state.tabs[nextIdx];
  if (next) state.setActiveTab(next.id);
}

export function prevTab(): void {
  const state = useStore.getState();
  if (state.tabs.length === 0) return;
  const idx = state.tabs.findIndex((t) => t.id === state.activeTabId);
  if (idx === -1) {
    const first = state.tabs[0];
    if (first) state.setActiveTab(first.id);
    return;
  }
  const prevIdx = (idx - 1 + state.tabs.length) % state.tabs.length;
  const prev = state.tabs[prevIdx];
  if (prev) state.setActiveTab(prev.id);
}

/**
 * Move the active tab one slot to the left (towards index 0). No-op when
 * the tab is already at the start or when no tab is active. Companion to
 * the drag-to-reorder affordance — wired to a keyboard chord so
 * keyboard-only workflows can rearrange tabs too.
 */
export function moveActiveTabLeft(): void {
  const state = useStore.getState();
  const id = state.activeTabId;
  if (id === null) return;
  const idx = state.tabs.findIndex((t) => t.id === id);
  if (idx <= 0) return;
  state.reorderTab(idx, idx - 1);
}

/** Mirror of `moveActiveTabLeft` in the other direction. */
export function moveActiveTabRight(): void {
  const state = useStore.getState();
  const id = state.activeTabId;
  if (id === null) return;
  const idx = state.tabs.findIndex((t) => t.id === id);
  if (idx === -1 || idx >= state.tabs.length - 1) return;
  state.reorderTab(idx, idx + 1);
}

export function focusEditor(): void {
  if (typeof document === "undefined") return;
  // CodeMirror 6 renders its editable region as `.cm-content`.
  const el = document.querySelector<HTMLElement>(".cm-content");
  if (el) el.focus();
}
