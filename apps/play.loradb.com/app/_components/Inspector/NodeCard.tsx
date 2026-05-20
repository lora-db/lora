"use client";

/**
 * The floating popup that replaces the old right-side inspector
 * drawer. One card per active inspection — pinned cards persist
 * across new clicks, the unpinned card swaps target on the next click.
 *
 * Positioning rules:
 *   - If the inspection carries a `position` (user-dragged), it wins.
 *   - Else if it carries an `anchor` (click coords), the card sits a
 *     few pixels down-right of the anchor, clamped to the viewport.
 *   - Else the card falls back to the right edge of the viewport,
 *     matching the old drawer feel.
 */

import dynamic from "next/dynamic";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ActionIcon,
  Box,
  Code,
  Group,
  Paper,
  ScrollArea,
  Stack,
  Tabs,
  Text,
  TextInput,
  Tooltip,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import {
  IconGripVertical,
  IconPin,
  IconPinned,
  IconRoute,
  IconSearch,
  IconX,
} from "@tabler/icons-react";

import { runActiveTab } from "@/lib/actions/runActiveTab";
import { openTabInCell } from "@/lib/actions/tabActions";
import { useStore } from "@/lib/state/store";
import type { Inspection, InspectTarget } from "@/lib/state/slices/inspect";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import { CategoryBadge } from "../CategoryBadge";

import {
  GROUP_LABEL,
  GROUP_ORDER,
  PropertyValue,
  pickTitleProperty,
  renderValueText,
  semanticGroupFor,
  type SemanticGroup,
} from "./propertyValue";

const DEFAULT_WIDTH = 380;
const DEFAULT_HEIGHT = 520;
const MIN_WIDTH = 280;
const MIN_HEIGHT = 280;
const VIEWPORT_MARGIN = 16;

// `LoraJsonEditor` mounts CodeMirror — never on the server, so we
// dynamic-import it the same way the rest of the workbench does.
const LoraJsonEditor = dynamic(
  () =>
    import("@loradb/lora-query").then((m) => ({ default: m.LoraJsonEditor })),
  { ssr: false },
);

