/**
 * Default workspace tree + legacy migration.
 *
 * The seed layout is a single query leaf: one pane that owns its tab
 * strip, an editor surface and a result region. Splits create more
 * query leaves; editor and result are never separate panes.
 *
 * `migrateLayout` accepts a raw value plucked from IDB and returns a
 * canonical {@link SerializedLayout}. It rewrites old shapes —
 * pre-window-management `panelSizes.editorSplit`, and the intermediate
 * "editor + result column cell" tree — into the unified query model.
 */

import type { ResultTab, SerializedLayout } from "@/lib/state/slices/layout";

import {
  addTabToEditorView,
  firstEditorView,
  iterLeaves,
  makeLeaf,
  makeView,
  type PanelLeaf,
  type PanelNode,
  type SplitDirection,
} from "./tree";

/** Build a default workspace tree (one query leaf). */
export function buildDefaultWorkspace(opts?: {
  /** Initial result-tab selection for the seeded query. */
  resultTab?: ResultTab;
  /** Editor-to-result height split for the seeded query (0-100). */
  editorSizePct?: number;
  /** Seed the editor tab strip with these ids. */
  editorTabIds?: string[];
  /** Active tab inside the seeded query view. */
  editorActiveTabId?: string;
  /**
   * Direction is accepted for API compat with callers that used to
   * choose how editor and result were oriented. The new model has a
   * single leaf, so the value is ignored — kept to avoid churn at
   * call sites.
   */
  direction?: SplitDirection;
  /** Two-tuple sizes from legacy split. Ignored in the new model. */
  sizes?: [number, number];
}): {
  workspace: PanelNode;
  editorLeafId: string;
  resultLeafId: string;
  editorViewId: string;
  resultViewId: string;
} {
  const view = makeView({
    kind: "query",
    tabIds: opts?.editorTabIds ?? [],
    resultTab: opts?.resultTab ?? "graph",
    ...(opts?.editorActiveTabId !== undefined ? { tabId: opts.editorActiveTabId } : {}),
    ...(opts?.editorSizePct !== undefined ? { editorSizePct: opts.editorSizePct } : {}),
  });
  const leaf: PanelLeaf = makeLeaf([view]);
  return {
    workspace: leaf,
    editorLeafId: leaf.id,
    resultLeafId: leaf.id,
    editorViewId: view.id,
    resultViewId: view.id,
  };
}

export const DEFAULT_SIDEBAR_WIDTH = 280;
export const MIN_SIDEBAR_WIDTH = 180;
export const MAX_SIDEBAR_WIDTH = 520;

/**
 * Build a canonical `SerializedLayout` from possibly-legacy input. The
 * caller is responsible for assigning this back to the slice.
 */
export function migrateLayout(raw: unknown): SerializedLayout {
  const v = (raw ?? {}) as Partial<SerializedLayout> & {
    panelSizes?: Record<string, number>;
    resultTab?: ResultTab;
    splitOrientation?: SplitDirection;
  };

  if (v.workspace && (v.workspace.type === "leaf" || v.workspace.type === "group")) {
    const collapsed = collapseEditorResultCells(v.workspace);
    return {
      activitySection: v.activitySection ?? "queries",
      sidebarOpen: v.sidebarOpen ?? true,
      sidebarWidth: clampWidth(v.sidebarWidth),
      workspace: collapsed,
      activePaneId: v.activePaneId && findLeafById(collapsed, v.activePaneId)
        ? v.activePaneId
        : findFirstLeafId(collapsed),
    };
  }

  // Pre-window-management blob: a single editor split fraction + a
  // global result-tab choice. Rebuild as one query pane carrying that
  // split and tab choice.
  const editorFraction = clamp(v.panelSizes?.editorSplit ?? 0.5, 0.15, 0.85);
  const { workspace, editorLeafId } = buildDefaultWorkspace({
    resultTab: v.resultTab ?? "graph",
    editorSizePct: editorFraction * 100,
  });

  return {
    activitySection: v.activitySection ?? "queries",
    sidebarOpen: v.sidebarOpen ?? true,
    sidebarWidth: clampWidth(v.sidebarWidth),
    workspace,
    activePaneId: editorLeafId,
  };
}

/**
 * Collapse any legacy "editor leaf + result leaf in a column" cells
 * into a single query leaf. Walks recursively so nested cells in a
 * larger split layout migrate cleanly.
 */
function collapseEditorResultCells(node: PanelNode): PanelNode {
  if (node.type === "leaf") {
    return normaliseLeaf(node);
  }
  if (node.type === "group") {
    const cellView = tryReadCellGroup(node);
    if (cellView) return cellView;
    return {
      ...node,
      children: node.children.map(collapseEditorResultCells),
    };
  }
  return node;
}

/**
 * If `group` matches the legacy "column with one editor leaf + one
 * result leaf" shape, return a single query leaf carrying the merged
 * state. Otherwise return null.
 */
