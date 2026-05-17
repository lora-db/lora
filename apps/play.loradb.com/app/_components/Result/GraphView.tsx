"use client";

/**
 * Renders the result's `GraphData` via `LoraGraphCanvas`. The canvas
 * needs explicit pixel dimensions, so we measure the wrapper with a
 * `ResizeObserver` and re-render whenever the container resizes.
 *
 * The canvas's imperative handle is captured via a ref so we can call
 * `screenshot()` for the Phase 4b PNG export. The export trigger is a
 * window event (`GRAPH_PNG_EVENT`) so unrelated parts of the UI — the
 * Spotlight palette, a header button — can request a PNG without
 * threading refs through React.
 */

import dynamic from "next/dynamic";
import { useEffect, useMemo, useRef, useState } from "react";
import { Center, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import type { LoraGraphCanvasHandle } from "@loradb/lora-graph-canvas";

import type { AdaptedResult } from "@/lib/db/types";
import { useStore } from "@/lib/state/store";
import { inspectNode, inspectRelationship } from "@/lib/actions/inspectActions";
import {
  GRAPH_PNG_EVENT,
  downloadGraphPng,
} from "@/lib/actions/exportActions";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import { openConfirmDeleteDialog } from "@/app/_components/Dialogs/ConfirmDeleteDialog";

const LoraGraphCanvas = dynamic(
  () => import("@loradb/lora-graph-canvas").then((m) => ({ default: m.LoraGraphCanvas })),
  { ssr: false, loading: () => <GraphSkeleton /> },
);

function GraphSkeleton() {
  const { tokens } = usePlaygroundTheme();
  return (
    <Center h="100%" style={{ background: tokens.bg.editor }}>
      <Text size="xs" c={tokens.fg.subtle} ff={tokens.font.mono}>
        Loading graph…
      </Text>
    </Center>
  );
}

export function GraphView({ result }: { result: AdaptedResult }) {
  const { canvas, tokens } = usePlaygroundTheme();
  const graphMode = useStore((s) => s.graphMode);
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<LoraGraphCanvasHandle | null>(null);
  const [size, setSize] = useState<{ w: number; h: number }>({ w: 0, h: 0 });

  useEffect(() => {
    const el = wrapperRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const cr = entry.contentRect;
        setSize({ w: Math.max(0, Math.floor(cr.width)), h: Math.max(0, Math.floor(cr.height)) });
      }
    });
    ro.observe(el);
    // Seed with current size so the first paint isn't 0×0.
    const rect = el.getBoundingClientRect();
    setSize({ w: Math.floor(rect.width), h: Math.floor(rect.height) });
    return () => ro.disconnect();
  }, []);

  // PNG export bridge — listens for the global request event and calls
  // the canvas's imperative `screenshot()` handle. We keep this in
  // GraphView (rather than ResultPane) so we already hold the ref.
  useEffect(() => {
    if (typeof window === "undefined") return undefined;
    const onRequest = () => {
      (async () => {
        const handle = canvasRef.current;
        if (!handle) {
          notifications.show({
            color: "yellow",
            title: "Nothing to export",
            message: "Run a query that returns graph data first.",
          });
          return;
        }
        try {
          const blob = await handle.screenshot();
          if (!blob) {
            notifications.show({
              color: "red",
              title: "PNG export failed",
              message: "The canvas could not produce an image.",
            });
            return;
          }
          downloadGraphPng(blob);
        } catch (err) {
          notifications.show({
            color: "red",
            title: "PNG export failed",
            message: err instanceof Error ? err.message : String(err),
          });
        }
      })().catch(() => {
        /* swallowed by inner try/catch */
      });
    };
    window.addEventListener(GRAPH_PNG_EVENT, onRequest);
    return () => {
      window.removeEventListener(GRAPH_PNG_EVENT, onRequest);
    };
  }, []);

  // The store uses immer, which freezes nested objects. LoraGraphCanvas
  // mutates nodes/links in-place (e.g. assigns `_neighbors` and physics
  // state), so we hand it a shallow-cloned copy of each entry.
  const data = useMemo(() => {
    if (!result.graph) return null;
    return {
      nodes: result.graph.nodes.map((n) => ({ ...n })),
      links: result.graph.links.map((l) => ({ ...l })),
    };
  }, [result.graph]);

  if (!data) {
    return (
      <Center h="100%" style={{ background: tokens.bg.editor }}>
        <Text size="xs" c={tokens.fg.subtle}>
          No graph data in this result
        </Text>
      </Center>
    );
  }

  return (
    <div
      ref={wrapperRef}
      style={{
        position: "relative",
        flex: 1,
        minHeight: 0,
        width: "100%",
        height: "100%",
        background: tokens.bg.editor,
        overflow: "hidden",
      }}
    >
      {size.w > 0 && size.h > 0 && (
        <LoraGraphCanvas
          ref={canvasRef}
          // `defaultData` (uncontrolled) so user-driven edits — delete,
          // duplicate, add-connected, drag-to-pin — apply locally to the
          // canvas's own state. The parent remounts GraphView via a
          // `key={runId}` on every new query, so the seed re-applies
          // and edits don't bleed across runs.
          defaultData={data}
          theme={canvas}
          mode={graphMode}
          width={size.w}
          height={size.h}
          enableTooltip
          highlightNeighborsOnHover
          autoIndexNeighbors
          onBeforeNodeDelete={(nodes, { source }) =>
            // Imperative calls (host-driven, no user gesture) skip the
            // confirm modal — only user-initiated deletes prompt.
            source === "imperative"
              ? true
              : openConfirmDeleteDialog({ nodes, links: [], source })
          }
          onBeforeLinkDelete={(links, { source }) =>
            source === "imperative"
              ? true
              : openConfirmDeleteDialog({ nodes: [], links, source })
          }
          onNodeClick={(node) => {
            inspectNode({
              id: node.id,
              labels: node.group !== undefined ? [String(node.group)] : [],
              properties: node as Record<string, unknown>,
            });
          }}
          onLinkClick={(link) => {
            // `source`/`target` may have been replaced by the engine
            // with full NodeObject references; reduce to ids so the
            // drawer copies behave deterministically.
            const sourceId =
              typeof link.source === "object" && link.source !== null
                ? (link.source as { id: string | number }).id
                : (link.source as string | number);
            const targetId =
              typeof link.target === "object" && link.target !== null
                ? (link.target as { id: string | number }).id
                : (link.target as string | number);
            inspectRelationship({
              id: link.id ?? `${String(sourceId)}->${String(targetId)}`,
              type: link.label ?? "",
              startId: sourceId,
              endId: targetId,
              properties: link as Record<string, unknown>,
            });
          }}
        />
      )}
    </div>
  );
}
