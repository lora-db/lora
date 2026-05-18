/**
 * Unit coverage for the workspace tree mutators + the layout slice's
 * pane reducers. The pure tree helpers are exercised directly; the
 * slice glue is checked by spinning up an isolated slice and walking
 * it through the same scenarios end-to-end.
 *
 * The data model has one view kind — "query" — that owns the editor
 * surface, the tab strip and the result region as a single bound
 * pane. The old "editor" / "result" split is exercised here through
 * the migration helpers (`migrateLayout`) only.
 */

import { describe, expect, it } from "vitest";
import { create } from "zustand";
import { immer } from "zustand/middleware/immer";

import {
  closeLeaf,
  findLeaf,
  findViewLeaf,
  flatLeafIds,
  gcClosedTab,
  insertView,
  makeGroup,
  makeLeaf,
  makeView,
  moveView,
  removeView,
  setGroupDirection,
  setGroupSizes,
  setViewEditorSizePct,
  setViewResultMinimized,
  setViewResultTab,
  setViewTabId,
  splitLeaf,
  type PanelGroup,
  type PanelLeaf,
  type PanelNode,
  type PanelView,
} from "@/lib/state/workspace/tree";
import {
  buildDefaultWorkspace,
  healOrphanedTabs,
  migrateLayout,
} from "@/lib/state/workspace/default";
import { validateWorkspace } from "@/lib/state/workspace/validate";
import {
  createLayoutSlice,
  type LayoutSlice,
} from "@/lib/state/slices/layout";
import {
  createTabsSlice,
  type TabsSlice,
} from "@/lib/state/slices/tabs";

function leaf(id?: string): PanelLeaf {
  const v = makeView({ kind: "query" });
  return makeLeaf([v], id !== undefined ? { id } : undefined);
}

function leafIds(node: PanelNode): string[] {
  return flatLeafIds(node);
}

describe("workspace tree — splitLeaf", () => {
  it("wraps a lone leaf in a row group on split-right", () => {
    const a = leaf("a");
    const out = splitLeaf(a, "a", "row", "after")!;
    expect(out.tree.type).toBe("group");
    const group = out.tree as PanelGroup;
    expect(group.direction).toBe("row");
    expect(group.children).toHaveLength(2);
    expect(group.children[0]!.id).toBe("a");
    expect(out.newLeafId).toBe(group.children[1]!.id);
  });

  it("splices a new sibling into an existing same-direction group instead of nesting", () => {
    const a = leaf("a");
    const b = leaf("b");
    const group = makeGroup("row", [a, b], { sizes: [50, 50] });
    const out = splitLeaf(group, "a", "row", "after")!;
    expect(out.tree.type).toBe("group");
    const next = out.tree as PanelGroup;
    expect(next.children).toHaveLength(3);
    expect(next.children.map((c) => c.id)).toEqual(["a", out.newLeafId, "b"]);
    expect(next.sizes[0]).toBeCloseTo(25, 0);
    expect(next.sizes[1]).toBeCloseTo(25, 0);
    expect(next.sizes[2]).toBeCloseTo(50, 0);
  });

  it("nests a new group when splitting in the perpendicular direction", () => {
    const a = leaf("a");
    const b = leaf("b");
    const group = makeGroup("row", [a, b]);
    const out = splitLeaf(group, "a", "column", "after")!;
    const root = out.tree as PanelGroup;
    expect(root.direction).toBe("row");
    const nested = root.children[0]!;
    expect(nested.type).toBe("group");
    expect((nested as PanelGroup).direction).toBe("column");
    expect((nested as PanelGroup).children.map((c) => c.id)).toEqual(["a", out.newLeafId]);
  });
});

