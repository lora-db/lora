"use client";

/**
 * Mount point for the inspector popups. Subscribes to the
 * `inspections` slice and renders one `NodeCard` per entry inside a
 * shared `<Portal>` so the cards float above the workbench layout
 * regardless of which pane they were opened from.
 */

import { Portal } from "@mantine/core";

import { useStore } from "@/lib/state/store";

import { NodeCard } from "./NodeCard";

export function InspectorHost() {
  const inspections = useStore((s) => s.inspections);
  if (inspections.length === 0) return null;
  return (
    <Portal>
      {inspections.map((insp) => (
        <NodeCard key={insp.key} inspection={insp} />
      ))}
    </Portal>
  );
}
