/**
 * Default workspace tree + legacy migration.
 *
 * The seed layout reproduces the pre-window-management UI: one editor
 * leaf above one result leaf, both views unpinned (i.e. follow the
 * global active tab).
 *
 * `migrateLayout` accepts a raw value plucked from IDB and returns a
 * canonical {@link SerializedLayout}. The first IDB write after a
 * legacy load persists the new shape, so the migration is a one-shot
 * per browser profile.
 */

import type { ResultTab, SerializedLayout } from "@/lib/state/slices/layout";

import {
  addTabToEditorView,
  firstEditorView,
  iterLeaves,
  makeGroup,
  makeLeaf,
  makeView,
  type PanelLeaf,
  type PanelNode,
  type SplitDirection,
} from "./tree";

/** Build a default workspace tree (editor on top, result below). */
export function buildDefaultWorkspace(opts?: {
  direction?: SplitDirection;
  resultTab?: ResultTab;
  sizes?: [number, number];
  /** Seed the editor view's tab strip with these ids. */
  editorTabIds?: string[];
  /** Active tab inside the seeded editor view. */
  editorActiveTabId?: string;
}): {
  workspace: PanelNode;
  editorLeafId: string;
  resultLeafId: string;
  editorViewId: string;
  resultViewId: string;
} {
  const editorView = makeView({
    kind: "editor",
    tabIds: opts?.editorTabIds ?? [],
    ...(opts?.editorActiveTabId !== undefined ? { tabId: opts.editorActiveTabId } : {}),
  });
  const resultView = makeView({ kind: "result", resultTab: opts?.resultTab ?? "graph" });
  const editorLeaf: PanelLeaf = makeLeaf([editorView]);
  const resultLeaf: PanelLeaf = makeLeaf([resultView]);
  const direction = opts?.direction ?? "column";
  const sizes = opts?.sizes ?? [50, 50];
  return {
    workspace: makeGroup(direction, [editorLeaf, resultLeaf], { sizes }),
    editorLeafId: editorLeaf.id,
    resultLeafId: resultLeaf.id,
    editorViewId: editorView.id,
    resultViewId: resultView.id,
  };
}

export const DEFAULT_SIDEBAR_WIDTH = 280;
export const MIN_SIDEBAR_WIDTH = 180;
export const MAX_SIDEBAR_WIDTH = 520;

/**
 * Build a canonical `SerializedLayout` from possibly-legacy input. The
 * caller is responsible for assigning this back to the slice.
 *
 * Accepts:
 *  - the new shape (`workspace` present) → passed through with defaults filled in
 *  - the legacy shape (`panelSizes.editorSplit` / global `resultTab`) → synthesised
 *  - missing / malformed input → DEFAULT_LAYOUT
 */
export function migrateLayout(raw: unknown): SerializedLayout {
  const v = (raw ?? {}) as Partial<SerializedLayout> & {
    panelSizes?: Record<string, number>;
    resultTab?: ResultTab;
    /** Phase-1 transient shape — never shipped without `workspace`, but defensive. */
    splitOrientation?: SplitDirection;
  };

  // Already on the new shape — accept it (but defensively fill the new
  // chrome fields when missing so older betas can roll forward).
  if (v.workspace && (v.workspace.type === "leaf" || v.workspace.type === "group")) {
    return {
      activitySection: v.activitySection ?? "queries",
      sidebarOpen: v.sidebarOpen ?? true,
      sidebarWidth: clampWidth(v.sidebarWidth),
      workspace: v.workspace,
      activePaneId: v.activePaneId ?? findFirstLeafId(v.workspace),
    };
  }

  // Legacy path: synthesize a workspace from the old single fraction + global resultTab.
  // Note: we don't yet have the tab ids here (the tabs slice hydrates
  // separately). The Workbench bootstrap will populate the editor view
  // with whatever tab list is in scope after hydration.
  const editorFraction = clamp(v.panelSizes?.editorSplit ?? 0.5, 0.15, 0.85);
  const { workspace, editorLeafId } = buildDefaultWorkspace({
    direction: v.splitOrientation ?? "column",
    resultTab: v.resultTab ?? "graph",
    sizes: [editorFraction * 100, (1 - editorFraction) * 100],
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
 * Heal a workspace tree against the current set of tab ids — used by
 * the workbench bootstrap after IDB hydration:
 *
 *  - Seed every editor view's `tabIds` strip from its legacy `tabId`
 *    when the strip is empty (sessions persisted before per-pane strips).
 *  - For every known tab not present in any editor view's strip, append
 *    it to the first editor view so it stays reachable.
 *  - If the first editor view has no active `tabId`, activate the first
 *    entry in its strip.
 *
 * The function is pure: it returns a new tree (or the same reference
 * when nothing needed to change) so callers can decide whether to write.
 */
export function healOrphanedTabs(
  workspace: PanelNode,
  knownTabIds: ReadonlyArray<string>,
): PanelNode {
  let tree = workspace;

  // Step 1 — seed empty strips from legacy single `tabId`.
  for (const leaf of iterLeaves(tree)) {
    for (const v of leaf.views) {
      if (v.kind !== "editor") continue;
      if ((v.tabIds ?? []).length > 0) continue;
      if (!v.tabId) continue;
      tree = addTabToEditorView(tree, v.id, v.tabId);
    }
  }

  // Step 2 — fold tabs that aren't open in any editor view's strip into
  // the first editor view so the user can still reach them.
  const first = firstEditorView(tree);
  if (first) {
    const seen = new Set<string>();
    for (const leaf of iterLeaves(tree)) {
      for (const v of leaf.views) {
        if (v.kind !== "editor") continue;
        for (const id of v.tabIds ?? []) seen.add(id);
      }
    }
    for (const tabId of knownTabIds) {
      if (seen.has(tabId)) continue;
      tree = addTabToEditorView(tree, first.view.id, tabId);
      seen.add(tabId);
    }
  }

  // Step 3 — if the first editor view's strip is non-empty but has no
  // active selection, light up the first entry.
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
