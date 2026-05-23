"use client";

/**
 * `StatsPanel` — sidebar view over the live `GraphStats` and
 * `MemoryReport` snapshots returned by the WASM engine.
 *
 * Mirrors the SchemaDesignPanel structural pattern: top header with
 * refresh + freshness label, scrollable body with grouped sections.
 * Two RPCs back the view (`graphStats()` and `memoryReport()`); both
 * walk owned graph structures once and are cheap on small/medium
 * graphs but not on a hot path, so we refresh on user action and on
 * the WASM mutation event (debounced ~600ms) rather than polling.
 */

import { useEffect, useMemo, useState } from "react";
import {
  ActionIcon,
  Badge,
  Center,
  Group,
  Loader,
  Progress,
  ScrollArea,
  Stack,
  Text,
  Tooltip,
} from "@mantine/core";
import { IconRefresh } from "@tabler/icons-react";
import { formatDistanceToNowStrict } from "date-fns";
import type { MemoryReportSnapshot } from "@loradb/lora-wasm";

import { useStore } from "@/lib/state/store";
import {
  attachStatsMutationListener,
  refreshStats,
} from "@/lib/actions/statsActions";
import { memoryReportTotalBytes } from "@/lib/state/slices/stats";
import { formatBytes, formatCount } from "@/lib/util/format";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

function useFreshness(fetchedAt: number | null): string | null {
  const [, setTick] = useState(0);
  useEffect(() => {
    if (fetchedAt === null) return;
    const id = window.setInterval(() => setTick((n) => n + 1), 15_000);
    return () => window.clearInterval(id);
  }, [fetchedAt]);
  if (fetchedAt === null) return null;
  return formatDistanceToNowStrict(fetchedAt, { addSuffix: true });
}

function graphCoreBytes(r: MemoryReportSnapshot): number {
  return (
    r.nodesBytes +
    r.relationshipsBytes +
    r.outgoingBytes +
    r.incomingBytes +
    r.labelIndexBytes +
    r.typeIndexBytes
  );
}

function indexBytes(r: MemoryReportSnapshot): number {
  return (
    r.propertyIndexBytes +
    r.sortedIndexBytes +
    r.textIndexBytes +
    r.pointIndexBytes +
    r.fulltextIndexBytes +
    r.vectorIndexBytes
  );
}

function catalogBytes(r: MemoryReportSnapshot): number {
  return r.indexCatalogBytes + r.constraintCatalogBytes;
}

interface ComponentRow {
  label: string;
  bytes: number;
  /** When set, displayed as a smaller dimmed line under the label. */
  detail?: string;
}

const SECTION_HEADER = {
  fontSize: 10,
  letterSpacing: 0.8,
  textTransform: "uppercase" as const,
};

