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

function safeProperties(v: unknown): Record<string, unknown> {
  if (!v || typeof v !== "object") return {};
  try {
    return JSON.parse(JSON.stringify(v)) as Record<string, unknown>;
  } catch {
    // Cyclic or non-JSON-cloneable input — drop properties rather than
    // crash. The drawer falls back to "no properties".
    return {};
  }
}

function safeInspectTarget(t: InspectTarget): InspectTarget {
  if (t.kind === "node") {
    return {
      kind: "node",
      id: t.id,
      labels: Array.isArray(t.labels) ? t.labels.slice() : [],
      properties: safeProperties(t.properties),
    };
  }
  return {
    kind: "relationship",
    id: t.id,
    type: t.type,
    startId: t.startId,
    endId: t.endId,
    properties: safeProperties(t.properties),
  };
}

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
    // Belt-and-suspenders: detach `properties` from any non-plain or
    // cyclic source before it reaches the immer draft. Engine objects
    // from the graph canvas form cycles via `_neighbors`/`_links`, and
    // immer's `finalize` recurses through every property — a cycle here
    // would blow the stack. JSON round-trip throws on cycles (caught
    // and reduced to `{}`) and yields a plain object on success.
    const safeTarget = target ? safeInspectTarget(target) : null;
    set((state) => {
      // Immer freezes the draft assignment — cast through the
      // slice's own type so the discriminated union survives without
      // tripping `WritableDraft<…>` deep mutation rules.
      state.inspect = safeTarget as InspectSlice["inspect"];
    });
  },

  closeInspect() {
    set((state) => {
      state.inspect = null;
    });
  },
});
