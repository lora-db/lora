"use client";

/**
 * Imperative tab-management actions.
 *
 * The tabs slice owns the master tab records (`tabs[]`). The workspace
 * layout owns per-editor-view tab ordering (`view.tabIds`). These
 * actions keep the two in sync:
 *
 *  - `newTab` creates a global record AND adds it to the active editor
 *    view's strip.
 *  - `closeTabInView` removes the tab from a single editor view; if it
 *    leaves the global record orphaned (no other view holds it), the
 *    tab is also closed globally (and `closeTab` GCs the workspace).
 */

import { modals } from "@mantine/modals";

import { useStore } from "@/lib/state/store";
import {
  countEditorViews,
  findCellForLeaf,
  findLeaf,
  findViewLeaf,
  firstCell,
  firstEditorView,
  tabIsOpenInAnyEditorView,
} from "@/lib/state/workspace/tree";

const DEFAULT_BODY = "MATCH (n)\nOPTIONAL MATCH (n)-[r]->(m)\nRETURN n, r, m";

// Bounded LIFO of recently-closed tabs. Reopened via `mod+shift+T`.
// Captures everything we need to rehydrate a tab without rooting around
// in deleted state. Kept module-local — survives across actions but is
// intentionally not persisted to IDB (mirrors browser-tab UX).
interface ClosedTabSnapshot {
  name: string;
  body: string;
  savedQueryId?: string;
}
const RECENTLY_CLOSED_LIMIT = 16;
const recentlyClosed: ClosedTabSnapshot[] = [];

function pushClosedSnapshot(snap: ClosedTabSnapshot): void {
  recentlyClosed.push(snap);
  if (recentlyClosed.length > RECENTLY_CLOSED_LIMIT) {
    recentlyClosed.splice(0, recentlyClosed.length - RECENTLY_CLOSED_LIMIT);
  }
}

/**
 * Resolve the cell that should host a newly-opened tab.
 *
 *   1. The cell that contains the active pane (whether the user is
 *      currently looking at an editor or a result inside it).
 *   2. The first cell anywhere in the workspace if (1) doesn't apply.
 *
 * Returns `null` only when the workspace contains no cells at all (an
 * impossible state in normal use — the failsafe would have reset it).
 */
function pickCellForNewTab(): { editorViewId: string; cellId: string } | null {
  const state = useStore.getState();
  const activeCell = findCellForLeaf(state.workspace, state.activePaneId);
  const cell = activeCell ?? firstCell(state.workspace);
  if (!cell) return null;
  return { editorViewId: cell.editorView.id, cellId: cell.cellId };
}

/**
 * Find which editor view should host a newly-opened tab. Preference:
 *   1. The active pane's enclosing cell (whether the active view is
 *      editor or result).
 *   2. The first editor view anywhere in the workspace.
 *   3. `null` if the workspace has no editor view at all (in which case
 *      the caller should still create the global tab record).
 */
function pickEditorViewId(): string | null {
  const picked = pickCellForNewTab();
  if (picked) return picked.editorViewId;
  // Last-ditch fallback (no cell, but maybe a stray editor view).
  return firstEditorView(useStore.getState().workspace)?.view.id ?? null;
}

/**
 * Create a tab and, in the same operation, attach it to an editor
 * view's strip so the user always sees it.
 *
 * This is the canonical "open tab" entry point for every code path
 * outside the editor strip's own "+" button (saved queries, history,
 * Inspector "visualize neighbors", drop-zone import, share link). It
 * keeps the global `tabs` slice and the per-pane `tabIds` strips in
 * sync without callers having to remember the second step.
 */
export function openTabInCell(opts: {
  name?: string;
  body?: string;
  savedQueryId?: string;
  /** Specific editor view to host the tab. Defaults to the active cell's editor. */
  editorViewId?: string;
}): string {
  const state = useStore.getState();
  const id = state.openTab({
    body: opts.body ?? DEFAULT_BODY,
    ...(opts.name !== undefined ? { name: opts.name } : {}),
    ...(opts.savedQueryId !== undefined ? { savedQueryId: opts.savedQueryId } : {}),
  });
  const viewId = opts.editorViewId ?? pickEditorViewId();
  if (viewId) state.addTabToEditorView(viewId, id);
  return id;
}

/** Convenience: open a tab in the active cell's editor strip. */
export function newTab(): string {
  return openTabInCell({ body: DEFAULT_BODY });
}

/** Open a tab and add it to a specific editor view. Used by the in-strip "+" button. */
export function newTabInView(viewId: string, opts?: { name?: string; body?: string; savedQueryId?: string }): string {
  return openTabInCell({ ...(opts ?? {}), editorViewId: viewId });
}

/**
 * Close `tabId` from the editor view `viewId` only. If the tab is
 * still open in another editor view we keep the global record; if not
 * we drop it entirely (and gc its result). Honours the dirty-tab
 * confirm modal.
 *
 * Refuses to close the last tab in the last editor view, so the
 * workspace always has at least one query tab to land in.
 */