export function StatsPanel() {
  const { tokens } = usePlaygroundTheme();
  const stats = useStore((s) => s.graphStats);
  const memory = useStore((s) => s.memoryReport);
  const refreshing = useStore((s) => s.statsRefreshing);
  const fetchedAt = useStore((s) => s.statsFetchedAt);
  const history = useStore((s) => s.statsHistory);

  // Initial fetch + listener attach. The listener teardown runs on
  // unmount; the initial fetch only fires when we don't already have a
  // snapshot so re-mounts (HMR, panel toggle) don't double-fetch.
  useEffect(() => {
    if (stats === null || memory === null) void refreshStats();
    const detach = attachStatsMutationListener();
    return detach;
  }, [stats, memory]);

  const freshness = useFreshness(fetchedAt);

  const totalBytes = memory ? memoryReportTotalBytes(memory) : 0;
  const coreBytes = memory ? graphCoreBytes(memory) : 0;
  const idxBytes = memory ? indexBytes(memory) : 0;
  const catBytes = memory ? catalogBytes(memory) : 0;

  // Section→list of rows that the body table renders. Pre-computed
  // here so the JSX stays declarative.
  const components = useMemo<{ section: string; rows: ComponentRow[] }[]>(
    () =>
      memory
        ? [
            {
              section: "Graph core",
              rows: [
                {
                  label: "Node slab",
                  bytes: memory.nodesBytes,
                  detail: `${formatCount(memory.liveNodeCount)} live · ${formatCount(memory.nodeTombstoneCount)} tombstones`,
                },
                {
                  label: "Relationship slab",
                  bytes: memory.relationshipsBytes,
                  detail: `${formatCount(memory.liveRelationshipCount)} live · ${formatCount(memory.relationshipTombstoneCount)} tombstones`,
                },
                { label: "Outgoing adjacency", bytes: memory.outgoingBytes },
                { label: "Incoming adjacency", bytes: memory.incomingBytes },
                { label: "Label index", bytes: memory.labelIndexBytes },
                { label: "Rel-type index", bytes: memory.typeIndexBytes },
              ],
            },
            {
              section: "Secondary indexes",
              rows: [
                {
                  label: "Property (hash)",
                  bytes: memory.propertyIndexBytes,
                },
                { label: "Range (sorted)", bytes: memory.sortedIndexBytes },
                { label: "Text (trigram)", bytes: memory.textIndexBytes },
                { label: "Point (grid)", bytes: memory.pointIndexBytes },
                { label: "Fulltext (TF)", bytes: memory.fulltextIndexBytes },
                { label: "Vector", bytes: memory.vectorIndexBytes },
              ],
            },
            {
              section: "Catalogs",
              rows: [
                { label: "Index catalog", bytes: memory.indexCatalogBytes },
                {
                  label: "Constraint catalog",
                  bytes: memory.constraintCatalogBytes,
                },
              ],
            },
          ]
        : [],
    [memory],
  );

  return (
    <Stack gap={0} style={{ flex: 1, minHeight: 0 }}>
      {/* Header — refresh + freshness, lifted from SchemaDesignPanel
          so the visual rhythm matches the other panels. */}
      <Group
        justify="space-between"
        align="center"
        wrap="nowrap"
        px={12}
        py={8}
        style={{ borderBottom: `1px solid ${tokens.border.subtle}` }}
      >
        <Stack gap={0}>
          <Text
            size="xs"
            fw={600}
            c={tokens.fg.muted}
            style={{ letterSpacing: 1, textTransform: "uppercase" }}
          >
            Stats
          </Text>
          {freshness ? (
            <Text size="xs" c={tokens.fg.subtle} style={{ fontSize: 10 }}>
              updated {freshness}
            </Text>
          ) : null}
        </Stack>
        <Group gap={4} wrap="nowrap">
          {refreshing ? <Loader size="xs" /> : null}
          <Tooltip label="Refresh" withArrow>
            <ActionIcon
              variant="subtle"
              size="sm"
              color="gray"
              onClick={() => void refreshStats()}
              aria-label="Refresh stats"
              disabled={refreshing}
            >
              <IconRefresh size={14} />
            </ActionIcon>
          </Tooltip>
        </Group>
      </Group>

      <ScrollArea style={{ flex: 1, minHeight: 0 }}>
        {memory === null && stats === null ? (
          <Center py={48}>
            <Text size="xs" c={tokens.fg.subtle}>
              {refreshing ? "Reading…" : "No data yet."}
            </Text>
          </Center>
        ) : (
          <Stack gap={16} px={12} py={12}>
            {/* Cardinality — the two numbers users glance at first. */}
            <Group gap={8} grow>
              <StatTile
                label="Nodes"
                value={stats ? formatCount(stats.nodeCount) : "—"}
                hint={
                  memory
                    ? `${formatBytes(memory.nodesBytes / Math.max(1, memory.liveNodeCount))} / node`
                    : null
                }
              />
              <StatTile
                label="Relationships"
                value={stats ? formatCount(stats.relationshipCount) : "—"}
                hint={
                  memory
                    ? `${formatBytes(memory.relationshipsBytes / Math.max(1, memory.liveRelationshipCount))} / rel`
                    : null
                }
              />
            </Group>

            {/* Memory bar — three coloured sections summing to total. */}
            <Stack gap={6}>
              <Group justify="space-between" gap={4}>
                <Text style={SECTION_HEADER} c={tokens.fg.muted}>
                  Memory
                </Text>
                <Text size="xs" fw={600}>
                  {formatBytes(totalBytes)}
                </Text>
              </Group>
              <Progress.Root size="lg" radius="sm">
                {totalBytes > 0 ? (
                  <>
                    <Progress.Section
                      value={(coreBytes / totalBytes) * 100}
                      color="blue"
                    />
                    <Progress.Section
                      value={(idxBytes / totalBytes) * 100}
                      color="teal"
                    />
                    <Progress.Section
                      value={(catBytes / totalBytes) * 100}
                      color="gray"
                    />
                  </>
                ) : null}
              </Progress.Root>
              <Group gap={10}>
                <LegendDot color="var(--mantine-color-blue-6)" />
                <Text size="xs" c={tokens.fg.muted}>
                  Graph {formatBytes(coreBytes)}
                </Text>
                <LegendDot color="var(--mantine-color-teal-6)" />
                <Text size="xs" c={tokens.fg.muted}>
                  Indexes {formatBytes(idxBytes)}
                </Text>
                <LegendDot color="var(--mantine-color-gray-6)" />
                <Text size="xs" c={tokens.fg.muted}>
                  Catalogs {formatBytes(catBytes)}
                </Text>
              </Group>
            </Stack>

            {/* Sparkline — total bytes over the last few refreshes. */}
            {history.length >= 2 ? (
              <Stack gap={4}>
                <Text style={SECTION_HEADER} c={tokens.fg.muted}>
                  Trend (recent samples)
                </Text>
                <Sparkline
                  values={history.map((s) => s.totalBytes)}
                  color="var(--mantine-color-teal-6)"
                />
              </Stack>
            ) : null}

            {/* Per-component breakdown. */}
            <Stack gap={12}>
              {components.map(({ section, rows }) => (
                <Stack key={section} gap={4}>
                  <Text style={SECTION_HEADER} c={tokens.fg.muted}>
                    {section}
                  </Text>
                  <Stack gap={2}>
                    {rows.map((row) => (
                      <ComponentLine
                        key={row.label}
                        row={row}
                        totalBytes={totalBytes}
                      />
                    ))}
                  </Stack>
                </Stack>
              ))}
            </Stack>

            {/* Per-label / per-rel-type cardinality. */}
            {stats && stats.nodesByLabel.length > 0 ? (
              <Stack gap={4}>
                <Text style={SECTION_HEADER} c={tokens.fg.muted}>
                  Nodes by label
                </Text>
                <Stack gap={2}>
                  {stats.nodesByLabel.map(({ label, count }) => (
                    <KeyValueLine
                      key={`label-${label}`}
                      label={label}
                      value={formatCount(count)}
                    />
                  ))}
                </Stack>
              </Stack>
            ) : null}

            {stats && stats.relationshipsByType.length > 0 ? (
              <Stack gap={4}>
                <Text style={SECTION_HEADER} c={tokens.fg.muted}>
                  Relationships by type
                </Text>
                <Stack gap={2}>
                  {stats.relationshipsByType.map(({ label, count }) => (
                    <KeyValueLine
                      key={`reltype-${label}`}
                      label={label}
                      value={formatCount(count)}
                    />
                  ))}
                </Stack>
              </Stack>
            ) : null}

            {/* Active secondary indexes — chips grouped by index kind. */}
            {stats ? <IndexInventory stats={stats} /> : null}

            <Text size="xs" c={tokens.fg.subtle} style={{ fontSize: 10 }}>
              Numbers are approximate — fixed amortised overheads for
              BTree/HashMap entries. See the engine&apos;s MemoryReport docs for
              the methodology.
            </Text>
          </Stack>
        )}
      </ScrollArea>
    </Stack>
  );
}

