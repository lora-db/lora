"use client";

/**
 * Tabular results via Glide Data Grid. The grid takes a `getCellContent`
 * callback that's invoked per visible cell — we infer the cell kind from
 * the per-column `cellType` produced by the adapter, and from the actual
 * runtime value as a fallback.
 */

import dynamic from "next/dynamic";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Center, Text } from "@mantine/core";
import type {
  GridCell,
  GridColumn,
  Item,
  Theme as GlideTheme,
} from "@glideapps/glide-data-grid";
import { GridCellKind } from "@glideapps/glide-data-grid";

import "@glideapps/glide-data-grid/dist/index.css";

import type { AdaptedResult, CellType } from "@/lib/db/types";
import { pickLabel, primaryLabel } from "@/lib/db/adapter";
import { inspectNode, inspectRelationship } from "@/lib/actions/inspectActions";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import type { Tokens } from "@/lib/theme/tokens";
import { hexA } from "@/lib/theme/util";

const DataEditor = dynamic(
  () => import("@glideapps/glide-data-grid").then((m) => ({ default: m.DataEditor })),
  { ssr: false, loading: () => <TableSkeleton /> },
);

function TableSkeleton() {
  const { tokens } = usePlaygroundTheme();
  return (
    <Center h="100%" style={{ background: tokens.bg.editor }}>
      <Text size="xs" c={tokens.fg.subtle} ff={tokens.font.mono}>
        Loading table…
      </Text>
    </Center>
  );
}

interface NodeLike {
  kind: "node";
  id: number;
  labels: string[];
  properties: Record<string, unknown>;
}

interface RelLike {
  kind: "relationship";
  id: number;
  startId: number;
  endId: number;
  type: string;
  properties: Record<string, unknown>;
}

interface PathLike {
  kind: "path";
  nodes: number[];
  rels: number[];
}