export function NodeCard({ inspection }: { inspection: Inspection }) {
  const { tokens } = usePlaygroundTheme();
  const pin = useStore((s) => s.pinInspection);
  const close = useStore((s) => s.closeInspection);
  const move = useStore((s) => s.moveInspection);
  const resize = useStore((s) => s.resizeInspection);
  const bringToFront = useStore((s) => s.bringInspectionToFront);

  const cardRef = useRef<HTMLDivElement | null>(null);
  const [tab, setTab] = useState<string | null>("overview");

  const position = useMemo(() => computePosition(inspection), [inspection]);

  const onPin = useCallback(() => {
    pin(inspection.key);
  }, [inspection.key, pin]);

  const onClose = useCallback(() => {
    close(inspection.key);
  }, [inspection.key, close]);

  const onMouseDownAnywhere = useCallback(() => {
    bringToFront(inspection.key);
  }, [inspection.key, bringToFront]);

  // Drag from the header strip. We track the down-coords, the original
  // card position, and update the store on every move so the position
  // is part of the single source of truth.
  const dragState = useRef<{
    startX: number;
    startY: number;
    origX: number;
    origY: number;
  } | null>(null);

  const onHeaderPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      // Only drag from the explicit grip / header background, not from
      // action buttons (they handle their own pointer events).
      const target = e.target as HTMLElement;
      if (target.closest("[data-no-drag]")) return;
      const rect = cardRef.current?.getBoundingClientRect();
      if (!rect) return;
      e.preventDefault();
      (e.target as HTMLElement).setPointerCapture(e.pointerId);
      dragState.current = {
        startX: e.clientX,
        startY: e.clientY,
        origX: rect.left,
        origY: rect.top,
      };
    },
    [],
  );

  const onHeaderPointerMove = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const drag = dragState.current;
      if (!drag) return;
      const next = clampToViewport({
        x: drag.origX + (e.clientX - drag.startX),
        y: drag.origY + (e.clientY - drag.startY),
      });
      move(inspection.key, next);
    },
    [inspection.key, move],
  );

  const onHeaderPointerUp = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      dragState.current = null;
      try {
        (e.target as HTMLElement).releasePointerCapture(e.pointerId);
      } catch {
        // releasePointerCapture throws when the capture has already
        // been released (e.g. capture target was unmounted) — safe to
        // ignore.
      }
    },
    [],
  );

  // Resize from the bottom-right corner grip. Mirrors the drag
  // pattern — capture the pointer, track deltas against the size at
  // pointer-down, and write back through the store so size is part
  // of the same source of truth as position.
  const resizeStateRef = useRef<{
    startX: number;
    startY: number;
    origW: number;
    origH: number;
  } | null>(null);

  const onResizePointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const rect = cardRef.current?.getBoundingClientRect();
      if (!rect) return;
      e.stopPropagation();
      e.preventDefault();
      (e.target as HTMLElement).setPointerCapture(e.pointerId);
      resizeStateRef.current = {
        startX: e.clientX,
        startY: e.clientY,
        origW: rect.width,
        origH: rect.height,
      };
    },
    [],
  );

  const onResizePointerMove = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const st = resizeStateRef.current;
      if (!st) return;
      const maxW =
        typeof window !== "undefined"
          ? window.innerWidth - VIEWPORT_MARGIN * 2
          : Infinity;
      const maxH =
        typeof window !== "undefined"
          ? window.innerHeight - VIEWPORT_MARGIN * 2
          : Infinity;
      const width = Math.min(
        maxW,
        Math.max(MIN_WIDTH, st.origW + (e.clientX - st.startX)),
      );
      const height = Math.min(
        maxH,
        Math.max(MIN_HEIGHT, st.origH + (e.clientY - st.startY)),
      );
      resize(inspection.key, { width, height });
    },
    [inspection.key, resize],
  );

  const onResizePointerUp = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      resizeStateRef.current = null;
      try {
        (e.target as HTMLElement).releasePointerCapture(e.pointerId);
      } catch {
        // see onHeaderPointerUp — safe to ignore.
      }
    },
    [],
  );

  // Card-level keyboard shortcuts. Only fire when the card has focus.
  const onCardKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
        return;
      }
      if (e.key === "p" || e.key === "P") {
        onPin();
        return;
      }
      if (e.key === "c" || e.key === "C") {
        copyToClipboard(String(inspection.target.id), "ID");
        return;
      }
      if (e.key === "j" || e.key === "J") {
        copyAsJson(inspection.target);
      }
    },
    [inspection.target, onClose, onPin],
  );

  useEffect(() => {
    // Bring the card to front when it first mounts so a fresh click
    // always wins layering against older pinned cards.
    bringToFront(inspection.key);
  }, [inspection.key, bringToFront]);

  return (
    <Paper
      ref={cardRef}
      withBorder
      shadow="md"
      onMouseDown={onMouseDownAnywhere}
      onKeyDown={onCardKeyDown}
      tabIndex={-1}
      style={{
        position: "fixed",
        left: position.x,
        top: position.y,
        width: inspection.size?.width ?? DEFAULT_WIDTH,
        height: inspection.size?.height ?? DEFAULT_HEIGHT,
        minWidth: MIN_WIDTH,
        minHeight: MIN_HEIGHT,
        background: tokens.bg.panel,
        borderColor: tokens.border.subtle,
        borderRadius: tokens.radius.md,
        overflow: "hidden",
        zIndex: 200 + inspection.z,
        display: "flex",
        flexDirection: "column",
      }}
    >
      <CardHeader
        target={inspection.target}
        pinned={inspection.pinned}
        onPin={onPin}
        onClose={onClose}
        onPointerDown={onHeaderPointerDown}
        onPointerMove={onHeaderPointerMove}
        onPointerUp={onHeaderPointerUp}
      />
      <Tabs
        value={tab}
        onChange={setTab}
        keepMounted={false}
        styles={{
          root: {
            flex: 1,
            minHeight: 0,
            display: "flex",
            flexDirection: "column",
          },
          list: { paddingInline: 8 },
          panel: {
            flex: 1,
            minHeight: 0,
            display: "flex",
            flexDirection: "column",
          },
        }}
      >
        <Tabs.List>
          <Tabs.Tab value="overview">Overview</Tabs.Tab>
          <Tabs.Tab value="properties">Properties</Tabs.Tab>
          {inspection.target.kind === "node" && (
            <Tabs.Tab value="neighbors">Neighbors</Tabs.Tab>
          )}
          <Tabs.Tab value="raw">JSON</Tabs.Tab>
        </Tabs.List>
        <Tabs.Panel value="overview">
          <ScrollArea
            style={{ flex: 1, minHeight: 0 }}
            styles={{ viewport: { padding: 12 } }}
          >
            <OverviewPanel target={inspection.target} />
          </ScrollArea>
        </Tabs.Panel>
        <Tabs.Panel value="properties">
          <ScrollArea
            style={{ flex: 1, minHeight: 0 }}
            styles={{ viewport: { padding: 12 } }}
          >
            <PropertiesPanel target={inspection.target} />
          </ScrollArea>
        </Tabs.Panel>
        {inspection.target.kind === "node" && (
          <Tabs.Panel value="neighbors">
            <ScrollArea
              style={{ flex: 1, minHeight: 0 }}
              styles={{ viewport: { padding: 12 } }}
            >
              <NeighborsPanel target={inspection.target} />
            </ScrollArea>
          </Tabs.Panel>
        )}
        <Tabs.Panel value="raw">
          <RawPanel target={inspection.target} />
        </Tabs.Panel>
      </Tabs>
      <ResizeGrip
        onPointerDown={onResizePointerDown}
        onPointerMove={onResizePointerMove}
        onPointerUp={onResizePointerUp}
      />
    </Paper>
  );
}