function StatTile({
  label,
  value,
  hint,
}: {
  label: string;
  value: string;
  hint: string | null;
}) {
  const { tokens } = usePlaygroundTheme();
  return (
    <Stack
      gap={2}
      px={10}
      py={8}
      style={{
        background: tokens.bg.panel,
        borderRadius: tokens.radius.sm,
        border: `1px solid ${tokens.border.subtle}`,
      }}
    >
      <Text size="xs" c={tokens.fg.muted} style={SECTION_HEADER}>
        {label}
      </Text>
      <Text fw={700} size="md">
        {value}
      </Text>
      {hint ? (
        <Text size="xs" c={tokens.fg.subtle} style={{ fontSize: 10 }}>
          {hint}
        </Text>
      ) : null}
    </Stack>
  );
}

function LegendDot({ color }: { color: string }) {
  return (
    <span
      aria-hidden
      style={{
        display: "inline-block",
        width: 8,
        height: 8,
        borderRadius: 999,
        background: color,
      }}
    />
  );
}

function ComponentLine({
  row,
  totalBytes,
}: {
  row: ComponentRow;
  totalBytes: number;
}) {
  const { tokens } = usePlaygroundTheme();
  const pct = totalBytes > 0 ? (row.bytes / totalBytes) * 100 : 0;
  return (
    <Group justify="space-between" gap={4} wrap="nowrap">
      <Stack gap={0} style={{ minWidth: 0, flex: 1 }}>
        <Text size="xs" truncate="end">
          {row.label}
        </Text>
        {row.detail ? (
          <Text size="xs" c={tokens.fg.subtle} style={{ fontSize: 10 }}>
            {row.detail}
          </Text>
        ) : null}
      </Stack>
      <Group gap={6} wrap="nowrap">
        <Text size="xs" fw={600}>
          {formatBytes(row.bytes)}
        </Text>
        <Text
          size="xs"
          c={tokens.fg.subtle}
          style={{ fontSize: 10, minWidth: 36, textAlign: "right" }}
        >
          {pct >= 1 ? `${pct.toFixed(0)}%` : pct > 0 ? "<1%" : "0%"}
        </Text>
      </Group>
    </Group>
  );
}

