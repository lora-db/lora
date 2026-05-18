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

export type PanelKind = "editor" | "result";

export interface PanelView {
  /** Stable view id, ULID. */
  id: string;
  kind: PanelKind;
  /**
   * For editor views: the active tab in this pane (must be a member of
   * `tabIds`). For result views: a specific tab to follow, or
   * `undefined` to follow the active editor pane's tab.
   */
  tabId?: string;
  /**
   * Editor-only: ordered list of tab ids visible in this pane's tab
   * strip. The same tab id can appear in multiple editor views (a tab
   * being "open" in two panes); the underlying body is still shared.
   */
  tabIds?: string[];
  /** For result views only: which inner tab (Graph/Table/JSON/Plan). */
  resultTab?: ResultTab;
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

export function makeView(input: Partial<PanelView> & { kind: PanelKind }): PanelView {
  const view: PanelView = {
    id: input.id ?? ulid(),
    kind: input.kind,
  };
  if (input.tabId !== undefined) view.tabId = input.tabId;
  if (input.kind === "editor") {
    view.tabIds = input.tabIds ? [...input.tabIds] : [];
  }
  if (input.kind === "result") {
    view.resultTab = input.resultTab ?? "graph";
  }
  return view;
}

export function makeLeaf(views: PanelView[], opts?: { id?: string; activeViewId?: string }): PanelLeaf {
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

export function findViewLeaf(node: PanelNode, viewId: string): PanelLeaf | null {
  for (const leaf of iterLeaves(node)) {
    if (leaf.views.some((v) => v.id === viewId)) return leaf;
  }
  return null;
}

export function findFirstResultView(node: PanelNode): { leaf: PanelLeaf; view: PanelView } | null {
  for (const leaf of iterLeaves(node)) {
    for (const v of leaf.views) {
      if (v.kind === "result") return { leaf, view: v };
    }
  }
  return null;
}

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

  const seedView =
    newView ??
    cloneViewWithNewId(leaf.views.find((v) => v.id === leaf.activeViewId) ?? leaf.views[0]!);
  const newLeaf = makeLeaf([seedView]);

  // Walk and rewrite. When we find the parent of `leaf`, decide whether to
  // splice next to it (parent already runs in the same direction) or wrap
  // the leaf in a new group.
  const rewrite = (node: PanelNode, parentDirection: SplitDirection | null): PanelNode => {
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
    const rewrittenChildren = node.children.map((c) => rewrite(c, node.direction));
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
    const nextSizes = normaliseSizes(survivingIndices.map((i) => group.sizes[i] ?? 0));
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
      const at = index === undefined ? nextViews.length : Math.max(0, Math.min(nextViews.length, index));
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
        const at = toIndex === undefined ? remaining.length : Math.max(0, Math.min(remaining.length, toIndex));
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
 * Set a result view's inner tab.
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
      if (view.kind !== "result" || view.resultTab === resultTab) return node;
      const next = [...node.views];
      next[idx] = { ...view, resultTab };
      return { ...node, views: next };
    }
    return { ...node, children: node.children.map(rewrite) };
  };
  return rewrite(tree);
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
 * Append (or insert at `index`) a tab id into an editor view's strip
 * and activate it. No-op for non-editor views or duplicate adds.
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
      if (view.kind !== "editor") return node;
      const tabIds = view.tabIds ?? [];
      // Already present — just activate.
      if (tabIds.includes(tabId)) {
        if (view.tabId === tabId) return node;
        const next = [...node.views];
        next[idx] = { ...view, tabId };
        return { ...node, views: next };
      }
      const at = index === undefined ? tabIds.length : Math.max(0, Math.min(tabIds.length, index));
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
 * Remove a tab id from an editor view's strip. If the removed tab was
 * the active one, the nearest sibling becomes active (right if any,
 * else left, else `undefined` when the strip is empty).
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
      if (view.kind !== "editor") return node;
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
      if (view.kind !== "editor") return node;
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

/** Returns true when `tabId` appears in any editor view's strip. */
export function tabIsOpenInAnyEditorView(tree: PanelNode, tabId: string): boolean {
  for (const leaf of iterLeaves(tree)) {
    for (const v of leaf.views) {
      if (v.kind === "editor" && (v.tabIds ?? []).includes(tabId)) return true;
    }
  }
  return false;
}

/** Find the first editor view in the tree (used as the destination for new tabs when no pane is active). */
export function firstEditorView(tree: PanelNode): { leaf: PanelLeaf; view: PanelView } | null {
  for (const leaf of iterLeaves(tree)) {
    for (const v of leaf.views) {
      if (v.kind === "editor") return { leaf, view: v };
    }
  }
  return null;
}

// ────────────────────────────────────────────────────────────────
// Cell helpers
//
// A "cell" is a column group containing exactly one editor leaf and
// one result leaf — i.e. a paired workspace. These helpers let other
// code reason about cells without dipping into the recursive PanelNode
// model. The on-disk shape is still the generic tree; cells are just
// a stricter view of it.
// ────────────────────────────────────────────────────────────────

export interface WorkspaceCellView {
  /** Group id (the column wrapping editor+result). */
  cellId: string;
  editorLeafId: string;
  editorView: PanelView;
  resultLeafId: string;
  resultView: PanelView;
}

/** A cell is a column group of one editor leaf + one result leaf. */
export function asCell(node: PanelNode): WorkspaceCellView | null {
  if (node.type !== "group") return null;
  if (node.direction !== "column") return null;
  if (node.children.length !== 2) return null;
  const [a, b] = node.children;
  if (!a || !b || a.type !== "leaf" || b.type !== "leaf") return null;
  const aView = a.views[0];
  const bView = b.views[0];
  if (!aView || !bView) return null;
  if (aView.kind === "editor" && bView.kind === "result") {
    return {
      cellId: node.id,
      editorLeafId: a.id,
      editorView: aView,
      resultLeafId: b.id,
      resultView: bView,
    };
  }
  if (aView.kind === "result" && bView.kind === "editor") {
    return {
      cellId: node.id,
      editorLeafId: b.id,
      editorView: bView,
      resultLeafId: a.id,
      resultView: aView,
    };
  }
  return null;
}

/** Walk every workspace cell in the tree (depth-first). */
export function* iterCells(tree: PanelNode): Generator<WorkspaceCellView> {
  const cell = asCell(tree);
  if (cell) {
    yield cell;
    return;
  }
  if (tree.type === "group") {
    for (const child of tree.children) {
      yield* iterCells(child);
    }
  }
}

/** Find the cell that contains the given leaf id (editor or result). */
export function findCellForLeaf(tree: PanelNode, leafId: string): WorkspaceCellView | null {
  for (const cell of iterCells(tree)) {
    if (cell.editorLeafId === leafId || cell.resultLeafId === leafId) return cell;
  }
  return null;
}

/** Find the cell whose editor view contains `viewId`, or whose result view IS `viewId`. */
export function findCellForView(tree: PanelNode, viewId: string): WorkspaceCellView | null {
  for (const cell of iterCells(tree)) {
    if (cell.editorView.id === viewId || cell.resultView.id === viewId) return cell;
  }
  return null;
}

/** First cell in the tree, or null if none. */
export function firstCell(tree: PanelNode): WorkspaceCellView | null {
  for (const cell of iterCells(tree)) return cell;
  return null;
}

/** Find the cell whose editor leaf is `leafId`, or null if it's a result leaf or unknown. */
export function findCellByEditorLeaf(tree: PanelNode, leafId: string): WorkspaceCellView | null {
  for (const cell of iterCells(tree)) {
    if (cell.editorLeafId === leafId) return cell;
  }
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
    if (av?.kind === "editor" && av.tabId) return av.tabId;
    const sameLeafEditor = activeLeaf.views.find(
      (v) => v.kind === "editor" && v.tabId,
    );
    if (sameLeafEditor?.tabId) return sameLeafEditor.tabId;
    const cell = findCellForLeaf(tree, activeLeaf.id);
    if (cell?.editorView.tabId) return cell.editorView.tabId;
  }
  const first = firstCell(tree);
  if (first?.editorView.tabId) return first.editorView.tabId;
  for (const leaf of iterLeaves(tree)) {
    for (const v of leaf.views) {
      if (v.kind === "editor" && v.tabId) return v.tabId;
    }
  }
  return null;
}