function tryReadCellGroup(group: Extract<PanelNode, { type: "group" }>): PanelNode | null {
  if (group.direction !== "column") return null;
  if (group.children.length !== 2) return null;
  const [a, b] = group.children;
  if (!a || !b || a.type !== "leaf" || b.type !== "leaf") return null;
  const aView = a.views[0];
  const bView = b.views[0];
  if (!aView || !bView) return null;
  // The pre-unified model had `kind: "editor" | "result"`. We accept
  // either child order so old IDB blobs migrate either way.
  const aKind = (aView as unknown as { kind?: string }).kind;
  const bKind = (bView as unknown as { kind?: string }).kind;
  let editorView: typeof aView | null = null;
  let resultView: typeof aView | null = null;
  let editorSizePct = group.sizes[0] ?? 50;
  if (aKind === "editor" && bKind === "result") {
    editorView = aView;
    resultView = bView;
    editorSizePct = group.sizes[0] ?? 50;
  } else if (aKind === "result" && bKind === "editor") {
    editorView = bView;
    resultView = aView;
    editorSizePct = group.sizes[1] ?? 50;
  } else {
    return null;
  }
  const merged = makeView({
    kind: "query",
    tabIds: editorView.tabIds ?? (editorView.tabId ? [editorView.tabId] : []),
    ...(editorView.tabId !== undefined ? { tabId: editorView.tabId } : {}),
    resultTab: resultView.resultTab ?? "graph",
    editorSizePct,
  });
  return makeLeaf([merged]);
}

/**
 * Tidy a single leaf carried over from the editor/result era: rewrite
 * non-query views (only "editor" and "result" existed historically)
 * into a query view so the rest of the app can stop branching on kind.
 */
function normaliseLeaf(leaf: PanelLeaf): PanelLeaf {
  let dirty = false;
  const views = leaf.views.map((view) => {
    const kind = (view as unknown as { kind?: string }).kind;
    if (kind === "query") return view;
    dirty = true;
    return makeView({
      kind: "query",
      tabIds: view.tabIds ?? (view.tabId ? [view.tabId] : []),
      ...(view.tabId !== undefined ? { tabId: view.tabId } : {}),
      resultTab: view.resultTab ?? "graph",
      ...(view.editorSizePct !== undefined ? { editorSizePct: view.editorSizePct } : {}),
      ...(view.resultMinimized !== undefined ? { resultMinimized: view.resultMinimized } : {}),
    });
  });
  if (!dirty) return leaf;
  const activeStillPresent = views.some((v) => v.id === leaf.activeViewId);
  return {
    ...leaf,
    views,
    activeViewId: activeStillPresent ? leaf.activeViewId : views[0]!.id,
  };
}

/**
 * Heal a workspace tree against the current set of tab ids — used by
 * the workbench bootstrap after IDB hydration:
 *
 *  - Seed every query view's `tabIds` strip from its legacy `tabId`
 *    when the strip is empty.
 *  - For every known tab not present in any query view's strip, append
 *    it to the first query view so it stays reachable.
 *  - If the first query view has no active `tabId`, activate the first
 *    entry in its strip.
 */
export function healOrphanedTabs(
  workspace: PanelNode,
  knownTabIds: ReadonlyArray<string>,
): PanelNode {
  let tree = workspace;

  for (const leaf of iterLeaves(tree)) {
    for (const v of leaf.views) {
      if ((v.tabIds ?? []).length > 0) continue;
      if (!v.tabId) continue;
      tree = addTabToEditorView(tree, v.id, v.tabId);
    }
  }

  const first = firstEditorView(tree);
  if (first) {
    const seen = new Set<string>();
    for (const leaf of iterLeaves(tree)) {
      for (const v of leaf.views) {
        for (const id of v.tabIds ?? []) seen.add(id);
      }
    }
    for (const tabId of knownTabIds) {
      if (seen.has(tabId)) continue;
      tree = addTabToEditorView(tree, first.view.id, tabId);
      seen.add(tabId);
    }
  }

  const refreshed = firstEditorView(tree);
  if (refreshed) {
    const strip = refreshed.view.tabIds ?? [];
    if (!refreshed.view.tabId && strip.length > 0) {
      tree = addTabToEditorView(tree, refreshed.view.id, strip[0]!);
    }
  }

  return tree;
}

function clamp(n: number, min: number, max: number): number {
  if (Number.isNaN(n)) return (min + max) / 2;
  return Math.min(max, Math.max(min, n));
}

function clampWidth(n: number | undefined): number {
  return clamp(n ?? DEFAULT_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
}

function findFirstLeafId(node: PanelNode): string {
  if (node.type === "leaf") return node.id;
  return findFirstLeafId(node.children[0]!);
}

function findLeafById(node: PanelNode, id: string): boolean {
  if (node.type === "leaf") return node.id === id;
  return node.children.some((c) => findLeafById(c, id));
}