describe("workspace tree — closeLeaf", () => {
  it("removes a leaf and collapses the parent when only one child remains", () => {
    const a = leaf("a");
    const b = leaf("b");
    const group = makeGroup("row", [a, b]);
    const next = closeLeaf(group, "b");
    expect(next).not.toBeNull();
    expect(next!.type).toBe("leaf");
    expect((next as PanelLeaf).id).toBe("a");
  });

  it("preserves the group when ≥2 children remain", () => {
    const a = leaf("a");
    const b = leaf("b");
    const c = leaf("c");
    const group = makeGroup("row", [a, b, c]);
    const next = closeLeaf(group, "b");
    expect(next!.type).toBe("group");
    expect(leafIds(next!)).toEqual(["a", "c"]);
  });

  it("returns null when removing the only remaining leaf", () => {
    const a = leaf("a");
    const next = closeLeaf(a, "a");
    expect(next).toBeNull();
  });
});

describe("workspace tree — setGroupSizes / setGroupDirection", () => {
  it("setGroupSizes normalises the array and rewrites the right group", () => {
    const a = leaf("a");
    const b = leaf("b");
    const group = makeGroup("row", [a, b], { sizes: [50, 50] });
    const next = setGroupSizes(group, group.id, [70, 30]) as PanelGroup;
    expect(next.sizes[0]).toBeCloseTo(70, 0);
    expect(next.sizes[1]).toBeCloseTo(30, 0);
  });

  it("setGroupDirection flips the group's direction", () => {
    const a = leaf("a");
    const b = leaf("b");
    const group = makeGroup("row", [a, b]);
    const next = setGroupDirection(group, group.id, "column") as PanelGroup;
    expect(next.direction).toBe("column");
    expect(next.children).toHaveLength(2);
  });
});

describe("workspace tree — views", () => {
  it("setViewResultTab swaps the inner result tab", () => {
    const v = makeView({ kind: "query", resultTab: "graph" });
    const l = makeLeaf([v]);
    const next = setViewResultTab(l, v.id, "json") as PanelLeaf;
    expect(next.views[0]!.resultTab).toBe("json");
  });

  it("setViewTabId pins/unpins a view", () => {
    const v = makeView({ kind: "query" });
    const l = makeLeaf([v]);
    const pinned = setViewTabId(l, v.id, "tab-42") as PanelLeaf;
    expect(pinned.views[0]!.tabId).toBe("tab-42");
    const unpinned = setViewTabId(pinned, v.id, undefined) as PanelLeaf;
    expect(unpinned.views[0]!.tabId).toBeUndefined();
  });

  it("setViewResultMinimized toggles the minimized flag", () => {
    const v = makeView({ kind: "query" });
    const l = makeLeaf([v]);
    const min = setViewResultMinimized(l, v.id, true) as PanelLeaf;
    expect(min.views[0]!.resultMinimized).toBe(true);
    const back = setViewResultMinimized(min, v.id, false) as PanelLeaf;
    expect(back.views[0]!.resultMinimized).toBe(false);
  });

  it("setViewEditorSizePct clamps to [10, 90]", () => {
    const v = makeView({ kind: "query" });
    const l = makeLeaf([v]);
    const tooLow = setViewEditorSizePct(l, v.id, 1) as PanelLeaf;
    expect(tooLow.views[0]!.editorSizePct).toBe(10);
    const tooHigh = setViewEditorSizePct(l, v.id, 99) as PanelLeaf;
    expect(tooHigh.views[0]!.editorSizePct).toBe(90);
    const normal = setViewEditorSizePct(l, v.id, 65) as PanelLeaf;
    expect(normal.views[0]!.editorSizePct).toBeCloseTo(65, 1);
  });

  it("insertView / removeView keep activeViewId consistent", () => {
    const v1 = makeView({ kind: "query" });
    const l = makeLeaf([v1]);
    const v2 = makeView({ kind: "query" });
    const withTwo = insertView(l, l.id, v2) as PanelLeaf;
    expect(withTwo.views).toHaveLength(2);
    expect(withTwo.activeViewId).toBe(v2.id);
    const back = removeView(withTwo, v2.id) as PanelLeaf;
    expect(back.views).toHaveLength(1);
    expect(back.activeViewId).toBe(v1.id);
  });
});

