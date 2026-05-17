/**
 * Inspect slice — the currently-inspected node or relationship.
 *
 * Drives the `InspectorDrawer` overlay. A single target at a time
 * (drawer is modal-ish) so the slice carries a `null` or a
 * discriminated-union value rather than an array.
 *
 * Cleared on tab/result switches lives in the consumer for now — the
 * drawer simply re-renders against whichever target is active.
 */

import type { StateCreator } from "zustand";

export type InspectTarget =
  | {
      kind: "node";
      id: string | number;
      labels: string[];
      properties: Record<string, unknown>;
    }
  | {
      kind: "relationship";
      id: string | number;
      type: string;
      startId: string | number;
      endId: string | number;
      properties: Record<string, unknown>;
    };

export interface InspectSlice {
  inspect: InspectTarget | null;
  setInspect(target: InspectTarget | null): void;
  closeInspect(): void;
}

export const createInspectSlice: StateCreator<
  InspectSlice,
  [["zustand/immer", never]],
  [],
  InspectSlice
> = (set) => ({
  inspect: null,

  setInspect(target) {
    set((state) => {
      // Immer freezes the draft assignment — cast through the
      // slice's own type so the discriminated union survives without
      // tripping `WritableDraft<…>` deep mutation rules.
      state.inspect = target as InspectSlice["inspect"];
    });
  },

  closeInspect() {
    set((state) => {
      state.inspect = null;
    });
  },
});