// ---------------------------------------------------------------------------
// Resize grip
// ---------------------------------------------------------------------------

function ResizeGrip({
  onPointerDown,
  onPointerMove,
  onPointerUp,
}: {
  onPointerDown: (e: React.PointerEvent<HTMLDivElement>) => void;
  onPointerMove: (e: React.PointerEvent<HTMLDivElement>) => void;
  onPointerUp: (e: React.PointerEvent<HTMLDivElement>) => void;
}) {
  const { tokens } = usePlaygroundTheme();
  // Three diagonal hash strokes pinned to the bottom-right corner —
  // the universally-recognised "drag to resize" affordance. Sized
  // big enough to be an easy pointer target without dominating the
  // card chrome.
  const stroke = tokens.fg.subtle;
  return (
    <div
      data-no-drag
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize inspector"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      style={{
        position: "absolute",
        right: 0,
        bottom: 0,
        width: 18,
        height: 18,
        cursor: "nwse-resize",
        zIndex: 2,
        backgroundImage: `linear-gradient(135deg, transparent 0 40%, ${stroke} 40% 48%, transparent 48% 60%, ${stroke} 60% 68%, transparent 68% 80%, ${stroke} 80% 88%, transparent 88% 100%)`,
        opacity: 0.55,
        transition: "opacity 120ms",
      }}
      onPointerEnter={(e) => {
        (e.currentTarget as HTMLDivElement).style.opacity = "1";
      }}
      onPointerLeave={(e) => {
        (e.currentTarget as HTMLDivElement).style.opacity = "0.55";
      }}
    />
  );
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

interface CardHeaderProps {
  target: InspectTarget;
  pinned: boolean;
  onPin: () => void;
  onClose: () => void;
  onPointerDown: (e: React.PointerEvent<HTMLDivElement>) => void;
  onPointerMove: (e: React.PointerEvent<HTMLDivElement>) => void;
  onPointerUp: (e: React.PointerEvent<HTMLDivElement>) => void;
}