describe("workspace tree — moveView", () => {
  it("moves a view from one leaf to another", () => {
    const v1 = makeView({ kind: "query" });
    const v2 = makeView({ kind: "query" });
    const l1 = makeLeaf([v1]);
    const l2 = makeLeaf([v2]);
    const group = makeGroup("row", [l1, l2]);
    const next = moveView(group, v1.id, l2.id);
    const leafWithV1 = findViewLeaf(next!, v1.id);
    expect(leafWithV1).not.toBeNull();
    expect(leafWithV1!.views.some((v) => v.id === v1.id)).toBe(true);
  });
});

describe("workspace tree — gcClosedTab", () => {
  it("drops the closed tab from a view's strip and activates a sibling", () => {
    const v = makeView({ kind: "query", tabIds: ["T1", "T2"], tabId: "T1" });
    const l = makeLeaf([v]);
    const next = gcClosedTab(l, "T1") as PanelLeaf;
    const survivor = next.views[0]!;
    expect(survivor.tabIds).toEqual(["T2"]);
    expect(survivor.tabId).toBe("T2");
  });

  it("leaves a view's strip empty (but doesn't remove the view) when its only tab closes", () => {
    const v = makeView({ kind: "query", tabIds: ["T1"], tabId: "T1" });
    const l = makeLeaf([v]);
    const next = gcClosedTab(l, "T1") as PanelLeaf;
    expect(next.views).toHaveLength(1);
    expect(next.views[0]!.tabIds).toEqual([]);
    expect(next.views[0]!.tabId).toBeUndefined();
  });
});

describe("validateWorkspace", () => {
  it("accepts a well-formed default tree", () => {
    const { workspace } = buildDefaultWorkspace({
      editorTabIds: ["T1"],
      editorActiveTabId: "T1",
    });
    expect(validateWorkspace(workspace, { tabIds: new Set(["T1"]) })).toBeNull();
  });

  it("rejects a tree with no query view", () => {
    const tree = {
      type: "leaf" as const,
      id: "bad",
      views: [],
      activeViewId: "x",
    };
    expect(validateWorkspace(tree, { tabIds: new Set() })).toMatch(/no views|invalid|≥1/);
  });

  it("rejects a view referencing a missing tab", () => {
    const tree = makeLeaf([
      makeView({ kind: "query", tabIds: ["MISSING"], tabId: "MISSING" }),
    ]);
    expect(validateWorkspace(tree, { tabIds: new Set(["OTHER"]) })).toMatch(/missing tab/);
  });

  it("rejects a group with only one child", () => {
    const tree = {
      type: "group",
      id: "g1",
      direction: "row",
      sizes: [100],
      children: [makeLeaf([makeView({ kind: "query" })])],
    };
    expect(validateWorkspace(tree)).toMatch(/≥2 children/);
  });
});

describe("healOrphanedTabs", () => {
  it("seeds an empty strip from a legacy single tabId", () => {
    const view = makeView({ kind: "query", tabId: "t1" });
    expect(view.tabIds).toEqual([]);
    const root = makeLeaf([view]);
    const healed = healOrphanedTabs(root, ["t1"]) as PanelLeaf;
    expect(healed.views[0]!.tabIds).toEqual(["t1"]);
    expect(healed.views[0]!.tabId).toBe("t1");
  });

  it("appends tabs that aren't open in any view to the first one", () => {
    const view = makeView({ kind: "query", tabIds: ["a"], tabId: "a" });
    const root = makeLeaf([view]);
    const healed = healOrphanedTabs(root, ["a", "b", "c"]) as PanelLeaf;
    expect(healed.views[0]!.tabIds).toEqual(["a", "b", "c"]);
  });

  it("activates the first strip entry when no tabId is set", () => {
    const view = makeView({ kind: "query", tabIds: ["a"], tabId: "a" });
    delete view.tabId;
    const root = makeLeaf([view]);
    const healed = healOrphanedTabs(root, ["a"]) as PanelLeaf;
    expect(healed.views[0]!.tabId).toBe("a");
  });

  it("returns the same reference when nothing needs healing", () => {
    const view = makeView({ kind: "query", tabIds: ["a"], tabId: "a" });
    const root = makeLeaf([view]);
    expect(healOrphanedTabs(root, ["a"])).toBe(root);
  });
});