/** Count editor views in the tree. */
export function countEditorViews(tree: PanelNode): number {
  let n = 0;
  for (const leaf of iterLeaves(tree)) {
    for (const v of leaf.views) {
      if (v.kind === "editor") n++;
    }
  }
  return n;
}

/** Count result views in the tree. */
export function countResultViews(tree: PanelNode): number {
  let n = 0;
  for (const leaf of iterLeaves(tree)) {
    for (const v of leaf.views) {
      if (v.kind === "result") n++;
    }
  }
  return n;
}

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
 * Garbage-collect any reference to `closedTabId` across the tree.
 *
 *  - **Editor views**: drop the tab id from their `tabIds`. If the
 *    strip becomes empty, the editor view is removed (and its
 *    containing leaf collapses).
 *  - **Result views**: do NOT remove or unpin — instead, repoint each
 *    affected result view to the *current* `tabId` of the editor view
 *    in the same cell, so the result pane keeps showing whatever its
 *    paired editor is showing. If no companion editor exists (which
 *    shouldn't happen in canonical cell layouts), the result view is
 *    left alone.
 */
export function gcClosedTab(tree: PanelNode, closedTabId: string): PanelNode | null {
  let current: PanelNode | null = tree;

  const editorViewIds: string[] = [];
  const resultViewRepoints: { viewId: string; newTabId: string | undefined }[] = [];
  const cellResultIds = new Set<string>();
  for (const leaf of iterLeaves(tree)) {
    for (const v of leaf.views) {
      if (v.kind === "editor" && (v.tabIds ?? []).includes(closedTabId)) {
        editorViewIds.push(v.id);
      }
    }
  }
  // Walk cells to figure out where each *cell-bound* result view should repoint.
  for (const cell of iterCells(tree)) {
    cellResultIds.add(cell.resultView.id);
    if (cell.resultView.tabId !== closedTabId) continue;
    // Calculate the editor's tab id post-gc: keep its current `tabId`
    // unless that's the closed tab, in which case the strip's nearest
    // sibling takes over (matching `removeTabFromEditorView` behaviour).
    const editorTabIds = cell.editorView.tabIds ?? [];
    const pos = editorTabIds.indexOf(closedTabId);
    const remaining = editorTabIds.filter((t) => t !== closedTabId);
    let projectedActive: string | undefined = cell.editorView.tabId;
    if (cell.editorView.tabId === closedTabId) {
      projectedActive = remaining[pos] ?? remaining[pos - 1] ?? undefined;
    }
    resultViewRepoints.push({
      viewId: cell.resultView.id,
      newTabId: projectedActive,
    });
  }
  // For result views NOT inside a canonical cell (legacy layouts /
  // tests), fall back to the old unpin behaviour so they don't keep
  // pointing at a now-deleted tab.
  for (const leaf of iterLeaves(tree)) {
    for (const v of leaf.views) {
      if (v.kind !== "result") continue;
      if (v.tabId !== closedTabId) continue;
      if (cellResultIds.has(v.id)) continue;
      resultViewRepoints.push({ viewId: v.id, newTabId: undefined });
    }
  }

  // Editor views: strip the tab id; remove the view if its strip is now empty.
  for (const viewId of editorViewIds) {
    if (!current) return null;
    current = removeTabFromEditorView(current, viewId, closedTabId);
    if (!current) return null;
    const leaf = findViewLeaf(current, viewId);
    const view = leaf?.views.find((v) => v.id === viewId);
    if (view && view.kind === "editor" && (view.tabIds ?? []).length === 0) {
      current = removeView(current, viewId);
    }
  }

  // Result views: repoint each one to its companion editor's tab.
  for (const { viewId, newTabId } of resultViewRepoints) {
    if (!current) return null;
    current = setViewTabId(current, viewId, newTabId);
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

function cloneViewWithNewId(view: PanelView): PanelView {
  const next: PanelView = { id: ulid(), kind: view.kind };
  if (view.tabId !== undefined) next.tabId = view.tabId;
  if (view.resultTab !== undefined) next.resultTab = view.resultTab;
  return next;
}
