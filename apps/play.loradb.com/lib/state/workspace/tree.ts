/**
 * Pure, immutable transformations on the workspace tree.
 *
 * Each function returns a NEW tree (and possibly an auxiliary value such as
 * the id of a newly-created leaf). Callers feed the result back into the
 * store via `set`; immer happily accepts plain objects as draft replacements
 * and re-freezes them.
 *
 * Keeping these helpers pure makes the unit tests trivial: hand them a tree,
 * inspect the result.
 */

import type { ResultTab } from "@/lib/state/slices/layout";

import { ulid } from "@/lib/util/id";

/**
 * A pane represents a single "query workspace": one editor surface
 * with its own tab strip and a result region that always belongs to
 * whichever tab is active. Splits create more queries, never bare
 * editors or bare results — the two are bound at the data layer so
 * neither can be closed alone.
 */
export type PanelKind = "query";

export interface PanelView {
  /** Stable view id, ULID. */
  id: string;
  kind: PanelKind;
  /** Active tab in this pane (must be a member of `tabIds`). */
  tabId?: string;
  /** Ordered list of tab ids visible in this pane's tab strip. */
  tabIds?: string[];
  /** Which inner result tab (Graph/Table/JSON/Plan). */
  resultTab?: ResultTab;
  /** When `true`, the result region collapses to a thin restore strip. */
  resultMinimized?: boolean;
  /** Percentage of the leaf's height the editor surface gets (0–100). */
  editorSizePct?: number;
  /** When `true`, the Params panel sidecar is visible for this view. */
  paramsPanelOpen?: boolean;
  /** Width of the Params panel as a percentage of the editor row (12–60). */
  paramsPanelSize?: number;
}

export interface PanelLeaf {
  type: "leaf";
  id: string;
  views: PanelView[];
  activeViewId: string;
}

export type SplitDirection = "row" | "column";

export interface PanelGroup {
  type: "group";
  id: string;
  /** "row" lays out children horizontally (side-by-side); "column" vertically (top/bottom). */
  direction: SplitDirection;
  /** Fractions summing to ~100, index-aligned with children. */
  sizes: number[];
  children: PanelNode[];
}

export type PanelNode = PanelGroup | PanelLeaf;

export type Placement = "before" | "after";

// ────────────────────────────────────────────────────────────────
// Constructors
// ────────────────────────────────────────────────────────────────

export function makeView(
  input?: Partial<PanelView> & { kind?: PanelKind },
): PanelView {
  const view: PanelView = {
    id: input?.id ?? ulid(),
    kind: "query",
    tabIds: input?.tabIds ? [...input.tabIds] : [],
    resultTab: input?.resultTab ?? "graph",
  };
  if (input?.tabId !== undefined) view.tabId = input.tabId;
  if (input?.resultMinimized !== undefined)
    view.resultMinimized = input.resultMinimized;
  if (input?.editorSizePct !== undefined)
    view.editorSizePct = input.editorSizePct;
  if (input?.paramsPanelOpen !== undefined)
    view.paramsPanelOpen = input.paramsPanelOpen;
  if (input?.paramsPanelSize !== undefined)
    view.paramsPanelSize = input.paramsPanelSize;
  return view;
}

export function makeLeaf(
  views: PanelView[],
  opts?: { id?: string; activeViewId?: string },
): PanelLeaf {
  const first = views[0];
  if (!first) {
    throw new Error("makeLeaf: leaf must contain at least one view");
  }
  return {
    type: "leaf",
    id: opts?.id ?? ulid(),
    views,
    activeViewId: opts?.activeViewId ?? first.id,
  };
}

export function makeGroup(
  direction: SplitDirection,
  children: PanelNode[],
  opts?: { id?: string; sizes?: number[] },
): PanelGroup {
  if (children.length < 2) {
    throw new Error("makeGroup: a group must hold at least two children");
  }
  const sizes =
    opts?.sizes && opts.sizes.length === children.length
      ? normaliseSizes(opts.sizes)
      : even(children.length);
  return {
    type: "group",
    id: opts?.id ?? ulid(),
    direction,
    sizes,
    children,
  };
}

// ────────────────────────────────────────────────────────────────
// Traversal
// ────────────────────────────────────────────────────────────────

