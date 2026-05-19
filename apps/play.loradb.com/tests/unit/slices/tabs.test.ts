/**
 * Unit coverage for the tabs slice. We instantiate the slice in isolation
 * (no other slices, no persistence subscription) so each test exercises
 * only the slice's own reducers. Anything that depends on cross-slice
 * coordination (e.g. orphaned result entries) is covered by the e2e suite.
 */

import { describe, expect, it } from "vitest";
import { create } from "zustand";
import { immer } from "zustand/middleware/immer";

import {
  createTabsSlice,
  type EditorTab,
  type SerializedTab,
  type TabsSlice,
} from "@/lib/state/slices/tabs";

function makeStore() {
  return create<TabsSlice>()(
    immer((set, get, api) =>
      createTabsSlice(
        set as Parameters<typeof createTabsSlice>[0],
        get as Parameters<typeof createTabsSlice>[1],
        api as Parameters<typeof createTabsSlice>[2],
      ),
    ),
  );
}

function names(tabs: ReadonlyArray<EditorTab>): string[] {
  return tabs.map((t) => t.name);
}

describe("tabs slice", () => {
  it("opens a tab with an auto-incremented untitled name", () => {
    const store = makeStore();
    const a = store.getState().openTab();
    const b = store.getState().openTab();
    expect(names(store.getState().tabs)).toEqual(["Query 1", "Query 2"]);
    expect(a).not.toBe(b);
  });

  it("setBody marks the tab dirty only when the body actually changes", () => {
    const store = makeStore();
    const id = store.getState().openTab({ body: "RETURN 1" });
    expect(store.getState().tabs[0]?.dirty).toBe(false);

    // Identical body: should remain clean.
    store.getState().setBody(id, "RETURN 1");
    expect(store.getState().tabs[0]?.dirty).toBe(false);

    // Real change: flips dirty.
    store.getState().setBody(id, "RETURN 2");
    expect(store.getState().tabs[0]?.dirty).toBe(true);
    expect(store.getState().tabs[0]?.body).toBe("RETURN 2");
  });

  it("markClean clears the dirty flag", () => {
    const store = makeStore();
    const id = store.getState().openTab({ body: "RETURN 1" });
    store.getState().setBody(id, "RETURN 2");
    store.getState().markClean(id);
    expect(store.getState().tabs[0]?.dirty).toBe(false);
  });

  it("closeTab removes the tab from the slice", () => {
    const store = makeStore();
    const a = store.getState().openTab({ name: "A" });
    const b = store.getState().openTab({ name: "B" });
    const c = store.getState().openTab({ name: "C" });
    store.getState().closeTab(b);
    expect(names(store.getState().tabs)).toEqual(["A", "C"]);
    expect(a).toBeTruthy();
    expect(c).toBeTruthy();
  });

  it("closing the only tab leaves an empty tabs array", () => {
    const store = makeStore();
    const id = store.getState().openTab();
    store.getState().closeTab(id);
    expect(store.getState().tabs).toHaveLength(0);
  });

  it("reorderTab moves a tab from one index to another", () => {
    const store = makeStore();
    store.getState().openTab({ name: "A" });
    store.getState().openTab({ name: "B" });
    store.getState().openTab({ name: "C" });

    store.getState().reorderTab(0, 2);
    expect(names(store.getState().tabs)).toEqual(["B", "C", "A"]);

    store.getState().reorderTab(2, 0);
    expect(names(store.getState().tabs)).toEqual(["A", "B", "C"]);
  });

  it("reorderTab is a no-op for invalid indices", () => {
    const store = makeStore();
    store.getState().openTab({ name: "A" });
    store.getState().openTab({ name: "B" });

    store.getState().reorderTab(0, 0); // same slot
    store.getState().reorderTab(-1, 1); // negative source
    store.getState().reorderTab(0, 99); // out-of-range target
    store.getState().reorderTab(5, 0); // out-of-range source
    expect(names(store.getState().tabs)).toEqual(["A", "B"]);
  });

  it("hydrateTabs restores order", () => {
    const store = makeStore();
    const records: SerializedTab[] = [
      { id: "x1", name: "X", body: "x", createdAt: 1 },
      { id: "x2", name: "Y", body: "y", createdAt: 2 },
    ];
    store.getState().hydrateTabs(records);
    expect(names(store.getState().tabs)).toEqual(["X", "Y"]);
  });

  it("openTab defaults params to '{}' and openTab honours the override", () => {
    const store = makeStore();
    const a = store.getState().openTab();
    const b = store.getState().openTab({ params: `{ "x": 1 }` });
    const tabA = store.getState().tabs.find((t) => t.id === a);
    const tabB = store.getState().tabs.find((t) => t.id === b);
    expect(tabA?.params).toBe("{}");
    expect(tabB?.params).toBe(`{ "x": 1 }`);
  });

  it("setParams updates the payload and flips dirty only on real changes", () => {
    const store = makeStore();
    const id = store.getState().openTab();
    expect(store.getState().tabs[0]?.dirty).toBe(false);

    // Identical payload: stays clean.
    store.getState().setParams(id, "{}");
    expect(store.getState().tabs[0]?.dirty).toBe(false);

    // Real change.
    store.getState().setParams(id, `{ "userId": "alice" }`);
    const tab = store.getState().tabs.find((t) => t.id === id);
    expect(tab?.params).toBe(`{ "userId": "alice" }`);
    expect(tab?.dirty).toBe(true);
  });

  it("hydrateTabs back-fills missing params field with '{}'", () => {
    const store = makeStore();
    // Legacy record without `params` — exercise the migration path.
    const records: SerializedTab[] = [
      { id: "x1", name: "Legacy", body: "RETURN 1", createdAt: 1 },
    ];
    store.getState().hydrateTabs(records);
    expect(store.getState().tabs[0]?.params).toBe("{}");
  });

  it("hydrateTabs preserves an explicit params payload", () => {
    const store = makeStore();
    const records: SerializedTab[] = [
      {
        id: "x1",
        name: "With params",
        body: "RETURN $x",
        params: `{ "x": 1 }`,
        createdAt: 1,
      },
    ];
    store.getState().hydrateTabs(records);
    expect(store.getState().tabs[0]?.params).toBe(`{ "x": 1 }`);
  });

  it("renameTab updates the name in place", () => {
    const store = makeStore();
    const id = store.getState().openTab({ name: "First" });
    store.getState().renameTab(id, "Renamed");
    expect(store.getState().tabs[0]?.name).toBe("Renamed");
  });

  it("bindSavedQueryId attaches a saved-query reference without flipping dirty", () => {
    const store = makeStore();
    const id = store.getState().openTab({ name: "Q" });
    store.getState().bindSavedQueryId(id, "saved-1");
    expect(store.getState().tabs[0]?.savedQueryId).toBe("saved-1");
    expect(store.getState().tabs[0]?.dirty).toBe(false);
  });
});