describe("migrateLayout", () => {
  it("accepts the new shape verbatim", () => {
    const { workspace } = buildDefaultWorkspace();
    const out = migrateLayout({
      activitySection: "schema",
      sidebarOpen: false,
      workspace,
      activePaneId: (workspace as PanelLeaf).id,
    });
    expect(out.activitySection).toBe("schema");
    expect(out.sidebarOpen).toBe(false);
    expect(out.workspace).toBe(workspace);
  });

  it("collapses a legacy editor+result column cell into a single query leaf", () => {
    // Hand-build the legacy shape (editor leaf + result leaf in a column).
    const legacyEditorView = {
      id: "ev",
      kind: "editor" as unknown as "query",
      tabIds: ["T1"],
      tabId: "T1",
    };
    const legacyResultView = {
      id: "rv",
      kind: "result" as unknown as "query",
      resultTab: "table" as const,
    };
    const legacyTree = {
      type: "group" as const,
      id: "g",
      direction: "column" as const,
      sizes: [60, 40],
      children: [
        { type: "leaf" as const, id: "el", views: [legacyEditorView], activeViewId: "ev" },
        { type: "leaf" as const, id: "rl", views: [legacyResultView], activeViewId: "rv" },
      ],
    };
    const out = migrateLayout({ workspace: legacyTree });
    // Should now be a single leaf with one query view.
    expect(out.workspace.type).toBe("leaf");
    const leaf = out.workspace as PanelLeaf;
    expect(leaf.views).toHaveLength(1);
    const v = leaf.views[0]!;
    expect(v.kind).toBe("query");
    expect(v.tabIds).toEqual(["T1"]);
    expect(v.tabId).toBe("T1");
    expect(v.resultTab).toBe("table");
    expect(v.editorSizePct).toBeCloseTo(60, 0);
  });

  it("synthesises a workspace tree from the legacy panelSizes shape", () => {
    const out = migrateLayout({
      activitySection: "queries",
      sidebarOpen: true,
      panelSizes: { editorSplit: 0.62 },
      resultTab: "table",
    });
    expect(out.workspace.type).toBe("leaf");
    const leaf = out.workspace as PanelLeaf;
    expect(leaf.views[0]!.resultTab).toBe("table");
    expect(leaf.views[0]!.editorSizePct).toBeCloseTo(62, 0);
  });
});

// ────────────────────────────────────────────────────────────────
// Slice-level coverage — splitPane / closePane / activePane bookkeeping.
// ────────────────────────────────────────────────────────────────

function makeLayoutStore() {
  return create<LayoutSlice>()(
    immer((set, get, api) =>
      createLayoutSlice(
        set as Parameters<typeof createLayoutSlice>[0],
        get as Parameters<typeof createLayoutSlice>[1],
        api as Parameters<typeof createLayoutSlice>[2],
      ),
    ),
  );
}

