/**
 * Schema slice — cached labels/relTypes/property-keys and per-label counts
 * for the currently loaded database. Recomputed on demand by the schema
 * sidebar; not persisted (cheap to rebuild from the live DB).
 */

import type { StateCreator } from "zustand";

export interface SchemaSnapshot {
  labels: string[];
  relTypes: string[];
  propertyKeys: string[];
  countsByLabel: Record<string, number>;
  /**
   * Property keys observed on nodes of each label. Empty array when
   * the introspection probe failed for that label (e.g. a label with
   * special characters that couldn't be safely interpolated).
   */
  propertiesByLabel: Record<string, string[]>;
  /**
   * Property keys observed on relationships of each rel-type. Same
   * fallback semantics as {@link propertiesByLabel}.
   */
  propertiesByRelType: Record<string, string[]>;
  fetchedAt: number;
}

export interface SchemaSlice {
  schema: SchemaSnapshot | null;
  refreshing: boolean;
  setSchema(snap: SchemaSnapshot | null): void;
  setRefreshing(v: boolean): void;
}

export const createSchemaSlice: StateCreator<
  SchemaSlice,
  [["zustand/immer", never]],
  [],
  SchemaSlice
> = (set) => ({
  schema: null,
  refreshing: false,

  setSchema(snap) {
    set((state) => {
      state.schema = snap;
    });
  },

  setRefreshing(v) {
    set((state) => {
      state.refreshing = v;
    });
  },
});
