"use client";

/**
 * Imperative wrappers for window-management actions — split, close,
 * reorient, move-view-between-panes, etc. Callable from hotkeys,
 * Spotlight commands, and context menus.
 */

import { useStore } from "@/lib/state/store";
import { ulid } from "@/lib/util/id";
import type {
  PanelGroup,
  PanelKind,
  PanelNode,
  PanelView,
  Placement,
  ResultTab,
  SplitDirection,
} from "@/lib/state/slices/layout";
import {
  findLeaf,
  findViewLeaf,
  flatLeafIds,
  iterLeaves,
  makeGroup,
  makeLeaf,
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
 * Split the workspace, creating a new self-contained editor+result
 * pair alongside the existing one.
 *
 * Semantically: each "workspace cell" is a column containing an editor
 * leaf on top and a result leaf below (both pinned to the same tab).
 * Splitting builds a brand-new cell for a fresh tab and places it next
 * to the current cell. The existing cell's editor and result get
 * pinned to the source tab so the two workspaces are fully independent.
 *
 * `direction` controls how cells are arranged relative to each other:
 *   - "row"    → cells sit side by side (the most common split)
 *   - "column" → cells stack top/bottom (rare but supported)
 *
 * Returns the new leaf's id (the new editor leaf) so the caller can
 * shift focus to it.
 */
export function splitActivePane(direction: SplitDirection, placement: Placement = "after"): string | null {
  const state = useStore.getState();
  const paneId = state.activePaneId;
  const activeLeaf = findLeaf(state.workspace, paneId);
  const activeView = activeLeaf?.views.find((v) => v.id === activeLeaf.activeViewId);

  // The tab we clone from — prefer the active editor view's tab.
  let sourceTabId: string | null = null;
  if (activeView?.kind === "editor" && activeView.tabId) {
    sourceTabId = activeView.tabId;
  } else if (activeView?.tabId) {
    sourceTabId = activeView.tabId;
  } else {
    sourceTabId = getActiveTabId();
  }
  const sourceTab = sourceTabId ? state.tabs.find((t) => t.id === sourceTabId) : null;

  // Spawn the new tab globally. The tabs slice no longer carries any
  // "active tab" notion of its own — we wire the new tab into the new
  // cell's editor view below.
  const newTabId = state.openTab({ body: sourceTab?.body ?? "" });

  // Build the new workspace cell — editor + result, both pinned to the
  // new tab so it's fully independent of any sibling cells.
  const newCell = buildWorkspaceCell(newTabId, "graph");

  // Canonicalise the existing workspace into one or more cells before
  // splicing. This way the source workspace is guaranteed to be paired
  // (editor on top, result below in a column) regardless of any prior
  // orientation toggles or partial splits.
  const sourceTreeNormalized = canonicaliseAsCells(state.workspace, sourceTabId);

  // Insert the new cell. If the canonical source is already a group in
  // the requested direction with all-cell children, splice; otherwise
  // wrap it in a fresh group of the requested direction.
  let nextRoot: PanelNode;
  if (
    sourceTreeNormalized.type === "group" &&
    sourceTreeNormalized.direction === direction &&
    childrenAreCells(sourceTreeNormalized)
  ) {
    const before = placement === "before";
    nextRoot = {
      ...sourceTreeNormalized,
      sizes: balanceSizes(sourceTreeNormalized.children.length + 1),
      children: before
        ? [newCell, ...sourceTreeNormalized.children]
        : [...sourceTreeNormalized.children, newCell],
    } satisfies PanelGroup;
  } else {
    const before = placement === "before";
    nextRoot = makeGroup(
      direction,
      before ? [newCell, sourceTreeNormalized] : [sourceTreeNormalized, newCell],
      { sizes: [50, 50] },
    );
  }

  // The new cell's first child is its editor leaf — that's where focus lands.
  const newEditorLeafChild = newCell.children[0];
  const newEditorLeafId =
    newEditorLeafChild && newEditorLeafChild.type === "leaf"
      ? newEditorLeafChild.id
      : null;
  state.replaceWorkspace(nextRoot, {
    ...(newEditorLeafId ? { activePaneId: newEditorLeafId } : {}),
  });
  return newEditorLeafId;
}

// ────────────────────────────────────────────────────────────────
// Workspace cell helpers
// ────────────────────────────────────────────────────────────────

/**
 * Build a fresh column-cell containing one editor leaf and one result
 * leaf, both pinned to the same tab.
 */
function buildWorkspaceCell(tabId: string, resultTab: ResultTab): PanelGroup {
  const editorView = makeView({
    kind: "editor",
    tabId,
    tabIds: [tabId],
  });
  const resultView = makeView({
    kind: "result",
    tabId,
    resultTab,
  });
  return makeGroup("column", [makeLeaf([editorView]), makeLeaf([resultView])]);
}

/**
 * A canonical cell = column group containing exactly one editor leaf
 * and one result leaf.
 */
function isCanonicalCell(node: PanelNode): boolean {
  if (node.type !== "group") return false;
  if (node.direction !== "column") return false;
  if (node.children.length !== 2) return false;
  const [a, b] = node.children;
  if (!a || a.type !== "leaf" || a.views.length !== 1) return false;
  if (!b || b.type !== "leaf" || b.views.length !== 1) return false;
  const aKind = a.views[0]!.kind;
  const bKind = b.views[0]!.kind;
  return (
    (aKind === "editor" && bKind === "result") ||
    (aKind === "result" && bKind === "editor")
  );
}

function childrenAreCells(group: PanelGroup): boolean {
  return group.children.every(isCanonicalCell);
}

/**
 * Bring the tree into "row/column of cells" form when it cheaply can.
 *
 *  - Single canonical cell → re-pin both leaves to `sourceTabId` so the
 *    new sibling cell isn't sharing state with it.
 *  - Already a row/column of cells → returned unchanged.
 *  - Anything else → returned unchanged. We used to collapse arbitrary
 *    trees into a single editor+result cell, but that silently dropped
 *    the user's other panes (e.g. a third editor view, a manual `addView`).
 *    `splitActivePane` will wrap the un-normalised tree in a new parent
 *    group alongside the new cell instead.
 */
function canonicaliseAsCells(tree: PanelNode, sourceTabId: string | null): PanelNode {
  if (isCanonicalCell(tree)) return ensureCellPinned(tree as PanelGroup, sourceTabId);
  return tree;
}

/**
 * For a canonical cell, pin both leaves to `tabId` (preserving the
 * editor strip's other tab ids).
 */
function ensureCellPinned(cell: PanelGroup, tabId: string | null): PanelGroup {
  if (!tabId) return cell;
  const rewrite = (child: PanelNode): PanelNode => {
    if (child.type !== "leaf") return child;
    const view = child.views[0]!;
    if (view.kind === "editor") {
      const tabIds = view.tabIds && view.tabIds.includes(tabId)
        ? view.tabIds
        : [...(view.tabIds ?? []), tabId];
      return { ...child, views: [{ ...view, tabId, tabIds }] };
    }
    if (view.kind === "result") {
      return { ...child, views: [{ ...view, tabId }] };
    }
    return child;
  };
  return { ...cell, children: cell.children.map(rewrite) };
}

function balanceSizes(count: number): number[] {
  const each = 100 / count;
  return Array.from({ length: count }, () => each);
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
  state.setGroupDirection(parent.id, parent.direction === "row" ? "column" : "row");
}

/**
 * Add a fresh view of the given kind to the active pane (or create one
 * by splitting if requested). Returns the new view id.
 */
export function openViewInActivePane(kind: PanelKind, opts?: { resultTab?: ResultTab; tabId?: string }): string {
  const state = useStore.getState();
  return state.addView(state.activePaneId, kind, opts);
}

export function openViewInNewPane(
  kind: PanelKind,
  direction: SplitDirection,
  placement: Placement = "after",
  opts?: { resultTab?: ResultTab; tabId?: string },
): string | null {
  const state = useStore.getState();

  if (kind === "editor" && opts?.tabId === undefined) {
    // Always give a fresh editor pane its own tab so edits don't mirror
    // a sibling pane. Caller can pass a specific `tabId` to share a tab.
    const activeLeaf = findLeaf(state.workspace, state.activePaneId);
    const sourceView = activeLeaf?.views.find((v) => v.id === activeLeaf.activeViewId);
    const seedBody = (() => {
      if (sourceView?.kind === "editor" && sourceView.tabId) {
        return state.tabs.find((t) => t.id === sourceView.tabId)?.body ?? "";
      }
      const fallback = getActiveTabId();
      return fallback ? state.tabs.find((t) => t.id === fallback)?.body ?? "" : "";
    })();
    const newTabId = state.openTab({ body: seedBody });
    const view: PanelView = {
      id: ulid(),
      kind: "editor",
      tabId: newTabId,
      tabIds: [newTabId],
    };
    return state.splitPane(state.activePaneId, direction, placement, view);
  }

  const view: PanelView = {
    id: ulid(),
    kind,
    ...(opts?.tabId !== undefined ? { tabId: opts.tabId } : {}),
    ...(kind === "result" ? { resultTab: opts?.resultTab ?? "graph" } : {}),
  };
  return state.splitPane(state.activePaneId, direction, placement, view);
}

export function closeView(viewId: string): void {
  useStore.getState().removeView(viewId);
}

export function moveViewToPane(viewId: string, toPaneId: string, toIndex?: number): void {
  useStore.getState().moveView(viewId, toPaneId, toIndex);
}

/**
 * Set the result inner-tab on the most appropriate target:
 *   1. the active pane's currently-active view, if it's a result view
 *   2. otherwise the first result view in the workspace
 * Returns the view id we targeted, or null if no result view exists.
 */
export function setActiveResultTab(resultTab: ResultTab): string | null {
  const state = useStore.getState();
  // 1. Active pane?
  const activeLeaf = findLeaf(state.workspace, state.activePaneId);
  if (activeLeaf) {
    const av = activeLeaf.views.find((v) => v.id === activeLeaf.activeViewId);
    if (av?.kind === "result") {
      state.setResultTabForView(av.id, resultTab);
      return av.id;
    }
  }
  // 2. Fallback: first result view anywhere in the tree.
  const found = (function walk(node: typeof state.workspace): PanelView | null {
    if (node.type === "leaf") {
      return node.views.find((v) => v.kind === "result") ?? null;
    }
    for (const child of node.children) {
      const r = walk(child);
      if (r) return r;
    }
    return null;
  })(state.workspace);
  if (found) {
    state.setResultTabForView(found.id, resultTab);
    return found.id;
  }
  return null;
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
 * Activate `tabId` in some editor view. Preference order:
 *   1. The active pane's editor view if it already contains the tab.
 *   2. Any other editor view whose strip already lists the tab.
 *   3. The first editor view (the tab is added to its strip).
 *
 * Also moves `activePaneId` so the focused pane shows the new tab.
 */
export function focusTabInWorkspace(tabId: string): void {
  const state = useStore.getState();
  const activeLeaf = findLeaf(state.workspace, state.activePaneId);
  // Preference 1.
  if (activeLeaf) {
    for (const v of activeLeaf.views) {
      if (v.kind === "editor" && (v.tabIds ?? []).includes(tabId)) {
        state.setViewTabId(v.id, tabId);
        return;
      }
    }
  }
  // Preference 2.
  for (const leaf of iterLeaves(state.workspace)) {
    for (const v of leaf.views) {
      if (v.kind !== "editor") continue;
      if ((v.tabIds ?? []).includes(tabId)) {
        state.setActivePane(leaf.id);
        state.setViewTabId(v.id, tabId);
        return;
      }
    }
  }
  // Preference 3: add to the first editor view's strip.
  for (const leaf of iterLeaves(state.workspace)) {
    for (const v of leaf.views) {
      if (v.kind === "editor") {
        state.setActivePane(leaf.id);
        state.addTabToEditorView(v.id, tabId);
        return;
      }
    }
  }
}

/** GC any workspace views pinned to a tab that no longer exists. */
export function gcWorkspaceForTab(tabId: string): void {
  useStore.getState().gcClosedTab(tabId);
}

export { findViewLeaf };
