/**
 * Inspect slice — one or more pinned/unpinned `Inspection`s, each
 * rendered by `NodeCard` as a floating, draggable popup.
 *
 * - At most one *unpinned* inspection exists at a time; opening a new
 *   target replaces it (the classic "drawer feel").
 * - Any number of *pinned* inspections can coexist; the user explicitly
 *   pins via the card header.
 * - Pins do not survive a refresh — the slice is never persisted.
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

export interface Inspection {
  /** Stable per-target key — `${kind}:${id}`. Re-inspecting the same target reuses it. */
  key: string;
  target: InspectTarget;
  pinned: boolean;
  /** Viewport-space coordinates of the click that opened this inspection, when known. */
  anchor?: { x: number; y: number };
  /** User-dragged absolute position. When unset, the card derives its position from `anchor`. */
  position?: { x: number; y: number };
  /** User-resized size. When unset, the card uses its default dimensions. */
  size?: { width: number; height: number };
  /** Stack ordering — higher means drawn on top. Bumped on click. */
  z: number;
}

function safeProperties(v: unknown): Record<string, unknown> {
  if (!v || typeof v !== "object") return {};
  try {
    return JSON.parse(JSON.stringify(v)) as Record<string, unknown>;
  } catch {
    // Cyclic or non-JSON-cloneable input — drop properties rather than
    // crash. The card falls back to "no properties".
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

export function inspectionKey(t: InspectTarget): string {
  return `${t.kind}:${String(t.id)}`;
}

export interface InspectSlice {
  inspections: Inspection[];
  setInspect(
    target: InspectTarget | null,
    options?: { anchor?: { x: number; y: number } },
  ): void;
  pinInspection(key: string, pinned?: boolean): void;
  closeInspection(key: string): void;
  closeAllInspections(options?: { pinnedToo?: boolean }): void;
  bringInspectionToFront(key: string): void;
  moveInspection(key: string, position: { x: number; y: number }): void;
  resizeInspection(key: string, size: { width: number; height: number }): void;
  /** Legacy alias — close the top unpinned card (matches old single-target drawer). */
  closeInspect(): void;
}

function nextZ(list: Inspection[]): number {
  if (list.length === 0) return 1;
  let m = 0;
  for (const i of list) if (i.z > m) m = i.z;
  return m + 1;
}

export const createInspectSlice: StateCreator<
  InspectSlice,
  [["zustand/immer", never]],
  [],
  InspectSlice
> = (set) => ({
  inspections: [],

  setInspect(target, options) {
    // Belt-and-suspenders: detach `properties` from any non-plain or
    // cyclic source before it reaches the immer draft. Engine objects
    // from the graph canvas form cycles via `_neighbors`/`_links`, and
    // immer's `finalize` recurses through every property — a cycle here
    // would blow the stack. JSON round-trip throws on cycles (caught
    // and reduced to `{}`) and yields a plain object on success.
    if (target === null) {
      set((state) => {
        state.inspections = state.inspections.filter((i) => i.pinned);
      });
      return;
    }
    const safe = safeInspectTarget(target);
    const key = inspectionKey(safe);
    set((state) => {
      // Re-inspecting an existing target reuses its entry — pinned or
      // not — and bumps it to the front.
      const existing = state.inspections.find((i) => i.key === key);
      if (existing) {
        existing.target = safe as Inspection["target"];
        existing.z = nextZ(state.inspections);
        if (options?.anchor) existing.anchor = options.anchor;
        return;
      }
      // New target replaces the (singleton) unpinned inspection so the
      // common "click around the graph" flow doesn't litter the screen.
      state.inspections = state.inspections.filter((i) => i.pinned);
      state.inspections.push({
        key,
        target: safe as Inspection["target"],
        pinned: false,
        anchor: options?.anchor,
        z: nextZ(state.inspections),
      });
    });
  },

  pinInspection(key, pinned) {
    set((state) => {
      const found = state.inspections.find((i) => i.key === key);
      if (!found) return;
      found.pinned = pinned ?? !found.pinned;
    });
  },

  closeInspection(key) {
    set((state) => {
      state.inspections = state.inspections.filter((i) => i.key !== key);
    });
  },

  closeAllInspections(options) {
    set((state) => {
      if (options?.pinnedToo) {
        state.inspections = [];
      } else {
        state.inspections = state.inspections.filter((i) => i.pinned);
      }
    });
  },

  bringInspectionToFront(key) {
    set((state) => {
      const found = state.inspections.find((i) => i.key === key);
      if (!found) return;
      found.z = nextZ(state.inspections);
    });
  },

  moveInspection(key, position) {
    set((state) => {
      const found = state.inspections.find((i) => i.key === key);
      if (!found) return;
      found.position = position;
    });
  },

  resizeInspection(key, size) {
    set((state) => {
      const found = state.inspections.find((i) => i.key === key);
      if (!found) return;
      found.size = size;
    });
  },

  closeInspect() {
    set((state) => {
      // Close the topmost unpinned inspection, mirroring the old
      // single-target drawer semantics.
      const sorted = [...state.inspections]
        .filter((i) => !i.pinned)
        .sort((a, b) => b.z - a.z);
      const head = sorted[0];
      if (!head) return;
      state.inspections = state.inspections.filter((i) => i.key !== head.key);
    });
  },
});
