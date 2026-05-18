/**
 * Unit coverage for the workspace tree mutators + the layout slice's
 * pane reducers. The pure tree helpers are exercised directly; the
 * slice glue is checked by spinning up an isolated layout slice and
 * walking it through the same scenarios end-to-end.
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

function leaf(viewKind: "editor" | "result", id?: string): PanelLeaf {
  const v = makeView({ kind: viewKind });
  return makeLeaf([v], id !== undefined ? { id } : undefined);
}

function leafIds(node: PanelNode): string[] {
  return flatLeafIds(node);
}

describe("workspace tree — splitLeaf", () => {
  it("wraps a lone leaf in a row group on split-right", () => {
    const a = leaf("editor", "a");
    const out = splitLeaf(a, "a", "row", "after")!;
    expect(out.tree.type).toBe("group");
    const group = out.tree as PanelGroup;
    expect(group.direction).toBe("row");
    expect(group.children).toHaveLength(2);
    expect(group.children[0]!.id).toBe("a");
    expect(out.newLeafId).toBe(group.children[1]!.id);
  });

  it("splices a new sibling into an existing same-direction group instead of nesting", () => {
    const a = leaf("editor", "a");
    const b = leaf("result", "b");
    const group = makeGroup("row", [a, b], { sizes: [50, 50] });
    const out = splitLeaf(group, "a", "row", "after")!;
    expect(out.tree.type).toBe("group");
    const next = out.tree as PanelGroup;
    expect(next.children).toHaveLength(3);
    expect(next.children.map((c) => c.id)).toEqual(["a", out.newLeafId, "b"]);
    // The source pane's slice should have been halved to make room.
    expect(next.sizes[0]).toBeCloseTo(25, 0);
    expect(next.sizes[1]).toBeCloseTo(25, 0);
    expect(next.sizes[2]).toBeCloseTo(50, 0);
  });

  it("nests a new group when splitting in the perpendicular direction", () => {
    const a = leaf("editor", "a");
    const b = leaf("result", "b");
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
    const a = leaf("editor", "a");
    const b = leaf("result", "b");
    const group = makeGroup("row", [a, b]);
    const next = closeLeaf(group, "b");
    expect(next).not.toBeNull();
    expect(next!.type).toBe("leaf");
    expect((next as PanelLeaf).id).toBe("a");
  });

  it("preserves the group when ≥2 children remain", () => {
    const a = leaf("editor", "a");
    const b = leaf("result", "b");
    const c = leaf("result", "c");
    const group = makeGroup("row", [a, b, c]);
    const next = closeLeaf(group, "b");
    expect(next!.type).toBe("group");
    expect(leafIds(next!)).toEqual(["a", "c"]);
  });

  it("returns null when removing the only remaining leaf", () => {
    const a = leaf("editor", "a");
    const next = closeLeaf(a, "a");
    expect(next).toBeNull();
  });
});

describe("workspace tree — setGroupSizes / setGroupDirection", () => {
  it("setGroupSizes normalises the array and rewrites the right group", () => {
    const a = leaf("editor", "a");
    const b = leaf("result", "b");
    const group = makeGroup("row", [a, b], { sizes: [50, 50] });
    const next = setGroupSizes(group, group.id, [70, 30]) as PanelGroup;
    expect(next.sizes[0]).toBeCloseTo(70, 0);
    expect(next.sizes[1]).toBeCloseTo(30, 0);
  });

  it("setGroupDirection flips the group's direction", () => {
    const a = leaf("editor", "a");
    const b = leaf("result", "b");
    const group = makeGroup("row", [a, b]);
    const next = setGroupDirection(group, group.id, "column") as PanelGroup;
    expect(next.direction).toBe("column");
    expect(next.children).toHaveLength(2);
  });
});

describe("workspace tree — views", () => {
  it("setViewResultTab updates only the targeted result view", () => {
    const v1 = makeView({ kind: "result", resultTab: "graph" });
    const v2 = makeView({ kind: "result", resultTab: "graph" });
    const l = makeLeaf([v1, v2]);
    const next = setViewResultTab(l, v2.id, "json") as PanelLeaf;
    expect(next.views[0]!.resultTab).toBe("graph");
    expect(next.views[1]!.resultTab).toBe("json");
  });

  it("setViewTabId pins/unpins a view", () => {
    const v = makeView({ kind: "editor" });
    const l = makeLeaf([v]);
    const pinned = setViewTabId(l, v.id, "tab-42") as PanelLeaf;
    expect(pinned.views[0]!.tabId).toBe("tab-42");
    const unpinned = setViewTabId(pinned, v.id, undefined) as PanelLeaf;
    expect(unpinned.views[0]!.tabId).toBeUndefined();
  });

  it("insertView / removeView keep activeViewId consistent", () => {
    const v1 = makeView({ kind: "editor" });
    const l = makeLeaf([v1]);
    const v2 = makeView({ kind: "result" });
    const withTwo = insertView(l, l.id, v2) as PanelLeaf;
    expect(withTwo.views).toHaveLength(2);
    expect(withTwo.activeViewId).toBe(v2.id); // insertion activates the new view
    const back = removeView(withTwo, v2.id) as PanelLeaf;
    expect(back.views).toHaveLength(1);
    expect(back.activeViewId).toBe(v1.id);
  });
});

describe("workspace tree — moveView", () => {
  it("moves a view from one leaf to another", () => {
    const v1 = makeView({ kind: "editor" });
    const v2 = makeView({ kind: "result" });
    const l1 = makeLeaf([v1]);
    const l2 = makeLeaf([v2]);
    const group = makeGroup("row", [l1, l2]);
    const next = moveView(group, v1.id, l2.id) as PanelGroup;
    // l1 collapsed because empty; root is now just l2 (or the migrated leaf).
    // Either way v1 must end up in the surviving leaf.
    const leafWithV1 = findViewLeaf(next, v1.id);
    expect(leafWithV1).not.toBeNull();
    expect(leafWithV1!.views.some((v) => v.id === v1.id)).toBe(true);
  });
});

describe("workspace tree — gcClosedTab", () => {
  it("drops the closed tab id from editor view strips and removes empty editor views", () => {
    const v1 = makeView({ kind: "editor", tabIds: ["T1"], tabId: "T1" });
    const v2 = makeView({ kind: "editor", tabIds: ["T2"], tabId: "T2" });
    const l = makeLeaf([v1, v2]);
    const next = gcClosedTab(l, "T1") as PanelLeaf;
    expect(next.views).toHaveLength(1);
    expect(next.views[0]!.id).toBe(v2.id);
  });

  it("keeps an editor view when other tabs remain in its strip", () => {
    const v = makeView({ kind: "editor", tabIds: ["T1", "T2"], tabId: "T1" });
    const l = makeLeaf([v]);
    const next = gcClosedTab(l, "T1") as PanelLeaf;
    expect(next.views).toHaveLength(1);
    const survivor = next.views[0]! as PanelView;
    expect(survivor.tabIds).toEqual(["T2"]);
    expect(survivor.tabId).toBe("T2");
  });

  it("unpins a pinned result view rather than removing it (non-cell layout)", () => {
    const v = makeView({ kind: "result", tabId: "T-stale", resultTab: "table" });
    const l = makeLeaf([v]);
    const next = gcClosedTab(l, "T-stale") as PanelLeaf;
    expect(next.views).toHaveLength(1);
    expect(next.views[0]!.tabId).toBeUndefined();
    expect(next.views[0]!.resultTab).toBe("table");
  });

  it("repoints a cell's result view to the companion editor's surviving tab", () => {
    // Build a canonical cell where the editor has [T1, T2], active T1,
    // and the result is pinned to T1.
    const editorView = makeView({ kind: "editor", tabIds: ["T1", "T2"], tabId: "T1" });
    const resultView = makeView({ kind: "result", tabId: "T1", resultTab: "graph" });
    const cell = makeGroup("column", [
      makeLeaf([editorView]),
      makeLeaf([resultView]),
    ]);
    // T1 closes: editor strip drops T1 (so active becomes T2);
    // the result view should follow to T2, not unpin.
    const next = gcClosedTab(cell, "T1") as PanelGroup;
    // Find the result view inside the new tree.
    let foundResultTabId: string | undefined;
    for (const child of next.children) {
      if (child.type !== "leaf") continue;
      const v = child.views[0]!;
      if (v.kind === "result") foundResultTabId = v.tabId;
    }
    expect(foundResultTabId).toBe("T2");
  });
});

describe("validateWorkspace", () => {
  it("accepts a well-formed default tree", () => {
    const { workspace, editorViewId } = buildDefaultWorkspace({
      editorTabIds: ["T1"],
      editorActiveTabId: "T1",
    });
    void editorViewId;
    expect(
      validateWorkspace(workspace, { tabIds: new Set(["T1"]) }),
    ).toBeNull();
  });

  it("rejects a tree with no editor view", () => {
    const tree = makeLeaf([makeView({ kind: "result" })]);
    expect(validateWorkspace(tree, { tabIds: new Set() })).toMatch(/no editor view/);
  });

  it("rejects a tree with no result view", () => {
    const tree = makeLeaf([makeView({ kind: "editor", tabIds: ["T1"], tabId: "T1" })]);
    expect(validateWorkspace(tree, { tabIds: new Set(["T1"]) })).toMatch(/no result view/);
  });

  it("rejects an editor view referencing a missing tab", () => {
    const tree = makeGroup("column", [
      makeLeaf([makeView({ kind: "editor", tabIds: ["MISSING"], tabId: "MISSING" })]),
      makeLeaf([makeView({ kind: "result" })]),
    ]);
    expect(validateWorkspace(tree, { tabIds: new Set(["OTHER"]) })).toMatch(/missing tab/);
  });

  it("rejects a group with only one child", () => {
    const tree = {
      type: "group",
      id: "g1",
      direction: "row",
      sizes: [100],
      children: [makeLeaf([makeView({ kind: "editor" })])],
    };
    expect(validateWorkspace(tree)).toMatch(/≥2 children/);
  });
});

describe("healOrphanedTabs", () => {
  it("seeds an empty editor strip from a legacy single tabId", () => {
    const editor = makeView({ kind: "editor", tabId: "t1" });
    // Strip defaults to [] from makeView since we didn't pass tabIds.
    expect(editor.tabIds).toEqual([]);
    const root = makeGroup("column", [
      makeLeaf([editor]),
      makeLeaf([makeView({ kind: "result" })]),
    ]);
    const healed = healOrphanedTabs(root, ["t1"]);
    const leaf = (healed as PanelGroup).children[0] as PanelLeaf;
    expect(leaf.views[0]!.tabIds).toEqual(["t1"]);
    expect(leaf.views[0]!.tabId).toBe("t1");
  });

  it("appends tabs that aren't open in any editor view to the first one", () => {
    const editor = makeView({ kind: "editor", tabIds: ["a"], tabId: "a" });
    const root = makeGroup("column", [
      makeLeaf([editor]),
      makeLeaf([makeView({ kind: "result" })]),
    ]);
    const healed = healOrphanedTabs(root, ["a", "b", "c"]);
    const leaf = (healed as PanelGroup).children[0] as PanelLeaf;
    expect(leaf.views[0]!.tabIds).toEqual(["a", "b", "c"]);
  });

  it("activates the first strip entry when no view tabId is set", () => {
    // Use makeView so tabIds is initialised, then strip the tabId we
    // added so heal sees a "strip without an active selection" leaf.
    const editor = makeView({ kind: "editor", tabIds: ["a"], tabId: "a" });
    delete editor.tabId;
    const root = makeGroup("column", [
      makeLeaf([editor]),
      makeLeaf([makeView({ kind: "result" })]),
    ]);
    const healed = healOrphanedTabs(root, ["a"]);
    const leaf = (healed as PanelGroup).children[0] as PanelLeaf;
    expect(leaf.views[0]!.tabId).toBe("a");
  });

  it("returns the same reference when nothing needs healing", () => {
    const editor = makeView({ kind: "editor", tabIds: ["a"], tabId: "a" });
    const root = makeGroup("column", [
      makeLeaf([editor]),
      makeLeaf([makeView({ kind: "result" })]),
    ]);
    expect(healOrphanedTabs(root, ["a"])).toBe(root);
  });
});

describe("migrateLayout", () => {
  it("accepts the new shape verbatim (fills missing chrome fields)", () => {
    const { workspace } = buildDefaultWorkspace();
    const out = migrateLayout({
      activitySection: "schema",
      sidebarOpen: false,
      workspace,
      activePaneId: "whatever",
    });
    expect(out.activitySection).toBe("schema");
    expect(out.sidebarOpen).toBe(false);
    expect(out.workspace).toBe(workspace);
  });

  it("preserves a leaf's non-default activeViewId across the new-shape path", () => {
    const editor = makeView({ kind: "editor" });
    const resultA = makeView({ kind: "result", resultTab: "graph" });
    const resultB = makeView({ kind: "result", resultTab: "table" });
    // Build a leaf with three views, activeViewId pointing at the
    // middle one (not the default first).
    const multi = makeLeaf([editor, resultA, resultB], { activeViewId: resultA.id });
    const root = makeGroup("column", [multi, makeLeaf([makeView({ kind: "result" })])]);
    const out = migrateLayout({ workspace: root, activePaneId: multi.id });
    const restored = out.workspace as PanelGroup;
    const firstLeaf = restored.children[0] as PanelLeaf;
    expect(firstLeaf.activeViewId).toBe(resultA.id);
  });

  it("synthesises a workspace tree from the legacy panelSizes shape", () => {
    const out = migrateLayout({
      activitySection: "queries",
      sidebarOpen: true,
      panelSizes: { editorSplit: 0.62 },
      resultTab: "table",
    });
    expect(out.workspace.type).toBe("group");
    const root = out.workspace as PanelGroup;
    expect(root.direction).toBe("column");
    expect(root.sizes[0]).toBeCloseTo(62, 0);
    expect(root.sizes[1]).toBeCloseTo(38, 0);
    // Result leaf inherits the legacy global resultTab.
    const resultLeaf = (root.children[1] as PanelLeaf);
    expect(resultLeaf.views[0]!.resultTab).toBe("table");
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

  it("closePane refuses to close the last editor or result pane", () => {
    const store = makeLayoutStore();
    // Default workspace has one editor leaf + one result leaf.
    const root = store.getState().workspace as PanelGroup;
    const editorId = root.children[0]!.id;
    const resultId = root.children[1]!.id;
    // Closing the only editor leaf must be refused.
    store.getState().closePane(editorId);
    expect(store.getState().workspace.type).toBe("group");
    // Closing the only result leaf must also be refused.
    store.getState().closePane(resultId);
    expect(store.getState().workspace.type).toBe("group");
  });

  it("closePane salvages tabs that lived only in the closed leaf's strip", () => {
    const store = makeLayoutStore();
    const root = store.getState().workspace as PanelGroup;
    // Two editor leaves at the row level — quickest setup that
    // exercises the salvage path.
    const editorLeafA = makeLeaf([makeView({ kind: "editor", tabIds: ["T-keep"], tabId: "T-keep" })]);
    const editorLeafB = makeLeaf([makeView({ kind: "editor", tabIds: ["T-orphan"], tabId: "T-orphan" })]);
    const resultLeaf = makeLeaf([makeView({ kind: "result", resultTab: "graph" })]);
    const next = makeGroup("row", [editorLeafA, editorLeafB, resultLeaf]);
    void root;
    store.setState({ ...store.getState(), workspace: next, activePaneId: editorLeafB.id });
    expect(store.getState().workspace.type).toBe("group");
    store.getState().closePane(editorLeafB.id);
    // After closing leaf B, the survivor editor (leafA) must hold T-orphan too.
    const survivorView = (findLeaf(store.getState().workspace, editorLeafA.id) as PanelLeaf)!.views[0]!;
    expect(survivorView.kind).toBe("editor");
    expect(survivorView.tabIds).toContain("T-keep");
    expect(survivorView.tabIds).toContain("T-orphan");
  });

  it("closePane succeeds when another view of the same kind remains", () => {
    const store = makeLayoutStore();
    const root = store.getState().workspace as PanelGroup;
    const editorId = root.children[0]!.id;
    // Split the editor pane so we have two editor leaves; closing one is allowed.
    store.getState().splitPane(editorId, "row", "after");
    const beforeIds = flatLeafIds(store.getState().workspace);
    expect(beforeIds.length).toBe(3);
    store.getState().closePane(editorId);
    const afterIds = flatLeafIds(store.getState().workspace);
    expect(afterIds.length).toBe(2);
    expect(afterIds).not.toContain(editorId);
  });

  it("resetLayout returns to a default split and reattaches existing tabs", () => {
    const store = makeLayoutStore();
    // Pretend we have tabs in a sibling slice — the layout slice walks
    // them via the immer-merged draft.
    const sibling = store.getState() as unknown as { tabs: Array<{ id: string }> };
    sibling.tabs = [{ id: "T-keep-1" }, { id: "T-keep-2" }];
    // Split twice so the workspace is non-trivial.
    const root = store.getState().workspace as PanelGroup;
    store.getState().splitPane(root.children[0]!.id, "row", "after");
    expect(flatLeafIds(store.getState().workspace).length).toBe(3);
    // Reset.
    store.getState().resetLayout();
    const after = store.getState().workspace as PanelGroup;
    expect(after.type).toBe("group");
    expect(after.children).toHaveLength(2);
    expect(after.direction).toBe("column");
    const editorLeaf = after.children[0]! as PanelLeaf;
    const editorView = editorLeaf.views[0]! as PanelView;
    expect(editorView.kind).toBe("editor");
    expect(editorView.tabIds).toEqual(["T-keep-1", "T-keep-2"]);
    expect(editorView.tabId).toBe("T-keep-1");
  });

  it("gcClosedTab unpins result views that pointed at the closed tab", () => {
    const store = makeLayoutStore();
    const root = store.getState().workspace as PanelGroup;
    const editorLeafId = root.children[0]!.id;
    // Insert a pinned result view inside the editor leaf.
    store.getState().addView(editorLeafId, "result", { tabId: "T-stale" });
    const beforeViews = (findLeaf(store.getState().workspace, editorLeafId) as PanelLeaf).views;
    expect(beforeViews).toHaveLength(2);
    store.getState().gcClosedTab("T-stale");
    const afterViews = (findLeaf(store.getState().workspace, editorLeafId) as PanelLeaf).views;
    expect(afterViews).toHaveLength(2);
    const resultView = afterViews.find((v) => v.kind === "result")!;
    expect(resultView.tabId).toBeUndefined();
  });

  it("closeTab in the tabs slice GCs editor strips on the workspace", () => {
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
    const root = store.getState().workspace as PanelGroup;
    const editorLeafId = root.children[0]!.id;
    const editorView = (root.children[0] as PanelLeaf).views[0]!;
    store.getState().addTabToEditorView(editorView.id, tabAId);
    store.getState().addTabToEditorView(editorView.id, tabBId);
    // Pin the cell's result view to tabB so we can verify it repoints.
    const resultView = (root.children[1] as PanelLeaf).views[0]!;
    store.getState().setViewTabId(resultView.id, tabBId);
    expect((store.getState().workspace as PanelGroup).children).toHaveLength(2);
    void editorLeafId;

    // Closing tabB through the tabs slice must clean it out of the
    // editor strip AND repoint the cell-bound result view to tabA.
    store.getState().closeTab(tabBId);

    const after = store.getState().workspace as PanelGroup;
    const editorAfter = (after.children[0] as PanelLeaf).views[0]!;
    expect(editorAfter.tabIds).toEqual([tabAId]);
    const resultAfter = (after.children[1] as PanelLeaf).views[0]!;
    expect(resultAfter.tabId).toBe(tabAId);
    expect(store.getState().tabs.map((t) => t.id)).not.toContain(tabBId);
  });

  it("toggleRootDirection flips row ↔ column", () => {
    const store = makeLayoutStore();
    const before = store.getState().workspace as PanelGroup;
    expect(before.direction).toBe("column");
    store.getState().toggleRootDirection();
    expect((store.getState().workspace as PanelGroup).direction).toBe("row");
    store.getState().toggleRootDirection();
    expect((store.getState().workspace as PanelGroup).direction).toBe("column");
  });
});