describe("layout slice — pane reducers", () => {
  it("splitPane moves activePaneId to the new leaf", () => {
    const store = makeLayoutStore();
    const initial = store.getState();
    const oldActive = initial.activePaneId;
    const newId = initial.splitPane(oldActive, "row", "after");
    expect(newId).not.toBeNull();
    expect(store.getState().activePaneId).toBe(newId);
    expect(leafIds(store.getState().workspace)).toContain(newId);
  });

  it("closePane refuses to close the last query pane", () => {
    const store = makeLayoutStore();
    const root = store.getState().workspace as PanelLeaf;
    store.getState().closePane(root.id);
    // Workspace must still hold ≥1 query.
    expect(store.getState().workspace.type).toBe("leaf");
  });

  it("closePane salvages tabs that lived only in the closed leaf's strip", () => {
    const store = makeLayoutStore();
    const leafA = makeLeaf([
      makeView({ kind: "query", tabIds: ["T-keep"], tabId: "T-keep" }),
    ]);
    const leafB = makeLeaf([
      makeView({ kind: "query", tabIds: ["T-orphan"], tabId: "T-orphan" }),
    ]);
    const next = makeGroup("row", [leafA, leafB]);
    store.setState({ ...store.getState(), workspace: next, activePaneId: leafB.id });
    store.getState().closePane(leafB.id);
    const survivor = (findLeaf(store.getState().workspace, leafA.id) as PanelLeaf).views[0]!;
    expect(survivor.tabIds).toContain("T-keep");
    expect(survivor.tabIds).toContain("T-orphan");
  });

  it("closePane succeeds when another query pane remains", () => {
    const store = makeLayoutStore();
    const root = store.getState().workspace as PanelLeaf;
    const newLeafId = store.getState().splitPane(root.id, "row", "after");
    expect(flatLeafIds(store.getState().workspace).length).toBe(2);
    store.getState().closePane(newLeafId!);
    expect(flatLeafIds(store.getState().workspace).length).toBe(1);
  });

  it("resetLayout returns to a single-pane default with the existing tabs attached", () => {
    const store = makeLayoutStore();
    const sibling = store.getState() as unknown as { tabs: Array<{ id: string }> };
    sibling.tabs = [{ id: "T-keep-1" }, { id: "T-keep-2" }];
    const root = store.getState().workspace as PanelLeaf;
    store.getState().splitPane(root.id, "row", "after");
    expect(flatLeafIds(store.getState().workspace).length).toBe(2);
    store.getState().resetLayout();
    const after = store.getState().workspace as PanelLeaf;
    expect(after.type).toBe("leaf");
    const view = after.views[0]!;
    expect(view.kind).toBe("query");
    expect(view.tabIds).toEqual(["T-keep-1", "T-keep-2"]);
    expect(view.tabId).toBe("T-keep-1");
  });

  it("setResultMinimizedForView toggles minimize on a single view", () => {
    const store = makeLayoutStore();
    const root = store.getState().workspace as PanelLeaf;
    const viewId = root.views[0]!.id;
    store.getState().setResultMinimizedForView(viewId, true);
    const next = store.getState().workspace as PanelLeaf;
    expect(next.views[0]!.resultMinimized).toBe(true);
  });

  it("closeTab in the tabs slice GCs the workspace strips", () => {
    type Combined = TabsSlice & LayoutSlice;
    const store = create<Combined>()(
      immer((set, get, api) => ({
        ...createTabsSlice(
          set as Parameters<typeof createTabsSlice>[0],
          get as Parameters<typeof createTabsSlice>[1],
          api as Parameters<typeof createTabsSlice>[2],
        ),
        ...createLayoutSlice(
          set as Parameters<typeof createLayoutSlice>[0],
          get as Parameters<typeof createLayoutSlice>[1],
          api as Parameters<typeof createLayoutSlice>[2],
        ),
      })),
    );
    const tabAId = store.getState().openTab({ name: "A", body: "" });
    const tabBId = store.getState().openTab({ name: "B", body: "" });
    const root = store.getState().workspace as PanelLeaf;
    const view = root.views[0]!;
    store.getState().addTabToEditorView(view.id, tabAId);
    store.getState().addTabToEditorView(view.id, tabBId);

    store.getState().closeTab(tabBId);

    const after = store.getState().workspace as PanelLeaf;
    expect(after.views[0]!.tabIds).toEqual([tabAId]);
    expect(after.views[0]!.tabId).toBe(tabAId);
    expect(store.getState().tabs.map((t) => t.id)).not.toContain(tabBId);
  });

  it("toggleRootDirection is a no-op when the root is a leaf", () => {
    const store = makeLayoutStore();
    const before = store.getState().workspace;
    store.getState().toggleRootDirection();
    expect(store.getState().workspace).toBe(before);
  });

  it("toggleRootDirection flips row ↔ column when the root is a group", () => {
    const store = makeLayoutStore();
    const root = store.getState().workspace as PanelLeaf;
    store.getState().splitPane(root.id, "row", "after");
    const before = store.getState().workspace as PanelGroup;
    expect(before.direction).toBe("row");
    store.getState().toggleRootDirection();
    const after = store.getState().workspace as PanelGroup;
    expect(after.direction).toBe("column");
  });
});
