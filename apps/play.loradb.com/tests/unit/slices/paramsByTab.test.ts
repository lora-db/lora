/**
 * Unit coverage for the paramsByTab bridge slice. The slice diffs
 * before mutating so consumers can subscribe with reference
 * equality — the test fixes that contract.
 */

import { describe, expect, it } from "vitest";
import { create } from "zustand";
import { immer } from "zustand/middleware/immer";

import {
  createParamsByTabSlice,
  type ParamsByTabSlice,
} from "@/lib/state/slices/paramsByTab";

function makeStore() {
  return create<ParamsByTabSlice>()(
    immer((set, get, api) =>
      createParamsByTabSlice(
        set as Parameters<typeof createParamsByTabSlice>[0],
        get as Parameters<typeof createParamsByTabSlice>[1],
        api as Parameters<typeof createParamsByTabSlice>[2],
      ),
    ),
  );
}

describe("paramsByTab slice", () => {
  it("setDetectedParams stores a copy of the input list", () => {
    const store = makeStore();
    const input = ["userId", "minAge"];
    store.getState().setDetectedParams("tab-1", input);
    const stored = store.getState().paramsByTab["tab-1"];
    expect(stored).toEqual(["userId", "minAge"]);
    // Mutating the input must not bleed into the slice.
    input.push("cap");
    expect(store.getState().paramsByTab["tab-1"]).toEqual(["userId", "minAge"]);
  });

  it("setDetectedParams skips the write when the list is identical", () => {
    const store = makeStore();
    store.getState().setDetectedParams("tab-1", ["a", "b"]);
    const beforeMap = store.getState().paramsByTab;
    const beforeList = beforeMap["tab-1"];
    store.getState().setDetectedParams("tab-1", ["a", "b"]);
    const afterMap = store.getState().paramsByTab;
    expect(afterMap["tab-1"]).toBe(beforeList);
    expect(afterMap).toBe(beforeMap);
  });

  it("setDetectedParams replaces the list on order change", () => {
    const store = makeStore();
    store.getState().setDetectedParams("tab-1", ["a", "b"]);
    store.getState().setDetectedParams("tab-1", ["b", "a"]);
    expect(store.getState().paramsByTab["tab-1"]).toEqual(["b", "a"]);
  });

  it("clearDetectedParams removes the entry", () => {
    const store = makeStore();
    store.getState().setDetectedParams("tab-1", ["a"]);
    store.getState().setDetectedParams("tab-2", ["b"]);
    store.getState().clearDetectedParams("tab-1");
    expect("tab-1" in store.getState().paramsByTab).toBe(false);
    expect(store.getState().paramsByTab["tab-2"]).toEqual(["b"]);
  });

  it("multiple tabs are independent", () => {
    const store = makeStore();
    store.getState().setDetectedParams("tab-1", ["a"]);
    store.getState().setDetectedParams("tab-2", ["b", "c"]);
    expect(store.getState().paramsByTab).toEqual({
      "tab-1": ["a"],
      "tab-2": ["b", "c"],
    });
  });
});