function KeyValueLine({ label, value }: { label: string; value: string }) {
  return (
    <Group justify="space-between" gap={4} wrap="nowrap">
      <Text size="xs" truncate="end" style={{ minWidth: 0, flex: 1 }}>
        {label}
      </Text>
      <Text size="xs" fw={600}>
        {value}
      </Text>
    </Group>
  );
}

interface IndexInventoryProps {
  stats: NonNullable<ReturnType<typeof useStore.getState>["graphStats"]>;
}

function IndexInventory({ stats }: IndexInventoryProps) {
  const { tokens } = usePlaygroundTheme();
  // Flatten every index list with a per-kind color so the chip row
  // doubles as a legend.
  const groups: Array<{
    kind: string;
    color: string;
    items: { label: string; property: string }[];
  }> = [
    {
      kind: "RANGE",
      color: "blue",
      items: [...stats.nodeRangeIndexes, ...stats.relationshipRangeIndexes],
    },
    {
      kind: "TEXT",
      color: "teal",
      items: [...stats.nodeTextIndexes, ...stats.relationshipTextIndexes],
    },
    {
      kind: "POINT",
      color: "violet",
      items: [...stats.nodePointIndexes, ...stats.relationshipPointIndexes],
    },
    {
      kind: "VECTOR",
      color: "grape",
      items: [...stats.nodeVectorIndexes, ...stats.relationshipVectorIndexes],
    },
  ].filter((g) => g.items.length > 0);

  if (groups.length === 0) return null;

  return (
    <Stack gap={4}>
      <Text style={SECTION_HEADER} c={tokens.fg.muted}>
        Active secondary indexes
      </Text>
      <Stack gap={6}>
        {groups.map(({ kind, color, items }) => (
          <Group key={kind} gap={4} wrap="wrap">
            <Badge color={color} variant="light" size="xs">
              {kind}
            </Badge>
            {items.map((item, i) => (
              <Text
                key={`${kind}-${item.label}-${item.property}-${i}`}
                size="xs"
                c={tokens.fg.muted}
              >
                {item.label}.{item.property}
              </Text>
            ))}
          </Group>
        ))}
      </Stack>
    </Stack>
  );
}

/**
 * Bare-bones SVG sparkline. Avoids bringing in a chart dep just to
 * draw N points — total bytes are normalised to the local min/max so
 * the line fills the box regardless of absolute scale.
 */
function Sparkline({ values, color }: { values: number[]; color: string }) {
  const width = 240;
  const height = 36;
  const max = Math.max(...values);
  const min = Math.min(...values);
  const span = max - min || 1;
  const stepX = values.length > 1 ? width / (values.length - 1) : width;

  const path = values
    .map((v, i) => {
      const x = i * stepX;
      const y = height - ((v - min) / span) * height;
      return `${i === 0 ? "M" : "L"}${x.toFixed(1)} ${y.toFixed(1)}`;
    })
    .join(" ");

  return (
    <svg
      viewBox={`0 0 ${width} ${height}`}
      width="100%"
      height={height}
      aria-label="Total memory trend"
      role="img"
    >
      <path d={path} fill="none" stroke={color} strokeWidth={1.5} />
    </svg>
  );
}