export function* iterLeaves(node: PanelNode): Generator<PanelLeaf> {
  if (node.type === "leaf") {
    yield node;
    return;
  }
  for (const child of node.children) yield* iterLeaves(child);
}

export function findLeaf(node: PanelNode, leafId: string): PanelLeaf | null {
  if (node.type === "leaf") return node.id === leafId ? node : null;
  for (const child of node.children) {
    const hit = findLeaf(child, leafId);
    if (hit) return hit;
  }
  return null;
}

export function firstLeaf(node: PanelNode): PanelLeaf {
  if (node.type === "leaf") return node;
  const child = node.children[0];
  if (!child) throw new Error("workspace tree: empty group");
  return firstLeaf(child);
}

export function findViewLeaf(
  node: PanelNode,
  viewId: string,
): PanelLeaf | null {
  for (const leaf of iterLeaves(node)) {
    if (leaf.views.some((v) => v.id === viewId)) return leaf;
  }
  return null;
}

/** First query view (= the canonical view kind) anywhere in the tree. */
export function firstQueryView(
  node: PanelNode,
): { leaf: PanelLeaf; view: PanelView } | null {
  for (const leaf of iterLeaves(node)) {
    for (const v of leaf.views) {
      if (v.kind === "query") return { leaf, view: v };
    }
  }
  return null;
}

/** Back-compat alias — pre-refactor code looked up the first editor view. */
export const firstEditorView = firstQueryView;

/**
 * Pre-order leaf list — useful for "focus next pane" navigation.
 */
export function flatLeafIds(node: PanelNode): string[] {
  const out: string[] = [];
  for (const leaf of iterLeaves(node)) out.push(leaf.id);
  return out;
}

// ────────────────────────────────────────────────────────────────
// Pure tree transformations
// ────────────────────────────────────────────────────────────────

/**
 * Wrap the leaf identified by `paneId` in a new group, splitting it in
 * `direction`. The new sibling is a duplicate of the leaf's *active* view
 * unless `newView` is provided. Returns the rewritten tree plus the new
 * leaf's id (so callers can shift focus to it).
 *
 * If the parent group already has the requested direction we just insert
 * the new sibling next to the source rather than nesting a fresh group —
 * this keeps the tree shallow.
 */
export function splitLeaf(
  tree: PanelNode,
  paneId: string,
  direction: SplitDirection,
  placement: Placement,
  newView?: PanelView,
): { tree: PanelNode; newLeafId: string } | null {
  const leaf = findLeaf(tree, paneId);
  if (!leaf) return null;

  // A fresh sibling pane is always a brand-new query — splits never
  // share tab state with the source pane. Callers can pass a pre-built
  // view via `newView` when they already wired up a tab for it.
  const seedView = newView ?? makeView({ kind: "query" });
  const newLeaf = makeLeaf([seedView]);

  // Walk and rewrite. When we find the parent of `leaf`, decide whether to
  // splice next to it (parent already runs in the same direction) or wrap
  // the leaf in a new group.
  const rewrite = (
    node: PanelNode,
    parentDirection: SplitDirection | null,
  ): PanelNode => {
    if (node.type === "leaf") {
      if (node.id !== paneId) return node;
      if (parentDirection === direction) {
        // Sibling will be spliced by the parent — we just emit the leaf
        // unchanged. The actual splice happens one frame up the recursion.
        return node;
      }
      // Wrap leaf in a new group of the requested direction.
      const before = placement === "before";
      return makeGroup(direction, before ? [newLeaf, node] : [node, newLeaf]);
    }
    // Group node. Recurse and, if any child is the target leaf and we share
    // its direction, splice the new leaf in.
    const childIndex = node.children.findIndex(
      (c) => c.type === "leaf" && c.id === paneId,
    );
    const rewrittenChildren = node.children.map((c) =>
      rewrite(c, node.direction),
    );
    if (childIndex !== -1 && node.direction === direction) {
      const insertAt = placement === "before" ? childIndex : childIndex + 1;
      const nextChildren = [
        ...rewrittenChildren.slice(0, insertAt),
        newLeaf,
        ...rewrittenChildren.slice(insertAt),
      ];
      // Halve the source pane's existing slice between it and the new leaf
      // so neither pane jumps to 50/50 of the whole group.
      const sourceSize = node.sizes[childIndex] ?? 100 / node.children.length;
      const nextSizes = [...node.sizes];
      nextSizes[childIndex] = sourceSize / 2;
      nextSizes.splice(insertAt, 0, sourceSize / 2);
      return {
        type: "group",
        id: node.id,
        direction: node.direction,
        sizes: normaliseSizes(nextSizes),
        children: nextChildren,
      };
    }
    return {
      type: "group",
      id: node.id,
      direction: node.direction,
      sizes: node.sizes,
      children: rewrittenChildren,
    };
  };

  // Top-level handling: if the tree is just the leaf itself, wrap.
  if (tree.type === "leaf" && tree.id === paneId) {
    const before = placement === "before";
    return {
      tree: makeGroup(direction, before ? [newLeaf, tree] : [tree, newLeaf]),
      newLeafId: newLeaf.id,
    };
  }

  return { tree: rewrite(tree, null), newLeafId: newLeaf.id };
}