function isObj(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
function isNodeLike(v: unknown): v is NodeLike {
  return isObj(v) && v.kind === "node";
}
function isRelLike(v: unknown): v is RelLike {
  return isObj(v) && v.kind === "relationship";
}
function isPathLike(v: unknown): v is PathLike {
  return isObj(v) && v.kind === "path";
}

function nodeBubble(n: NodeLike): string {
  const props = n.properties as Record<string, unknown> | undefined;
  const named = props?.["name"];
  if (typeof named === "string" && named.length > 0) return named;
  if (props) {
    for (const key of Object.keys(props)) {
      const value = props[key];
      if (typeof value === "string" && value.length > 0) return value;
    }
  }
  const label = primaryLabel({
    kind: "node",
    id: n.id,
    labels: n.labels,
    // Cast through unknown to keep the adapter helper happy without a
    // wider type import — the helper only reads `labels`/`properties`.
    properties: (n.properties ?? {}) as Record<string, never>,
  });
  if (label) return label;
  // Fall back via pickLabel to share the adapter's preference order.
  return pickLabel({
    kind: "node",
    id: n.id,
    labels: n.labels,
    properties: (n.properties ?? {}) as Record<string, never>,
  });
}

function buildCell(value: unknown, hint: CellType | undefined, tokens: Tokens): GridCell {
  void hint;
  // Null first — applies regardless of the column hint.
  if (value === null || value === undefined) {
    return {
      kind: GridCellKind.Text,
      data: "",
      displayData: "null",
      allowOverlay: false,
      themeOverride: { textDark: tokens.fg.subtle, textMedium: tokens.fg.subtle },
    };
  }

  if (isNodeLike(value)) {
    return {
      kind: GridCellKind.Bubble,
      data: [nodeBubble(value)],
      allowOverlay: false,
      themeOverride: {
        bgBubble: hexA(tokens.accent.info, 0.18),
        textBubble: tokens.accent.info,
      },
    };
  }

  if (isRelLike(value)) {
    return {
      kind: GridCellKind.Bubble,
      data: [value.type],
      allowOverlay: false,
      themeOverride: {
        bgBubble: hexA(tokens.accent.warning, 0.18),
        textBubble: tokens.accent.warning,
      },
    };
  }

  if (isPathLike(value)) {
    const len = value.nodes.length;
    return {
      kind: GridCellKind.Bubble,
      data: [`path (${len} node${len === 1 ? "" : "s"})`],
      allowOverlay: false,
      themeOverride: {
        bgBubble: hexA(tokens.accent.success, 0.18),
        textBubble: tokens.accent.success,
      },
    };
  }

  if (Array.isArray(value)) {
    return {
      kind: GridCellKind.Text,
      data: "",
      displayData: `[${value.length}]`,
      allowOverlay: false,
      themeOverride: { textDark: tokens.fg.muted },
    };
  }

  if (typeof value === "string") {
    return {
      kind: GridCellKind.Text,
      data: value,
      displayData: value,
      allowOverlay: false,
    };
  }

  if (typeof value === "number") {
    return {
      kind: GridCellKind.Number,
      data: value,
      displayData: Number.isFinite(value) ? String(value) : "NaN",
      allowOverlay: false,
    };
  }

  if (typeof value === "boolean") {
    return {
      kind: GridCellKind.Boolean,
      data: value,
      allowOverlay: false,
    };
  }

  if (isObj(value)) {
    const keys = Object.keys(value);
    return {
      kind: GridCellKind.Text,
      data: "",
      displayData: `{${keys.length}}`,
      allowOverlay: false,
      themeOverride: { textDark: tokens.fg.muted },
    };
  }

  // Hint exhausted — last resort.
  const text = String(value);
  return {
    kind: GridCellKind.Text,
    data: text,
    displayData: text,
    allowOverlay: false,
  };
}

export function TableView({ result }: { result: AdaptedResult }) {
  const { grid, tokens } = usePlaygroundTheme();
  const wrapperRef = useRef<HTMLDivElement | null>(null);
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
    const rect = el.getBoundingClientRect();
    setSize({ w: Math.floor(rect.width), h: Math.floor(rect.height) });
    return () => ro.disconnect();
  }, []);

  const columns = useMemo<GridColumn[]>(() => {
    const total = size.w > 0 ? size.w : 800;
    // Reserve ~52px for the row-number gutter; spread the remainder.
    const usable = Math.max(120, total - 52);
    const perColumn = Math.max(120, Math.floor(usable / Math.max(1, result.columns.length)));
    return result.columns.map((title, i) => ({
      title,
      id: `col-${i}`,
      width: perColumn,
    }));
  }, [result.columns, size.w]);

  const getCellContent = useCallback(
    (cell: Item): GridCell => {
      const [col, row] = cell;
      const r = result.rows[row];
      if (!r) {
        return {
          kind: GridCellKind.Text,
          data: "",
          displayData: "",
          allowOverlay: false,
        };
      }
      const value = r.values[col];
      const hint = result.cellTypes[col];
      return buildCell(value, hint, tokens);
    },
    [result, tokens],
  );

  const onCellClicked = useCallback(
    (cell: Item) => {
      const [col, row] = cell;
      const r = result.rows[row];
      if (!r) return;
      const value = r.values[col];
      if (isNodeLike(value)) {
        inspectNode({
          id: value.id,
          labels: value.labels,
          properties: value.properties,
        });
        return;
      }
      if (isRelLike(value)) {
        inspectRelationship({
          id: value.id,
          type: value.type,
          startId: value.startId,
          endId: value.endId,
          properties: value.properties,
        });
      }
    },
    [result],
  );

  if (result.rows.length === 0) {
    return (
      <Center h="100%" style={{ background: tokens.bg.editor }}>
        <Text size="xs" c={tokens.fg.subtle}>
          Query returned no rows
        </Text>
      </Center>
    );
  }

  const theme: Partial<GlideTheme> = grid;

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
        <DataEditor
          columns={columns}
          rows={result.rows.length}
          getCellContent={getCellContent}
          onCellClicked={onCellClicked}
          width={size.w}
          height={size.h}
          theme={theme}
          rowMarkers="number"
          smoothScrollX
          smoothScrollY
        />
      )}
    </div>
  );
}
