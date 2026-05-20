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
import { Center, Group, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { IconInfoCircle } from "@tabler/icons-react";
import type { LoraGraphCanvasHandle } from "@loradb/lora-graph-canvas";

import type { AdaptedResult } from "@/lib/db/types";
import { useStore } from "@/lib/state/store";
import { inspectNode, inspectRelationship } from "@/lib/actions/inspectActions";
import {
  GRAPH_PNG_EVENT,
  downloadGraphPng,
  isGraphPngEvent,
} from "@/lib/actions/exportActions";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import { openConfirmDeleteDialog } from "@/app/_components/Dialogs/ConfirmDeleteDialog";

const LoraGraphCanvas = dynamic(
  () =>
    import("@loradb/lora-graph-canvas").then((m) => ({
      default: m.LoraGraphCanvas,
    })),
  { ssr: false, loading: () => <GraphSkeleton /> },
);

// 3d-force-graph mutates node/link objects in place: it adds `_neighbors`
// and `_links` arrays of cross-references, rewrites link `source`/`target`
// from ids to NodeObject refs, and adds physics state. Those refs are
// cyclic, so handing the raw object to an immer-managed store causes
// `finalize` to recurse forever ("Maximum call stack size exceeded").
//
// The adapter stashes the user-meaningful payload on the canvas object
// under `_properties` / `_labels` / `_type` (see lib/db/adapter.ts).
// Those are plain frozen values from the result, so they're guaranteed
// acyclic and safe to hand to the inspect slice.
function readProperties(obj: unknown): Record<string, unknown> {
  if (!obj || typeof obj !== "object") return {};
  const v = (obj as { _properties?: unknown })._properties;
  return v && typeof v === "object" ? (v as Record<string, unknown>) : {};
}
function readLabels(obj: unknown): string[] {
  if (!obj || typeof obj !== "object") return [];
  const v = (obj as { _labels?: unknown })._labels;
  return Array.isArray(v) ? v.map((x) => String(x)) : [];
}
function readType(obj: unknown): string {
  if (!obj || typeof obj !== "object") return "";
  const v = (obj as { _type?: unknown })._type;
  return typeof v === "string" ? v : "";
}

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

export function GraphView({
  result,
  paneId,
}: {
  result: AdaptedResult;
  /** Pane this graph belongs to — used to scope the PNG export event. */
  paneId?: string;
}) {
  const { canvas, tokens } = usePlaygroundTheme();
  const graphMode = useStore((s) => s.graphMode);
  const focusOnNodeClick = useStore((s) => s.focusOnNodeClick);
  const alwaysShowLabels = useStore((s) => s.alwaysShowLabels);
  const fitOnSelect = useStore((s) => s.fitOnSelect);
  const nodeCap = useStore((s) => s.nodeCap);
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<LoraGraphCanvasHandle | null>(null);
  const [size, setSize] = useState<{ w: number; h: number }>({ w: 0, h: 0 });

  useEffect(() => {
    const el = wrapperRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const cr = entry.contentRect;
        setSize({
          w: Math.max(0, Math.floor(cr.width)),
          h: Math.max(0, Math.floor(cr.height)),
        });
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
    const onRequest = (event: Event) => {
      // Disambiguate when multiple graph panes are mounted. A `null`
      // event payload means "active pane"; we let the store decide
      // which pane that is.
      if (isGraphPngEvent(event)) {
        const detail = event.detail;
        if (detail.paneId !== null && detail.paneId !== paneId) return;
        if (detail.paneId === null && paneId !== undefined) {
          // Only the pane matching `activePaneId` should respond.
          const activeId = useStore.getState().activePaneId;
          if (activeId !== paneId) return;
        }
      }
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
  }, [paneId]);

  // The store uses immer, which freezes nested objects. LoraGraphCanvas
  // mutates nodes/links in-place (e.g. assigns `_neighbors` and physics
  // state), so we hand it a shallow-cloned copy of each entry.
  //
  // The `nodeCap` pref puts a ceiling on how many nodes we hand to the
  // canvas. Past ~a few thousand the force layout starts to chew CPU
  // even at idle, so the cap protects the playground from accidental
  // RETURN-everything queries. Truncation is deterministic (first N in
  // adapter order) and links whose endpoints fall outside the kept set
  // are dropped — the canvas would otherwise throw "node not found".
  const { data, truncation } = useMemo(() => {
    if (!result.graph) return { data: null, truncation: null };
    const totalNodes = result.graph.nodes.length;
    const capped =
      Number.isFinite(nodeCap) && nodeCap > 0 && totalNodes > nodeCap;
    const limit = capped ? nodeCap : totalNodes;
    const nodes = result.graph.nodes.slice(0, limit).map((n) => ({ ...n }));
    let links;
    if (capped) {
      const keptIds = new Set(nodes.map((n) => n.id));
      links = result.graph.links
        .filter((l) => {
          const sId =
            typeof l.source === "object" && l.source !== null
              ? (l.source as { id: string | number }).id
              : (l.source as string | number);
          const tId =
            typeof l.target === "object" && l.target !== null
              ? (l.target as { id: string | number }).id
              : (l.target as string | number);
          return keptIds.has(sId) && keptIds.has(tId);
        })
        .map((l) => ({ ...l }));
    } else {
      links = result.graph.links.map((l) => ({ ...l }));
    }
    return {
      data: { nodes, links },
      truncation: capped
        ? {
            kept: limit,
            total: totalNodes,
            droppedLinks: result.graph.links.length - links.length,
          }
        : null,
    };
  }, [result.graph, nodeCap]);

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
      {truncation ? (
        <Group
          gap={6}
          wrap="nowrap"
          style={{
            position: "absolute",
            bottom: 8,
            left: 8,
            zIndex: 2,
            padding: "4px 8px",
            borderRadius: tokens.radius.sm,
            background: tokens.bg.panel,
            border: `1px solid ${tokens.border.subtle}`,
            color: tokens.fg.muted,
            pointerEvents: "none",
            maxWidth: "calc(100% - 16px)",
          }}
        >
          <IconInfoCircle size={14} />
          <Text size="xs" c={tokens.fg.muted}>
            Showing {truncation.kept.toLocaleString()} of{" "}
            {truncation.total.toLocaleString()} nodes (node cap). Raise the cap
            in Settings to render more.
          </Text>
        </Group>
      ) : null}
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
          // Colour nodes by their primary `:Label`. The adapter populates
          // `node.group` with `primaryLabel(n)`, so this fans nodes out
          // across the theme's `nodePalette` deterministically — same
          // label → same swatch across runs and across the group legend.
          nodeAutoColorBy="group"
          focusOnClick={focusOnNodeClick}
          showLabels={alwaysShowLabels}
          fitOnSelect={fitOnSelect}
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
          onNodeClick={(node, event) => {
            inspectNode(
              {
                id: node.id,
                labels: readLabels(node),
                properties: readProperties(node),
              },
              { anchor: { x: event.clientX, y: event.clientY } },
            );
          }}
          onLinkClick={(link, event) => {
            // `source`/`target` may have been replaced by the engine
            // with full NodeObject references; reduce to ids so the
            // popup copies behave deterministically.
            const sourceId =
              typeof link.source === "object" && link.source !== null
                ? (link.source as { id: string | number }).id
                : (link.source as string | number);
            const targetId =
              typeof link.target === "object" && link.target !== null
                ? (link.target as { id: string | number }).id
                : (link.target as string | number);
            inspectRelationship(
              {
                id: link.id ?? `${String(sourceId)}->${String(targetId)}`,
                type: readType(link) || link.label || "",
                startId: sourceId,
                endId: targetId,
                properties: readProperties(link),
              },
              { anchor: { x: event.clientX, y: event.clientY } },
            );
          }}
        />
      )}
    </div>
  );
}