/**
 * Close (remove) the pane identified by `paneId`. Collapses single-child
 * groups so the tree stays canonical. Returns `null` when removing the
 * pane would empty the tree (so callers can refuse the operation).
 */
export function closeLeaf(tree: PanelNode, paneId: string): PanelNode | null {
  if (tree.type === "leaf") {
    return tree.id === paneId ? null : tree;
  }
  // Filter the group; recurse into nested groups; collapse single children.
  const rewriteGroup = (group: PanelGroup): PanelNode | null => {
    const nextChildren: PanelNode[] = [];
    const survivingIndices: number[] = [];
    for (let i = 0; i < group.children.length; i++) {
      const child = group.children[i]!;
      if (child.type === "leaf") {
        if (child.id !== paneId) {
          nextChildren.push(child);
          survivingIndices.push(i);
        }
      } else {
        const replaced = rewriteGroup(child);
        if (replaced !== null) {
          nextChildren.push(replaced);
          survivingIndices.push(i);
        }
      }
    }
    if (nextChildren.length === 0) return null;
    if (nextChildren.length === 1) {
      // Collapse: the lone child takes the parent's slot.
      return nextChildren[0]!;
    }
    const nextSizes = normaliseSizes(
      survivingIndices.map((i) => group.sizes[i] ?? 0),
    );
    return {
      type: "group",
      id: group.id,
      direction: group.direction,
      sizes: nextSizes,
      children: nextChildren,
    };
  };
  return rewriteGroup(tree);
}

/**
 * Toggle / set the direction of the group containing `paneId`. If
 * `paneId` is the root of the tree (a lone leaf), wraps nothing — we
 * cannot orient a single pane.
 */
export function setGroupDirection(
  tree: PanelNode,
  paneOrGroupId: string,
  direction: SplitDirection,
): PanelNode {
  if (tree.type === "leaf") return tree;
  // First, see if the id is itself a group we should rewrite.
  const rewrite = (node: PanelNode): PanelNode => {
    if (node.type === "leaf") return node;
    let updated: PanelGroup = node;
    if (node.id === paneOrGroupId) {
      updated = { ...node, direction };
    } else if (
      node.children.some((c) => c.type === "leaf" && c.id === paneOrGroupId)
    ) {
      // The id belongs to one of this group's leaf children.
      updated = { ...node, direction };
    }
    return {
      ...updated,
      children: updated.children.map(rewrite),
    };
  };
  return rewrite(tree);
}

/**
 * Set the sizes of the group with id `groupId`. Used by the
 * react-resizable-panels `onLayout` callback. Falls back to even sizes
 * if the array shape doesn't match.
 */
export function setGroupSizes(
  tree: PanelNode,
  groupId: string,
  sizes: number[],
): PanelNode {
  const rewrite = (node: PanelNode): PanelNode => {
    if (node.type === "leaf") return node;
    if (node.id === groupId && sizes.length === node.children.length) {
      return {
        ...node,
        sizes: normaliseSizes(sizes),
        children: node.children.map(rewrite),
      };
    }
    return { ...node, children: node.children.map(rewrite) };
  };
  return rewrite(tree);
}

/**
 * Remove `viewId` from its leaf. If the leaf would become empty, the
 * leaf itself is dropped (with the same collapse semantics as
 * `closeLeaf`). Returns the updated tree, or `null` if removing this
 * view would empty the workspace.
 */
