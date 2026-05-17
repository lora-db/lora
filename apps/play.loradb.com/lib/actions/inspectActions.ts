"use client";

/**
 * Imperative inspect actions — invoked from the graph canvas / table
 * cell clicks. Each action pushes a discriminated-union value into the
 * `inspect` slice; the `InspectorDrawer` component subscribes via the
 * `inspect` selector and renders accordingly.
 */

import { useStore } from "@/lib/state/store";
import type { InspectTarget } from "@/lib/state/slices/inspect";

export function inspectNode(node: {
  id: string | number;
  labels?: string[];
  properties?: Record<string, unknown>;
}): void {
  const target: InspectTarget = {
    kind: "node",
    id: node.id,
    labels: node.labels ?? [],
    properties: node.properties ?? {},
  };
  useStore.getState().setInspect(target);
}

export function inspectRelationship(rel: {
  id: string | number;
  type: string;
  startId: string | number;
  endId: string | number;
  properties?: Record<string, unknown>;
}): void {
  const target: InspectTarget = {
    kind: "relationship",
    id: rel.id,
    type: rel.type,
    startId: rel.startId,
    endId: rel.endId,
    properties: rel.properties ?? {},
  };
  useStore.getState().setInspect(target);
}

export function closeInspect(): void {
  useStore.getState().closeInspect();
}
