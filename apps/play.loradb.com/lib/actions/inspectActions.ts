"use client";

/**
 * Imperative inspect actions — invoked from the graph canvas / table
 * cell clicks. Each action pushes a discriminated-union value into the
 * `inspect` slice; the `NodeCard` popup subscribes via the
 * `inspections` selector and renders accordingly.
 */

import { useStore } from "@/lib/state/store";
import type { InspectTarget } from "@/lib/state/slices/inspect";

export interface InspectAnchor {
  /** Viewport-space x of the click that opened the inspection. */
  x: number;
  /** Viewport-space y of the click that opened the inspection. */
  y: number;
}

export function inspectNode(
  node: {
    id: string | number;
    labels?: string[];
    properties?: Record<string, unknown>;
  },
  options?: { anchor?: InspectAnchor },
): void {
  const target: InspectTarget = {
    kind: "node",
    id: node.id,
    labels: node.labels ?? [],
    properties: node.properties ?? {},
  };
  useStore.getState().setInspect(target, options);
}

export function inspectRelationship(
  rel: {
    id: string | number;
    type: string;
    startId: string | number;
    endId: string | number;
    properties?: Record<string, unknown>;
  },
  options?: { anchor?: InspectAnchor },
): void {
  const target: InspectTarget = {
    kind: "relationship",
    id: rel.id,
    type: rel.type,
    startId: rel.startId,
    endId: rel.endId,
    properties: rel.properties ?? {},
  };
  useStore.getState().setInspect(target, options);
}

export function closeInspect(): void {
  useStore.getState().closeInspect();
}

export function closeAllInspections(): void {
  useStore.getState().closeAllInspections({ pinnedToo: true });
}