export function removeView(tree: PanelNode, viewId: string): PanelNode | null {
  const leaf = findViewLeaf(tree, viewId);
  if (!leaf) return tree;

  if (leaf.views.length === 1) {
    return closeLeaf(tree, leaf.id);
  }

  const rewrite = (node: PanelNode): PanelNode => {
    if (node.type === "leaf") {
      if (node.id !== leaf.id) return node;
      const nextViews = node.views.filter((v) => v.id !== viewId);
      const stillActive = nextViews.some((v) => v.id === node.activeViewId);
      return {
        ...node,
        views: nextViews,
        activeViewId: stillActive ? node.activeViewId : nextViews[0]!.id,
      };
    }
    return { ...node, children: node.children.map(rewrite) };
  };
  return rewrite(tree);
}

/**
 * Insert / append `view` into the leaf identified by `paneId`. The view
 * is appended at the end by default; `index` lets DnD insert at a
 * specific tab slot. Activates the newly-inserted view.
 */
export function insertView(
  tree: PanelNode,
  paneId: string,
  view: PanelView,
  index?: number,
): PanelNode {
  const rewrite = (node: PanelNode): PanelNode => {
    if (node.type === "leaf") {
      if (node.id !== paneId) return node;
      const nextViews = [...node.views];
      const at =
        index === undefined
          ? nextViews.length
          : Math.max(0, Math.min(nextViews.length, index));
      nextViews.splice(at, 0, view);
      return { ...node, views: nextViews, activeViewId: view.id };
    }
    return { ...node, children: node.children.map(rewrite) };
  };
  return rewrite(tree);
}

/**
 * Move a view from one leaf to another. Idempotent — moving onto the
 * same leaf just reorders.
 */
export function moveView(
  tree: PanelNode,
  viewId: string,
  toPaneId: string,
  toIndex?: number,
): PanelNode | null {
  const sourceLeaf = findViewLeaf(tree, viewId);
  if (!sourceLeaf) return tree;
  const view = sourceLeaf.views.find((v) => v.id === viewId);
  if (!view) return tree;

  // If source === target, just reorder within the same leaf.
  if (sourceLeaf.id === toPaneId) {
    const rewrite = (node: PanelNode): PanelNode => {
      if (node.type === "leaf") {
        if (node.id !== toPaneId) return node;
        const remaining = node.views.filter((v) => v.id !== viewId);
        const at =
          toIndex === undefined
            ? remaining.length
            : Math.max(0, Math.min(remaining.length, toIndex));
        const next = [...remaining];
        next.splice(at, 0, view);
        return { ...node, views: next, activeViewId: view.id };
      }
      return { ...node, children: node.children.map(rewrite) };
    };
    return rewrite(tree);
  }

  // Remove from source then insert into target. removeView handles
  // collapse-on-empty; we then walk and insert.
  const removed = removeView(tree, viewId);
  if (!removed) return null;
  // The target leaf may have collapsed away (if it was directly nested
  // with the source) — bail out in that edge case.
  if (!findLeaf(removed, toPaneId)) return removed;
  return insertView(removed, toPaneId, view, toIndex);
}

/**
 * Set the inner result tab (Graph/Table/JSON/Plan) on a query view.
 */
export function setViewResultTab(
  tree: PanelNode,
  viewId: string,
  resultTab: ResultTab,
): PanelNode {
  const rewrite = (node: PanelNode): PanelNode => {
    if (node.type === "leaf") {
      const idx = node.views.findIndex((v) => v.id === viewId);
      if (idx === -1) return node;
      const view = node.views[idx]!;
      if (view.resultTab === resultTab) return node;
      const next = [...node.views];
      next[idx] = { ...view, resultTab };
      return { ...node, views: next };
    }
    return { ...node, children: node.children.map(rewrite) };
  };
  return rewrite(tree);
}

/** Toggle the minimized state of a query view's result region. */
export function setViewResultMinimized(
  tree: PanelNode,
  viewId: string,
  minimized: boolean,
): PanelNode {
  const rewrite = (node: PanelNode): PanelNode => {
    if (node.type === "leaf") {
      const idx = node.views.findIndex((v) => v.id === viewId);
      if (idx === -1) return node;
      const view = node.views[idx]!;
      if ((view.resultMinimized ?? false) === minimized) return node;
      const next = [...node.views];
      next[idx] = { ...view, resultMinimized: minimized };
      return { ...node, views: next };
    }
    return { ...node, children: node.children.map(rewrite) };
  };
  return rewrite(tree);
}

