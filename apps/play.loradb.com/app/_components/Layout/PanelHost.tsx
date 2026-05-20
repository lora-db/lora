"use client";

/**
 * Renders the workspace tree using `react-resizable-panels` (v4 API).
 *
 * `Group` (recursive) → `PanelLeafFrame` (leaf). Group sizes are
 * commit-flushed back into the store on each `onLayoutChanged` callback
 * (which fires only on pointer release) so the IDB session record
 * captures resizes without going through React on every pointer move.
 */

import { Fragment, useCallback, useMemo, useRef } from "react";
import {
  Group as PanelsGroup,
  type Layout,
  Panel,
  Separator,
} from "react-resizable-panels";

import { useStore } from "@/lib/state/store";
import type {
  PanelGroup as PanelGroupNode,
  PanelNode,
} from "@/lib/state/workspace/tree";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

import { PanelLeafFrame } from "./PanelLeafFrame";

export function PanelHost() {
  const workspace = useStore((s) => s.workspace);
  return <PanelHostNode node={workspace} />;
}

function PanelHostNode({ node }: { node: PanelNode }) {
  if (node.type === "leaf") {
    return <PanelLeafFrame leaf={node} />;
  }
  return <PanelHostGroup group={node} />;
}

function PanelHostGroup({ group }: { group: PanelGroupNode }) {
  const { tokens } = usePlaygroundTheme();
  const setGroupSizes = useStore((s) => s.setGroupSizes);
  // Avoid spamming the store during drags — only commit when sizes
  // actually change beyond a tiny epsilon. Sizes are 0–100 percentages.
  const lastSizesRef = useRef<number[]>(group.sizes);

  const defaultLayout = useMemo<Layout>(() => {
    const layout: Layout = {};
    group.children.forEach((child, i) => {
      layout[child.id] = group.sizes[i] ?? 100 / group.children.length;
    });
    return layout;
  }, [group.children, group.sizes]);

  const onLayoutChanged = useCallback(
    (layout: Layout) => {
      const nextSizes = group.children.map((child) => layout[child.id] ?? 0);
      const prev = lastSizesRef.current;
      const same =
        prev.length === nextSizes.length &&
        prev.every((p, i) => Math.abs(p - nextSizes[i]!) < 0.05);
      if (same) return;
      lastSizesRef.current = nextSizes;
      setGroupSizes(group.id, nextSizes);
    },
    [group.id, group.children, setGroupSizes],
  );

  return (
    <PanelsGroup
      // Key includes id + direction so flipping orientation re-instantiates
      // the underlying group widget cleanly. Without this the library can
      // hold onto stale layout when the axis changes.
      key={`${group.id}-${group.direction}`}
      id={group.id}
      orientation={group.direction === "row" ? "horizontal" : "vertical"}
      defaultLayout={defaultLayout}
      onLayoutChanged={onLayoutChanged}
      style={{ height: "100%", width: "100%" }}
    >
      {group.children.map((child, i) => (
        <Fragment key={child.id}>
          {i > 0 && (
            <Separator
              style={{
                background: tokens.border.subtle,
                width: group.direction === "row" ? 4 : "100%",
                height: group.direction === "column" ? 4 : "100%",
                cursor: group.direction === "row" ? "col-resize" : "row-resize",
              }}
            />
          )}
          <Panel
            id={child.id}
            defaultSize={group.sizes[i] ?? 100 / group.children.length}
            minSize={10}
            style={{
              display: "flex",
              flexDirection: "column",
              minHeight: 0,
              minWidth: 0,
            }}
          >
            <PanelHostNode node={child} />
          </Panel>
        </Fragment>
      ))}
    </PanelsGroup>
  );
}