function CardHeader({
  target,
  pinned,
  onPin,
  onClose,
  onPointerDown,
  onPointerMove,
  onPointerUp,
}: CardHeaderProps) {
  const { tokens } = usePlaygroundTheme();

  const title =
    target.kind === "node" ? deriveNodeTitle(target) : `:${target.type || "?"}`;

  return (
    <div
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      style={{
        display: "flex",
        flexDirection: "column",
        gap: 6,
        padding: "10px 12px 8px",
        borderBottom: `1px solid ${tokens.border.subtle}`,
        background: tokens.bg.overlay,
        cursor: "grab",
        userSelect: "none",
      }}
    >
      <Group gap={6} wrap="nowrap" align="center">
        <IconGripVertical size={12} color={tokens.fg.subtle} />
        <Group gap={4} wrap="wrap" style={{ flex: 1, minWidth: 0 }}>
          {target.kind === "node" ? (
            target.labels.map((l) => (
              <CategoryBadge key={l} kind="label" size="xs">
                {l}
              </CategoryBadge>
            ))
          ) : (
            <CategoryBadge kind="relType" size="xs">
              :{target.type || "?"}
            </CategoryBadge>
          )}
        </Group>
        <Group gap={2} wrap="nowrap" data-no-drag>
          <Tooltip label={pinned ? "Unpin" : "Pin"} withArrow>
            <ActionIcon
              size="sm"
              variant={pinned ? "filled" : "subtle"}
              color={pinned ? "yellow" : "gray"}
              onClick={onPin}
              aria-label={pinned ? "Unpin inspector" : "Pin inspector"}
            >
              {pinned ? <IconPinned size={12} /> : <IconPin size={12} />}
            </ActionIcon>
          </Tooltip>
          <Tooltip label="Close (Esc)" withArrow>
            <ActionIcon
              size="sm"
              variant="subtle"
              color="gray"
              onClick={onClose}
              aria-label="Close inspector"
            >
              <IconX size={12} />
            </ActionIcon>
          </Tooltip>
        </Group>
      </Group>
      <Group gap={6} wrap="nowrap" align="baseline">
        <Text size="sm" fw={600} c={tokens.fg.primary} lineClamp={1}>
          {title}
        </Text>
        <Tooltip label="Copy id" withArrow>
          <Code
            data-no-drag
            onClick={() => copyToClipboard(String(target.id), "ID")}
            style={{
              cursor: "pointer",
              fontSize: 10,
              background: "transparent",
              color: tokens.fg.subtle,
            }}
          >
            id {String(target.id)}
          </Code>
        </Tooltip>
      </Group>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Panels
// ---------------------------------------------------------------------------

function OverviewPanel({ target }: { target: InspectTarget }) {
  const { tokens } = usePlaygroundTheme();
  const propCount = Object.keys(target.properties).length;
  return (
    <Stack gap="sm">
      {target.kind === "node" ? (
        <SchemaChips labels={target.labels} properties={target.properties} />
      ) : (
        <Stack gap={4}>
          <Text size="xs" c={tokens.fg.muted} tt="uppercase" fw={600}>
            Endpoints
          </Text>
          <Code style={{ background: tokens.bg.app, fontSize: 11 }}>
            {String(target.startId)} → {String(target.endId)}
          </Code>
        </Stack>
      )}
      <Stack gap={4}>
        <Text size="xs" c={tokens.fg.muted} tt="uppercase" fw={600}>
          Summary
        </Text>
        <Text size="xs" c={tokens.fg.primary}>
          {propCount} {propCount === 1 ? "property" : "properties"}
          {target.kind === "node" && target.labels.length > 0
            ? ` · ${target.labels.length} ${target.labels.length === 1 ? "label" : "labels"}`
            : ""}
        </Text>
      </Stack>
      <TopProperties target={target} />
    </Stack>
  );
}

function TopProperties({ target }: { target: InspectTarget }) {
  const { tokens } = usePlaygroundTheme();
  const titleKey =
    target.kind === "node" ? pickTitleProperty(target.properties) : null;
  const keys = Object.keys(target.properties);
  if (keys.length === 0) {
    return (
      <Text size="xs" c={tokens.fg.subtle}>
        No properties.
      </Text>
    );
  }
  const head = titleKey
    ? [titleKey, ...keys.filter((k) => k !== titleKey)]
    : keys;
  const shown = head.slice(0, 5);
  return (
    <Stack gap={4}>
      <Text size="xs" c={tokens.fg.muted} tt="uppercase" fw={600}>
        Highlights
      </Text>
      <Stack gap={6}>
        {shown.map((k) => (
          <PropertyRow key={k} k={k} v={target.properties[k]} />
        ))}
      </Stack>
      {keys.length > shown.length ? (
        <Text size="xs" c={tokens.fg.subtle}>
          +{keys.length - shown.length} more on the Properties tab
        </Text>
      ) : null}
    </Stack>
  );
}

function PropertiesPanel({ target }: { target: InspectTarget }) {
  const { tokens } = usePlaygroundTheme();
  const [query, setQuery] = useState("");

  const constraints = useStore((s) => s.constraints);
  const constrainedKeys = useMemo(() => {
    const set = new Set<string>();
    if (!constraints || target.kind !== "node") return set;
    const labels = target.labels;
    for (const c of constraints) {
      if (c.entity !== "NODE") continue;
      if (!labels.includes(c.label)) continue;
      for (const p of c.properties) set.add(p);
    }
    return set;
  }, [constraints, target]);

  const allKeys = Object.keys(target.properties);

  const grouped = useMemo(() => {
    const buckets: Record<SemanticGroup, string[]> = {
      identifiers: [],
      descriptors: [],
      temporal: [],
      spatial: [],
      other: [],
    };
    const q = query.trim().toLowerCase();
    for (const k of allKeys) {
      if (q && !k.toLowerCase().includes(q)) {
        const v = target.properties[k];
        const text = renderValueText(v).toLowerCase();
        if (!text.includes(q)) continue;
      }
      const g = semanticGroupFor(k, target.properties[k], constrainedKeys);
      buckets[g].push(k);
    }
    return buckets;
  }, [allKeys, constrainedKeys, query, target.properties]);

  if (allKeys.length === 0) {
    return (
      <Text size="xs" c={tokens.fg.subtle}>
        No properties.
      </Text>
    );
  }

  return (
    <Stack gap="sm">
      <TextInput
        size="xs"
        placeholder="Filter properties…"
        value={query}
        onChange={(e) => setQuery(e.currentTarget.value)}
        leftSection={<IconSearch size={12} />}
      />
      {GROUP_ORDER.map((g) => {
        const keys = grouped[g];
        if (keys.length === 0) return null;
        return (
          <Stack gap={4} key={g}>
            <Text size="xs" c={tokens.fg.muted} tt="uppercase" fw={600}>
              {GROUP_LABEL[g]}
            </Text>
            <Stack gap={6}>
              {keys.map((k) => (
                <PropertyRow
                  key={k}
                  k={k}
                  v={target.properties[k]}
                  highlight={constrainedKeys.has(k)}
                />
              ))}
            </Stack>
          </Stack>
        );
      })}
    </Stack>
  );
}

function PropertyRow({
  k,
  v,
  highlight,
}: {
  k: string;
  v: unknown;
  highlight?: boolean;
}) {
  const { tokens } = usePlaygroundTheme();
  return (
    <Group
      align="flex-start"
      gap="sm"
      wrap="nowrap"
      style={{
        background: highlight ? tokens.bg.overlay : undefined,
        borderRadius: tokens.radius.sm,
        padding: highlight ? "4px 6px" : 0,
      }}
      onDoubleClick={() => copyToClipboard(renderValueText(v), `Value of ${k}`)}
    >
      <Text
        size="xs"
        c={tokens.fg.muted}
        ff={tokens.font.mono}
        style={{
          minWidth: 96,
          maxWidth: 120,
          wordBreak: "break-word",
        }}
      >
        {k}
      </Text>
      <div style={{ flex: 1, minWidth: 0 }}>
        <PropertyValue value={v} />
      </div>
    </Group>
  );
}

function NeighborsPanel({
  target,
}: {
  target: Extract<InspectTarget, { kind: "node" }>;
}) {
  const { tokens } = usePlaygroundTheme();
  // Static, schema-aware neighbor list. We don't run a query on
  // open — the user explicitly jumps into one. Pulls candidate
  // rel-types from the schema cache when available.
  const schema = useStore((s) => s.schema);
  const relTypes = schema?.relTypes ?? [];

  return (
    <Stack gap="sm">
      <Stack gap={4}>
        <Text size="xs" c={tokens.fg.muted} tt="uppercase" fw={600}>
          Quick traversals
        </Text>
        <NeighborJumpRow
          caption="All neighbors"
          cypher={cypherForNeighbors(target)}
        />
        <NeighborJumpRow
          caption="Outgoing only"
          cypher={cypherForNeighbors(target, "OUT")}
        />
        <NeighborJumpRow
          caption="Incoming only"
          cypher={cypherForNeighbors(target, "IN")}
        />
      </Stack>
      {relTypes.length > 0 ? (
        <Stack gap={4}>
          <Text size="xs" c={tokens.fg.muted} tt="uppercase" fw={600}>
            By relationship type
          </Text>
          {relTypes.map((rt) => (
            <NeighborJumpRow
              key={rt}
              caption={`:${rt}`}
              cypher={cypherForNeighbors(target, "ANY", rt)}
            />
          ))}
        </Stack>
      ) : (
        <Text size="xs" c={tokens.fg.subtle}>
          Run a query first to populate the schema cache with relationship
          types.
        </Text>
      )}
    </Stack>
  );
}

function NeighborJumpRow({
  caption,
  cypher,
}: {
  caption: string;
  cypher: string;
}) {
  const { tokens } = usePlaygroundTheme();
  return (
    <Group
      gap={6}
      wrap="nowrap"
      onClick={() => {
        openTabInCell({ name: caption, body: cypher });
        void runActiveTab();
      }}
      style={{
        cursor: "pointer",
        padding: "4px 6px",
        borderRadius: tokens.radius.sm,
        background: tokens.bg.app,
      }}
    >
      <Text size="xs" c={tokens.fg.primary} style={{ flex: 1 }}>
        {caption}
      </Text>
      <IconRoute size={11} color={tokens.fg.subtle} />
    </Group>
  );
}

function RawPanel({ target }: { target: InspectTarget }) {
  const { jsonEditor: jsonEditorTheme } = usePlaygroundTheme();
  const text = useMemo(() => {
    try {
      return JSON.stringify(target, null, 2);
    } catch {
      return String(target);
    }
  }, [target]);
  return (
    <Box
      style={{
        flex: 1,
        minHeight: 0,
        display: "flex",
        padding: 12,
      }}
    >
      <LoraJsonEditor
        value={text}
        readOnly
        theme={jsonEditorTheme}
        showLineNumbers={false}
        minHeight="100%"
        style={{ flex: 1, minHeight: 0 }}
      />
    </Box>
  );
}

// ---------------------------------------------------------------------------
// Schema chips
// ---------------------------------------------------------------------------

function SchemaChips({
  labels,
  properties,
}: {
  labels: string[];
  properties: Record<string, unknown>;
}) {
  const { tokens } = usePlaygroundTheme();
  const indexes = useStore((s) => s.indexes);
  const constraints = useStore((s) => s.constraints);

  const matchingIndexes = useMemo(() => {
    if (!indexes) return [];
    return indexes.filter(
      (idx) =>
        idx.entity === "NODE" &&
        idx.labelsOrTypes.some((l) => labels.includes(l)) &&
        idx.properties.every((p) => p in properties),
    );
  }, [indexes, labels, properties]);

  const matchingConstraints = useMemo(() => {
    if (!constraints) return [];
    return constraints.filter(
      (c) =>
        c.entity === "NODE" &&
        labels.includes(c.label) &&
        c.properties.every((p) => p in properties),
    );
  }, [constraints, labels, properties]);

  if (matchingIndexes.length === 0 && matchingConstraints.length === 0) {
    return null;
  }

  return (
    <Stack gap={4}>
      <Text size="xs" c={tokens.fg.muted} tt="uppercase" fw={600}>
        Schema
      </Text>
      <Group gap={4} wrap="wrap">
        {matchingConstraints.map((c) => (
          <CategoryBadge key={`c-${c.name}`} kind="parameter" size="xs">
            {c.kind.replace("_", " ")} ({c.properties.join(", ")})
          </CategoryBadge>
        ))}
        {matchingIndexes.map((idx) => (
          <CategoryBadge key={`i-${idx.name}`} kind="variable" size="xs">
            {idx.kind} idx ({idx.properties.join(", ")})
          </CategoryBadge>
        ))}
      </Group>
    </Stack>
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function deriveNodeTitle(
  target: Extract<InspectTarget, { kind: "node" }>,
): string {
  const titleKey = pickTitleProperty(target.properties);
  if (titleKey) {
    return String(target.properties[titleKey]);
  }
  const primary = target.labels[0];
  return primary ? `${primary} ${target.id}` : `Node ${target.id}`;
}

function computePosition(insp: Inspection): { x: number; y: number } {
  if (typeof window === "undefined") return { x: 100, y: 100 };
  const width = insp.size?.width ?? DEFAULT_WIDTH;
  if (insp.position) return clampToViewport(insp.position, width);
  if (insp.anchor) {
    return clampToViewport(
      { x: insp.anchor.x + 12, y: insp.anchor.y + 12 },
      width,
    );
  }
  // Fallback: right-anchored, matching the old drawer feel.
  return {
    x: Math.max(VIEWPORT_MARGIN, window.innerWidth - width - VIEWPORT_MARGIN),
    y: VIEWPORT_MARGIN + 48,
  };
}

function clampToViewport(
  pos: { x: number; y: number },
  width = DEFAULT_WIDTH,
): { x: number; y: number } {
  if (typeof window === "undefined") return pos;
  const maxX = window.innerWidth - width - VIEWPORT_MARGIN;
  const maxY = window.innerHeight - 160 - VIEWPORT_MARGIN;
  return {
    x: Math.min(
      Math.max(VIEWPORT_MARGIN, pos.x),
      Math.max(VIEWPORT_MARGIN, maxX),
    ),
    y: Math.min(
      Math.max(VIEWPORT_MARGIN, pos.y),
      Math.max(VIEWPORT_MARGIN, maxY),
    ),
  };
}

function copyToClipboard(value: string, label: string): void {
  if (typeof navigator === "undefined" || !navigator.clipboard) {
    notifications.show({
      color: "red",
      title: "Copy failed",
      message: "Clipboard API unavailable in this context.",
    });
    return;
  }
  navigator.clipboard
    .writeText(value)
    .then(() => {
      notifications.show({
        color: "green",
        title: "Copied",
        message: `${label} copied to clipboard.`,
        autoClose: 1500,
      });
    })
    .catch((err: unknown) => {
      notifications.show({
        color: "red",
        title: "Copy failed",
        message: err instanceof Error ? err.message : String(err),
      });
    });
}

function copyAsJson(target: InspectTarget): void {
  const payload =
    target.kind === "node"
      ? {
          id: target.id,
          labels: target.labels,
          properties: target.properties,
        }
      : {
          id: target.id,
          type: target.type,
          startId: target.startId,
          endId: target.endId,
          properties: target.properties,
        };
  try {
    copyToClipboard(JSON.stringify(payload, null, 2), "JSON");
  } catch {
    copyToClipboard(String(payload), "JSON");
  }
}

function cypherForNeighbors(
  target: InspectTarget,
  direction: "IN" | "OUT" | "ANY" = "ANY",
  relType?: string,
): string {
  const idLiteral =
    typeof target.id === "number"
      ? String(target.id)
      : `"${String(target.id)}"`;
  const left = direction === "IN" ? "<-" : "-";
  const right = direction === "OUT" ? "->" : "-";
  const rel = relType ? `[r:\`${relType}\`]` : `[r]`;
  return (
    `MATCH (n)${left}${rel}${right}(m)\n` +
    `WHERE id(n) = ${idLiteral}\n` +
    `RETURN n, r, m`
  );
}
