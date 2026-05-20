/**
 * Schema design slice — cached index and constraint catalog plus the
 * UI state that drives the wizards (which wizard is open, dismissed
 * recommendations).
 *
 * The cached catalog is rebuilt by `refreshSchemaDesign` after every
 * mutation (and after a `loradb:mutation` window event, mirroring the
 * schema introspection slice).
 *
 * `dismissedRecs` is persisted via the session record so a "no
 * thanks" on a recommendation card sticks across reloads.
 */

import type { StateCreator } from "zustand";

import type { ConstraintDef, IndexDef } from "@/lib/schemaDesign/types";

export type SchemaWizard = "newIndex" | "newConstraint" | null;

export interface SchemaDesignSlice {
  /** Last `SHOW INDEXES` snapshot, or `null` before first fetch. */
  indexes: IndexDef[] | null;
  /** Last `SHOW CONSTRAINTS` snapshot, or `null` before first fetch. */
  constraints: ConstraintDef[] | null;
  refreshing: boolean;
  lastFetchedAt: number | null;
  /** Which wizard modal (if any) is currently mounted. */
  wizard: SchemaWizard;
  /** Recommendations the user explicitly dismissed (persisted). */
  dismissedRecs: string[];
  /** When set, the New-Index wizard pre-fills from this seed. */
  newIndexSeed: Partial<{
    kind: IndexDef["kind"];
    entity: IndexDef["entity"];
    label: string;
    property: string;
  }> | null;
  /** When set, the New-Constraint wizard pre-fills from this seed. */
  newConstraintSeed: Partial<{
    kind: ConstraintDef["kind"];
    entity: ConstraintDef["entity"];
    label: string;
    property: string;
  }> | null;
  /**
   * When set, the index wizard is in "edit" mode — its draft is
   * fully pre-seeded from this def, the modal title flips to "Edit
   * index", and submit issues DROP + CREATE rather than a plain
   * CREATE. Cleared on close.
   */
  editingIndexDef: IndexDef | null;
  /** Same idea for the constraint wizard. */
  editingConstraintDef: ConstraintDef | null;

  setSchemaDesign(snap: {
    indexes: IndexDef[];
    constraints: ConstraintDef[];
  }): void;
  setSchemaDesignError(): void;
  setSchemaDesignRefreshing(v: boolean): void;
  openNewIndexWizard(seed?: SchemaDesignSlice["newIndexSeed"]): void;
  openNewConstraintWizard(seed?: SchemaDesignSlice["newConstraintSeed"]): void;
  openEditIndexWizard(def: IndexDef): void;
  openEditConstraintWizard(def: ConstraintDef): void;
  closeWizard(): void;
  dismissRecommendation(id: string): void;
  restoreRecommendations(): void;
  hydrateDismissedRecs(ids: string[] | undefined): void;
}

export const createSchemaDesignSlice: StateCreator<
  SchemaDesignSlice,
  [["zustand/immer", never]],
  [],
  SchemaDesignSlice
> = (set) => ({
  indexes: null,
  constraints: null,
  refreshing: false,
  lastFetchedAt: null,
  wizard: null,
  dismissedRecs: [],
  newIndexSeed: null,
  newConstraintSeed: null,
  editingIndexDef: null,
  editingConstraintDef: null,

  setSchemaDesign(snap) {
    set((state) => {
      state.indexes = snap.indexes;
      state.constraints = snap.constraints;
      state.lastFetchedAt = Date.now();
    });
  },

  setSchemaDesignError() {
    set((state) => {
      // Don't clobber a previously-good snapshot on a transient failure
      // — we just stop showing "refreshing" and let the UI surface the
      // error via the notifications channel.
      if (state.indexes === null) state.indexes = [];
      if (state.constraints === null) state.constraints = [];
    });
  },

  setSchemaDesignRefreshing(v) {
    set((state) => {
      state.refreshing = v;
    });
  },

  openNewIndexWizard(seed) {
    set((state) => {
      state.newIndexSeed = seed ?? null;
      state.editingIndexDef = null;
      state.wizard = "newIndex";
    });
  },

  openNewConstraintWizard(seed) {
    set((state) => {
      state.newConstraintSeed = seed ?? null;
      state.editingConstraintDef = null;
      state.wizard = "newConstraint";
    });
  },

  openEditIndexWizard(def) {
    set((state) => {
      state.editingIndexDef = def;
      state.newIndexSeed = null;
      state.wizard = "newIndex";
    });
  },

  openEditConstraintWizard(def) {
    set((state) => {
      state.editingConstraintDef = def;
      state.newConstraintSeed = null;
      state.wizard = "newConstraint";
    });
  },

  closeWizard() {
    set((state) => {
      state.wizard = null;
      state.newIndexSeed = null;
      state.newConstraintSeed = null;
      state.editingIndexDef = null;
      state.editingConstraintDef = null;
    });
  },

  dismissRecommendation(id) {
    set((state) => {
      if (state.dismissedRecs.includes(id)) return;
      state.dismissedRecs.push(id);
    });
  },

  restoreRecommendations() {
    set((state) => {
      state.dismissedRecs = [];
    });
  },

  hydrateDismissedRecs(ids) {
    set((state) => {
      state.dismissedRecs = Array.isArray(ids) ? [...ids] : [];
    });
  },
});