export function closeTabInView(viewId: string, tabId: string): void {
  const state = useStore.getState();
  const tab = state.tabs.find((t) => t.id === tabId);
  if (!tab) return;
  // Refuse to close if it would leave zero tabs globally.
  if (state.tabs.length <= 1) {
    return;
  }
  // Detect the "this is the only tab in the only editor view" case
  // before we touch anything — refusing here keeps the user from
  // stranding the workspace with no place to type.
  const viewLeafNow = findViewLeaf(state.workspace, viewId);
  const viewNow = viewLeafNow?.views.find((v) => v.id === viewId);
  const stripWouldEmpty =
    viewNow?.kind === "query" &&
    (viewNow.tabIds ?? []).length === 1 &&
    (viewNow.tabIds ?? [])[0] === tabId;
  if (stripWouldEmpty && countEditorViews(state.workspace) <= 1) {
    return;
  }
  const proceed = () => {
    const s = useStore.getState();
    s.removeTabFromEditorView(viewId, tabId);
    const stillOpen = tabIsOpenInAnyEditorView(useStore.getState().workspace, tabId);
    if (!stillOpen) {
      // Snapshot the tab before it disappears so `mod+shift+T` can
      // resurrect it. Read straight from `s.tabs` so the body we
      // capture is the latest in-memory value, not a stale closure.
      const closing = s.tabs.find((t) => t.id === tabId);
      if (closing) {
        pushClosedSnapshot({
          name: closing.name,
          body: closing.body,
          ...(closing.savedQueryId !== undefined
            ? { savedQueryId: closing.savedQueryId }
            : {}),
        });
      }
      s.closeTab(tabId);
    }
    // If we just emptied a non-last editor view, drop the view so the
    // leaf doesn't render a blank editor surface. (countEditorViews
    // guard above ensures we have ≥1 other editor view to land in.)
    if (stripWouldEmpty) {
      useStore.getState().removeView(viewId);
    }
  };
  if (!tab.dirty) {
    proceed();
    return;
  }
  modals.openConfirmModal({
    title: "Discard unsaved changes?",
    children: `"${tab.name}" has unsaved edits that will be lost.`,
    labels: { confirm: "Discard", cancel: "Keep editing" },
    confirmProps: { color: "red", "data-autofocus": "true" },
    onConfirm: proceed,
  });
}

export function closeActiveTab(): void {
  const state = useStore.getState();
  const viewId = pickEditorViewId();
  if (!viewId) return;
  const leaf = findLeaf(state.workspace, state.activePaneId);
  const view = leaf?.views.find((v) => v.id === viewId);
  const tabId = view?.tabId;
  if (!tabId) return;
  closeTabInView(viewId, tabId);
}

/**
 * Cycle the active tab inside the active editor pane (next / prev).
 * Falls back to global tabs when the active pane isn't an editor.
 */
function activeViewTabs(): { viewId: string; tabIds: string[]; activeId: string | null } | null {
  const state = useStore.getState();
  const viewId = pickEditorViewId();
  if (!viewId) return null;
  const leaf = findLeaf(state.workspace, state.activePaneId);
  const view = leaf?.views.find((v) => v.id === viewId);
  if (!view) return null;
  return {
    viewId,
    tabIds: view.tabIds ?? [],
    activeId: view.tabId ?? null,
  };
}

export function nextTab(): void {
  const state = useStore.getState();
  const ctx = activeViewTabs();
  if (!ctx || ctx.tabIds.length === 0) return;
  const idx = ctx.activeId ? ctx.tabIds.indexOf(ctx.activeId) : -1;
  const nextIdx = (idx + 1 + ctx.tabIds.length) % ctx.tabIds.length;
  const nextId = ctx.tabIds[nextIdx];
  if (nextId) state.setViewTabId(ctx.viewId, nextId);
}

export function prevTab(): void {
  const state = useStore.getState();
  const ctx = activeViewTabs();
  if (!ctx || ctx.tabIds.length === 0) return;
  const idx = ctx.activeId ? ctx.tabIds.indexOf(ctx.activeId) : 0;
  const prevIdx = (idx - 1 + ctx.tabIds.length) % ctx.tabIds.length;
  const prevId = ctx.tabIds[prevIdx];
  if (prevId) state.setViewTabId(ctx.viewId, prevId);
}

export function moveActiveTabLeft(): void {
  const state = useStore.getState();
  const ctx = activeViewTabs();
  if (!ctx || ctx.activeId === null) return;
  const idx = ctx.tabIds.indexOf(ctx.activeId);
  if (idx <= 0) return;
  state.reorderTabInEditorView(ctx.viewId, idx, idx - 1);
}

export function moveActiveTabRight(): void {
  const state = useStore.getState();
  const ctx = activeViewTabs();
  if (!ctx || ctx.activeId === null) return;
  const idx = ctx.tabIds.indexOf(ctx.activeId);
  if (idx === -1 || idx >= ctx.tabIds.length - 1) return;
  state.reorderTabInEditorView(ctx.viewId, idx, idx + 1);
}

export function focusEditor(): void {
  if (typeof document === "undefined") return;
  // CodeMirror 6 renders its editable region as `.cm-content`.
  const el = document.querySelector<HTMLElement>(".cm-content");
  if (el) el.focus();
}

/**
 * Re-open the most recently closed tab in the active editor strip.
 * Tabs are popped off a bounded LIFO captured by {@link closeTabInView}.
 * Reopened tabs always come back as a fresh global record — never
 * resurrects the original id so any tree references that were GC'd at
 * close time stay GC'd. Saved-query binding is preserved when present.
 *
 * Returns the new tab id (or `null` if the stack is empty).
 */
export function reopenLastClosedTab(): string | null {
  const snap = recentlyClosed.pop();
  if (!snap) return null;
  return openTabInCell({
    name: snap.name,
    body: snap.body,
    ...(snap.savedQueryId !== undefined
      ? { savedQueryId: snap.savedQueryId }
      : {}),
  });
}

