/**
 * Workspace tree validation — guards against malformed or stale layouts
 * (rolled back schema, corrupt IDB record, edge-case bug producing a
 * tree shape we can't render).
 *
 * The validator is intentionally strict: any anomaly returns a non-null
 * reason, and the caller (Workbench bootstrap) treats that as a signal
 * to drop the persisted layout and reseed from `buildDefaultWorkspace`.
 * Better a transient layout reset than a permanently broken workbench.
 */

import {
  countQueryViews,
  findLeaf,
  iterLeaves,
  type PanelGroup,
  type PanelLeaf,
  type PanelNode,
  type PanelView,
} from "./tree";

/**
 * Returns `null` when the tree is well-formed, or a short human-readable
 * reason string when something is wrong. The reason is suitable for a
 * dev console warning; we don't surface it to end users.
 */
export function validateWorkspace(
  tree: unknown,
  ctx?: { activePaneId?: string; tabIds?: Set<string> },
): string | null {
  if (!isPanelNode(tree)) return "workspace is not a panel node";

  const seenLeafIds = new Set<string>();
  const seenGroupIds = new Set<string>();
  const seenViewIds = new Set<string>();

  const walk = (node: PanelNode, depth: number): string | null => {
    if (depth > 100) return "workspace tree is too deep (cycle?)";
    if (node.type === "leaf") {
      if (!node.id || typeof node.id !== "string") return "leaf without id";
      if (seenLeafIds.has(node.id)) return `duplicate leaf id ${node.id}`;
      seenLeafIds.add(node.id);
      if (!Array.isArray(node.views) || node.views.length === 0) {
        return `leaf ${node.id} has no views`;
      }
      let activeFound = false;
      for (const view of node.views) {
        const reason = validateView(view, seenViewIds, ctx?.tabIds);
        if (reason) return reason;
        if (view.id === node.activeViewId) activeFound = true;
      }
      if (!activeFound) return `leaf ${node.id} activeViewId not in views`;
      return null;
    }
    if (!node.id || typeof node.id !== "string") return "group without id";
    if (seenGroupIds.has(node.id)) return `duplicate group id ${node.id}`;
    seenGroupIds.add(node.id);
    if (node.direction !== "row" && node.direction !== "column") {
      return `group ${node.id} has invalid direction`;
    }
    if (!Array.isArray(node.children) || node.children.length < 2) {
      return `group ${node.id} must contain ≥2 children`;
    }
    if (
      !Array.isArray(node.sizes) ||
      node.sizes.length !== node.children.length
    ) {
      return `group ${node.id} sizes/children length mismatch`;
    }
    for (const size of node.sizes) {
      if (typeof size !== "number" || !Number.isFinite(size) || size < 0) {
        return `group ${node.id} has invalid size`;
      }
    }
    for (const child of node.children) {
      const reason = walk(child, depth + 1);
      if (reason) return reason;
    }
    return null;
  };

  const reason = walk(tree, 0);
  if (reason) return reason;

  // Required invariant: at least one query pane.
  if (countQueryViews(tree) < 1) return "workspace has no query view";

  // Active pane id (if provided) must resolve.
  if (ctx?.activePaneId && !findLeaf(tree, ctx.activePaneId)) {
    return "activePaneId does not match any leaf";
  }

  // The first query view should have at least one tab in its strip so
  // the user has somewhere to land. If not, the bootstrap's
  // healOrphanedTabs plugs the gap.
  let firstHasTab = false;
  for (const leaf of iterLeaves(tree)) {
    for (const view of leaf.views) {
      if (view.kind === "query") {
        if ((view.tabIds ?? []).length > 0) firstHasTab = true;
        break;
      }
    }
    if (firstHasTab) break;
  }
  if (!firstHasTab && ctx?.tabIds && ctx.tabIds.size > 0) {
    // Tabs exist but aren't in any strip — healing covers this; don't fail.
  }

  return null;
}

function isPanelNode(value: unknown): value is PanelNode {
  if (!value || typeof value !== "object") return false;
  const t = (value as { type?: unknown }).type;
  return t === "leaf" || t === "group";
}

function validateView(
  view: unknown,
  seen: Set<string>,
  knownTabIds: Set<string> | undefined,
): string | null {
  if (!view || typeof view !== "object") return "view is not an object";
  const v = view as PanelView;
  if (!v.id || typeof v.id !== "string") return "view without id";
  if (seen.has(v.id)) return `duplicate view id ${v.id}`;
  seen.add(v.id);
  if (v.kind !== "query") {
    return `view ${v.id} has invalid kind`;
  }
  if (v.tabIds !== undefined && !Array.isArray(v.tabIds)) {
    return `view ${v.id} tabIds must be an array`;
  }
  const tabIds = v.tabIds ?? [];
  if (v.tabId !== undefined && tabIds.length > 0 && !tabIds.includes(v.tabId)) {
    return `view ${v.id} active tabId not in tabIds`;
  }
  if (knownTabIds) {
    for (const id of tabIds) {
      if (typeof id !== "string")
        return `view ${v.id} tabIds contains non-string`;
      if (!knownTabIds.has(id)) {
        return `view ${v.id} references missing tab ${id}`;
      }
    }
  }
  if (v.resultTab !== undefined) {
    if (
      v.resultTab !== "graph" &&
      v.resultTab !== "table" &&
      v.resultTab !== "json" &&
      v.resultTab !== "plan"
    ) {
      return `view ${v.id} has invalid resultTab`;
    }
  }
  return null;
}

// Re-export the layered helpers so callers can build the `ctx.tabIds`
// without importing tree.ts separately.
export type { PanelGroup, PanelLeaf, PanelNode };