/** Adjust the editor/result split inside a query view (0-100). */
export function setViewEditorSizePct(
  tree: PanelNode,
  viewId: string,
  pct: number,
): PanelNode {
  const clamped = Math.max(10, Math.min(90, pct));
  const rewrite = (node: PanelNode): PanelNode => {
    if (node.type === "leaf") {
      const idx = node.views.findIndex((v) => v.id === viewId);
      if (idx === -1) return node;
      const view = node.views[idx]!;
      if (Math.abs((view.editorSizePct ?? 50) - clamped) < 0.5) return node;
      const next = [...node.views];
      next[idx] = { ...view, editorSizePct: clamped };
      return { ...node, views: next };
    }
    return { ...node, children: node.children.map(rewrite) };
  };
  return rewrite(tree);
}

/** Toggle the Params panel sidecar inside a single query view. */
export function setViewParamsPanelOpen(
  tree: PanelNode,
  viewId: string,
  open: boolean,
): PanelNode {
  const rewrite = (node: PanelNode): PanelNode => {
    if (node.type === "leaf") {
      const idx = node.views.findIndex((v) => v.id === viewId);
      if (idx === -1) return node;
      const view = node.views[idx]!;
      if ((view.paramsPanelOpen ?? false) === open) return node;
      const next = [...node.views];
      next[idx] = { ...view, paramsPanelOpen: open };
      return { ...node, views: next };
    }
    return { ...node, children: node.children.map(rewrite) };
  };
  return rewrite(tree);
}

/** Adjust the Params panel width inside a single query view (12-60). */
export function setViewParamsPanelSize(
  tree: PanelNode,
  viewId: string,
  pct: number,
): PanelNode {
  const clamped = Math.max(12, Math.min(60, pct));
  const rewrite = (node: PanelNode): PanelNode => {
    if (node.type === "leaf") {
      const idx = node.views.findIndex((v) => v.id === viewId);
      if (idx === -1) return node;
      const view = node.views[idx]!;
      if (Math.abs((view.paramsPanelSize ?? 30) - clamped) < 0.5) return node;
      const next = [...node.views];
      next[idx] = { ...view, paramsPanelSize: clamped };
      return { ...node, views: next };
    }
    return { ...node, children: node.children.map(rewrite) };
  };
  return rewrite(tree);
}

/**
 * Resolve the active query view's id — mirrors {@link resolveActiveTabId}.
 * Returns `null` only when the tree has no query view at all.
 */
export function resolveActiveViewId(
  tree: PanelNode,
  activePaneId: string,
): string | null {
  const activeLeaf = findLeaf(tree, activePaneId);
  if (activeLeaf) {
    const av = activeLeaf.views.find((v) => v.id === activeLeaf.activeViewId);
    if (av) return av.id;
    const fallback = activeLeaf.views[0];
    if (fallback) return fallback.id;
  }
  for (const leaf of iterLeaves(tree)) {
    const v = leaf.views[0];
    if (v) return v.id;
  }
  return null;
}

/**
 * Pin (or unpin if `tabId === undefined`) a view to a specific tab.
 */
export function setViewTabId(
  tree: PanelNode,
  viewId: string,
  tabId: string | undefined,
): PanelNode {
  const rewrite = (node: PanelNode): PanelNode => {
    if (node.type === "leaf") {
      const idx = node.views.findIndex((v) => v.id === viewId);
      if (idx === -1) return node;
      const view = node.views[idx]!;
      if (view.tabId === tabId) return node;
      const next = [...node.views];
      const updated: PanelView = { ...view };
      if (tabId === undefined) {
        delete updated.tabId;
      } else {
        updated.tabId = tabId;
      }
      next[idx] = updated;
      return { ...node, views: next };
    }
    return { ...node, children: node.children.map(rewrite) };
  };
  return rewrite(tree);
}

/**
 * Append (or insert at `index`) a tab id into a query view's strip
 * and activate it. No-op for duplicate adds.
 */
