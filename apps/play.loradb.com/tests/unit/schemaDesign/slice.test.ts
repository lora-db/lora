/**
 * Unit tests for the schemaDesign Zustand slice. We construct a
 * single-slice store with immer middleware to exercise the actions
 * in isolation from the rest of the playground store.
 */

import { describe, expect, it, beforeEach } from "vitest";
import { create, type StoreApi } from "zustand";
import { immer } from "zustand/middleware/immer";

import {
  createSchemaDesignSlice,
  type SchemaDesignSlice,
} from "@/lib/state/slices/schemaDesign";

function makeStore(): StoreApi<SchemaDesignSlice> {
  return create<SchemaDesignSlice>()(
    immer((set, get, api) =>
      createSchemaDesignSlice(
        set as Parameters<typeof createSchemaDesignSlice>[0],
        get as Parameters<typeof createSchemaDesignSlice>[1],
        api as Parameters<typeof createSchemaDesignSlice>[2],
      ),
    ),
  );
}

let store: StoreApi<SchemaDesignSlice>;

beforeEach(() => {
  store = makeStore();
});

describe("initial state", () => {
  it("starts cold with null catalogs and no wizard", () => {
    const s = store.getState();
    expect(s.indexes).toBeNull();
    expect(s.constraints).toBeNull();
    expect(s.wizard).toBeNull();
    expect(s.refreshing).toBe(false);
    expect(s.dismissedRecs).toEqual([]);
  });
});

describe("setSchemaDesign", () => {
  it("populates indexes/constraints and stamps lastFetchedAt", () => {
    const before = Date.now();
    store.getState().setSchemaDesign({ indexes: [], constraints: [] });
    const s = store.getState();
    expect(s.indexes).toEqual([]);
    expect(s.constraints).toEqual([]);
    expect(s.lastFetchedAt).toBeGreaterThanOrEqual(before);
  });
});

describe("setSchemaDesignError", () => {
  it("seeds empty arrays only when the snapshot is still cold", () => {
    store.getState().setSchemaDesignError();
    expect(store.getState().indexes).toEqual([]);
    expect(store.getState().constraints).toEqual([]);
  });

  it("does not clobber a previously-good snapshot", () => {
    const good = [
      {
        name: "i",
        kind: "RANGE" as const,
        entity: "NODE" as const,
        labelsOrTypes: ["Person"],
        properties: ["email"],
        state: "online" as const,
        populationPercent: 100,
        owned: false,
      },
    ];
    store.getState().setSchemaDesign({ indexes: good, constraints: [] });
    store.getState().setSchemaDesignError();
    expect(store.getState().indexes).toBe(good);
  });
});

describe("openNewIndexWizard", () => {
  it("sets wizard='newIndex' and stores the seed", () => {
    store.getState().openNewIndexWizard({
      kind: "RANGE",
      entity: "NODE",
      label: "Person",
      property: "email",
    });
    const s = store.getState();
    expect(s.wizard).toBe("newIndex");
    expect(s.newIndexSeed).toEqual({
      kind: "RANGE",
      entity: "NODE",
      label: "Person",
      property: "email",
    });
  });

  it("clears the seed when called with no args", () => {
    store.getState().openNewIndexWizard({ label: "X" });
    store.getState().openNewIndexWizard();
    expect(store.getState().newIndexSeed).toBeNull();
  });
});

describe("openNewConstraintWizard", () => {
  it("sets wizard='newConstraint' and seeds", () => {
    store.getState().openNewConstraintWizard({
      kind: "UNIQUE",
      entity: "NODE",
      label: "Person",
      property: "email",
    });
    expect(store.getState().wizard).toBe("newConstraint");
    expect(store.getState().newConstraintSeed?.kind).toBe("UNIQUE");
  });
});

describe("closeWizard", () => {
  it("clears both seeds and resets the wizard slot", () => {
    store.getState().openNewIndexWizard({ label: "X" });
    store.getState().openNewConstraintWizard({ label: "Y" });
    store.getState().closeWizard();
    const s = store.getState();
    expect(s.wizard).toBeNull();
    expect(s.newIndexSeed).toBeNull();
    expect(s.newConstraintSeed).toBeNull();
  });

  it("clears the editing-def slots too", () => {
    store.getState().openEditIndexWizard({
      name: "idx",
      kind: "RANGE",
      entity: "NODE",
      labelsOrTypes: ["Person"],
      properties: ["email"],
      state: "online",
      populationPercent: 100,
      owned: false,
    });
    store.getState().closeWizard();
    expect(store.getState().editingIndexDef).toBeNull();
  });
});

describe("openEditIndexWizard", () => {
  it("sets wizard='newIndex' and stashes the def for the wizard to seed from", () => {
    const def = {
      name: "idx_person_email",
      kind: "RANGE" as const,
      entity: "NODE" as const,
      labelsOrTypes: ["Person"],
      properties: ["email"],
      state: "online" as const,
      populationPercent: 100,
      owned: false,
    };
    store.getState().openEditIndexWizard(def);
    const s = store.getState();
    expect(s.wizard).toBe("newIndex");
    expect(s.editingIndexDef).toEqual(def);
    // A subsequent "new" flow drops the editing handle.
    expect(s.newIndexSeed).toBeNull();
  });

  it("openNewIndexWizard clears any pre-existing editing handle", () => {
    store.getState().openEditIndexWizard({
      name: "idx",
      kind: "RANGE",
      entity: "NODE",
      labelsOrTypes: ["Person"],
      properties: ["email"],
      state: "online",
      populationPercent: 100,
      owned: false,
    });
    store.getState().openNewIndexWizard();
    expect(store.getState().editingIndexDef).toBeNull();
  });
});

describe("openEditConstraintWizard", () => {
  it("sets wizard='newConstraint' and stashes the def", () => {
    const def = {
      name: "unique_person_email",
      kind: "UNIQUE" as const,
      entity: "NODE" as const,
      label: "Person",
      properties: ["email"],
    };
    store.getState().openEditConstraintWizard(def);
    const s = store.getState();
    expect(s.wizard).toBe("newConstraint");
    expect(s.editingConstraintDef).toEqual(def);
    expect(s.newConstraintSeed).toBeNull();
  });
});

describe("dismissRecommendation", () => {
  it("appends a new id", () => {
    store.getState().dismissRecommendation("a");
    expect(store.getState().dismissedRecs).toEqual(["a"]);
  });

  it("is idempotent", () => {
    store.getState().dismissRecommendation("a");
    store.getState().dismissRecommendation("a");
    expect(store.getState().dismissedRecs).toEqual(["a"]);
  });

  it("restoreRecommendations clears all", () => {
    store.getState().dismissRecommendation("a");
    store.getState().dismissRecommendation("b");
    store.getState().restoreRecommendations();
    expect(store.getState().dismissedRecs).toEqual([]);
  });

  it("hydrateDismissedRecs replaces with the provided list", () => {
    store.getState().hydrateDismissedRecs(["x", "y"]);
    expect(store.getState().dismissedRecs).toEqual(["x", "y"]);
  });

  it("hydrateDismissedRecs(undefined) resets to empty", () => {
    store.getState().dismissRecommendation("a");
    store.getState().hydrateDismissedRecs(undefined);
    expect(store.getState().dismissedRecs).toEqual([]);
  });
});

describe("setSchemaDesignRefreshing", () => {
  it("toggles the flag", () => {
    store.getState().setSchemaDesignRefreshing(true);
    expect(store.getState().refreshing).toBe(true);
    store.getState().setSchemaDesignRefreshing(false);
    expect(store.getState().refreshing).toBe(false);
  });
});