export function addTabToEditorView(
  tree: PanelNode,
  viewId: string,
  tabId: string,
  index?: number,
): PanelNode {
  const rewrite = (node: PanelNode): PanelNode => {
    if (node.type === "leaf") {
      const idx = node.views.findIndex((v) => v.id === viewId);
      if (idx === -1) return node;
      const view = node.views[idx]!;
      const tabIds = view.tabIds ?? [];
      // Already present — just activate.
      if (tabIds.includes(tabId)) {
        if (view.tabId === tabId) return node;
        const next = [...node.views];
        next[idx] = { ...view, tabId };
        return { ...node, views: next };
      }
      const at =
        index === undefined
          ? tabIds.length
          : Math.max(0, Math.min(tabIds.length, index));
      const nextTabIds = [...tabIds.slice(0, at), tabId, ...tabIds.slice(at)];
      const next = [...node.views];
      next[idx] = { ...view, tabIds: nextTabIds, tabId };
      return { ...node, views: next };
    }
    return { ...node, children: node.children.map(rewrite) };
  };
  return rewrite(tree);
}

/**
 * Remove a tab id from a query view's strip. If the removed tab was
 * the active one, the nearest sibling becomes active.
 */
export function removeTabFromEditorView(
  tree: PanelNode,
  viewId: string,
  tabId: string,
): PanelNode {
  const rewrite = (node: PanelNode): PanelNode => {
    if (node.type === "leaf") {
      const idx = node.views.findIndex((v) => v.id === viewId);
      if (idx === -1) return node;
      const view = node.views[idx]!;
      const tabIds = view.tabIds ?? [];
      const pos = tabIds.indexOf(tabId);
      if (pos === -1) return node;
      const nextTabIds = tabIds.filter((t) => t !== tabId);
      const updated: PanelView = { ...view, tabIds: nextTabIds };
      if (view.tabId === tabId) {
        const next = nextTabIds[pos] ?? nextTabIds[pos - 1] ?? undefined;
        if (next === undefined) {
          delete updated.tabId;
        } else {
          updated.tabId = next;
        }
      }
      const nextViews = [...node.views];
      nextViews[idx] = updated;
      return { ...node, views: nextViews };
    }
    return { ...node, children: node.children.map(rewrite) };
  };
  return rewrite(tree);
}

/**
 * Reorder an editor view's tab from `fromIndex` to `toIndex`.
 */
export function reorderTabInEditorView(
  tree: PanelNode,
  viewId: string,
  fromIndex: number,
  toIndex: number,
): PanelNode {
  const rewrite = (node: PanelNode): PanelNode => {
    if (node.type === "leaf") {
      const idx = node.views.findIndex((v) => v.id === viewId);
      if (idx === -1) return node;
      const view = node.views[idx]!;
      const tabIds = view.tabIds ?? [];
      const len = tabIds.length;
      if (
        fromIndex === toIndex ||
        fromIndex < 0 ||
        fromIndex >= len ||
        toIndex < 0 ||
        toIndex >= len
      ) {
        return node;
      }
      const next = [...tabIds];
      const [moved] = next.splice(fromIndex, 1);
      if (moved === undefined) return node;
      next.splice(toIndex, 0, moved);
      const nextViews = [...node.views];
      nextViews[idx] = { ...view, tabIds: next };
      return { ...node, views: nextViews };
    }
    return { ...node, children: node.children.map(rewrite) };
  };
  return rewrite(tree);
}

/** Returns true when `tabId` appears in any query view's strip. */
export function tabIsOpenInAnyEditorView(
  tree: PanelNode,
  tabId: string,
): boolean {
  for (const leaf of iterLeaves(tree)) {
    for (const v of leaf.views) {
      if ((v.tabIds ?? []).includes(tabId)) return true;
    }
  }
  return false;
}

/**
 * Adapter exposing each leaf as a "cell" — kept for call sites that
 * iterate cells and read the editor/result view ids. In the unified
 * model a leaf IS the cell, so editor and result point at the same
 * underlying view.
 */
export interface WorkspaceCellView {
  cellId: string;
  editorLeafId: string;
  editorView: PanelView;
  resultLeafId: string;
  resultView: PanelView;
}

function viewToCell(leaf: PanelLeaf, view: PanelView): WorkspaceCellView {
  return {
    cellId: leaf.id,
    editorLeafId: leaf.id,
    editorView: view,
    resultLeafId: leaf.id,
    resultView: view,
  };
}

export function* iterCells(tree: PanelNode): Generator<WorkspaceCellView> {
  for (const leaf of iterLeaves(tree)) {
    for (const v of leaf.views) {
      if (v.kind === "query") yield viewToCell(leaf, v);
    }
  }
}

export function findCellForLeaf(
  tree: PanelNode,
  leafId: string,
): WorkspaceCellView | null {
  const leaf = findLeaf(tree, leafId);
  if (!leaf) return null;
  const v = leaf.views.find((view) => view.kind === "query");
  return v ? viewToCell(leaf, v) : null;
}

export function firstCell(tree: PanelNode): WorkspaceCellView | null {
  for (const cell of iterCells(tree)) return cell;
  return null;
}

/**
 * Resolve the tab id the user is "looking at" right now. Fallback order:
 *
 *   1. The active pane's active view's `tabId` (if it's an editor view).
 *   2. Any editor view inside the active pane with a `tabId`.
 *   3. The editor view of the active pane's enclosing cell.
 *   4. The first cell's editor view.
 *   5. The first editor view anywhere in the workspace.
 *
 * Returns `null` only when the workspace has no editor view at all (an
 * invariant the failsafe normally prevents).
 */
export function resolveActiveTabId(
  tree: PanelNode,
  activePaneId: string,
): string | null {
  const activeLeaf = findLeaf(tree, activePaneId);
  if (activeLeaf) {
    const av = activeLeaf.views.find((v) => v.id === activeLeaf.activeViewId);
    if (av?.tabId) return av.tabId;
    const fallback = activeLeaf.views.find((v) => v.tabId);
    if (fallback?.tabId) return fallback.tabId;
  }
  for (const leaf of iterLeaves(tree)) {
    for (const v of leaf.views) {
      if (v.tabId) return v.tabId;
    }
  }
  return null;
}

/** Count query views in the tree (one per pane in the new model). */
export function countQueryViews(tree: PanelNode): number {
  let n = 0;
  for (const leaf of iterLeaves(tree)) {
    for (const v of leaf.views) {
      if (v.kind === "query") n++;
    }
  }
  return n;
}

/** Back-compat aliases — the old model had two separate counts. */
export const countEditorViews = countQueryViews;
export const countResultViews = countQueryViews;

/**
 * Set the active view inside a leaf.
 */
export function setActiveView(
  tree: PanelNode,
  paneId: string,
  viewId: string,
): PanelNode {
  const rewrite = (node: PanelNode): PanelNode => {
    if (node.type === "leaf") {
      if (node.id !== paneId) return node;
      if (!node.views.some((v) => v.id === viewId)) return node;
      if (node.activeViewId === viewId) return node;
      return { ...node, activeViewId: viewId };
    }
    return { ...node, children: node.children.map(rewrite) };
  };
  return rewrite(tree);
}

/**
 * Garbage-collect every reference to `closedTabId` across the tree.
 *
 * Each query view's tab strip drops the id; if its active `tabId`
 * matched the closed one, the strip's nearest sibling becomes active.
 * A view whose strip empties is left in place (its leaf still owns the
 * editor surface — callers refuse the close earlier when this would
 * leave the workspace with no tabs at all).
 */
export function gcClosedTab(
  tree: PanelNode,
  closedTabId: string,
): PanelNode | null {
  let current: PanelNode | null = tree;
  const affected: string[] = [];
  for (const leaf of iterLeaves(tree)) {
    for (const v of leaf.views) {
      if ((v.tabIds ?? []).includes(closedTabId)) affected.push(v.id);
      else if (v.tabId === closedTabId) affected.push(v.id);
    }
  }
  for (const viewId of affected) {
    if (!current) return null;
    current = removeTabFromEditorView(current, viewId, closedTabId);
  }
  return current;
}

// ────────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────────

function even(n: number): number[] {
  const each = 100 / n;
  return Array.from({ length: n }, () => each);
}

function normaliseSizes(sizes: number[]): number[] {
  const positive = sizes.map((s) => Math.max(1, s));
  const total = positive.reduce((a, b) => a + b, 0);
  if (total === 0) return even(sizes.length);
  return positive.map((s) => (s * 100) / total);
}
