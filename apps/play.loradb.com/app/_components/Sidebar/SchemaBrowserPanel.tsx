"use client";

/**
 * `SchemaBrowserPanel` — read-only tree view over the cached
 * {@link SchemaSnapshot}. The first mount kicks off an introspection
 * pass if the slice has never been populated; from then on the panel
 * lives entirely off the store (other surfaces, like the editor's
 * mutation hook, drive subsequent refreshes).
 *
 * Clicking a label or rel-type copies its name to the clipboard
 * (Phase 4 will replace this with "insert MATCH (n:Label) RETURN n"
 * into the active editor tab).
 */

import { useEffect, useMemo, useState } from "react";
import {
  Accordion,
  ActionIcon,
  Badge,
  Center,
  CloseButton,
  Group,
  Loader,
  ScrollArea,
  Stack,
  Text,
  TextInput,
  Tooltip,
  UnstyledButton,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import {
  IconArrowRight,
  IconKey,
  IconRefresh,
  IconSearch,
  IconTag,
} from "@tabler/icons-react";
import { formatDistanceToNowStrict } from "date-fns";

import { useStore } from "@/lib/state/store";
import { refreshSchema } from "@/lib/actions/schemaActions";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

function copyToClipboard(text: string, kind: string): void {
  if (typeof window === "undefined" || !navigator.clipboard) return;
  navigator.clipboard
    .writeText(text)
    .then(() => {
      notifications.show({
        color: "green",
        title: "Copied",
        message: `${kind} "${text}" copied to clipboard.`,
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

/**
 * Hook: returns a string like "12s ago" / "3m ago" that re-renders
 * every 15 seconds while mounted. Returns `null` when no timestamp is
 * available so the caller can hide the "updated" suffix.
 */
function useFreshness(fetchedAt: number | undefined): string | null {
  const [, setTick] = useState(0);
  useEffect(() => {
    if (fetchedAt === undefined) return;
    const id = window.setInterval(() => {
      setTick((n) => n + 1);
    }, 15_000);
    return () => {
      window.clearInterval(id);
    };
  }, [fetchedAt]);
  if (fetchedAt === undefined) return null;
  return formatDistanceToNowStrict(fetchedAt, { addSuffix: true });
}

export function SchemaBrowserPanel() {
  const { tokens } = usePlaygroundTheme();
  const schema = useStore((s) => s.schema);
  const refreshing = useStore((s) => s.refreshing);

  // Kick off an initial introspection if the slice hasn't been
  // populated yet. The Editor pane also calls this on mount; both
  // calls are de-duped by `refreshing` racing into the same store.
  useEffect(() => {
    if (schema !== null) return;
    void refreshSchema();
  }, [schema]);

  const freshness = useFreshness(schema?.fetchedAt);
  const [filter, setFilter] = useState("");

  // Read directly off the schema slice. We avoid `?? []` defaults at
  // this level because those literals would change reference every
  // render and bust the filter useMemo's equality check; instead the
  // filter callback below guards against undefined.
  const countsByLabel = schema?.countsByLabel ?? {};

  // Filter every section by the same substring so users can narrow the
  // whole tree with one input. A label/rel-type matches if its name OR
  // any of its property keys matches.
  const { labels, relTypes, propertyKeys } = useMemo(() => {
    const allLabels = schema?.labels ?? [];
    const allRelTypes = schema?.relTypes ?? [];
    const allPropertyKeys = schema?.propertyKeys ?? [];
    const propertiesByLabel = schema?.propertiesByLabel ?? {};
    const propertiesByRelType = schema?.propertiesByRelType ?? {};
    const q = filter.trim().toLowerCase();
    if (q.length === 0) {
      return {
        labels: allLabels,
        relTypes: allRelTypes,
        propertyKeys: allPropertyKeys,
      };
    }
    const matchKeys = (keys: readonly string[]): boolean =>
      keys.some((k) => k.toLowerCase().includes(q));
    return {
      labels: allLabels.filter(
        (l) =>
          l.toLowerCase().includes(q) ||
          matchKeys(propertiesByLabel[l] ?? []),
      ),
      relTypes: allRelTypes.filter(
        (r) =>
          r.toLowerCase().includes(q) ||
          matchKeys(propertiesByRelType[r] ?? []),
      ),
      propertyKeys: allPropertyKeys.filter((k) => k.toLowerCase().includes(q)),
    };
  }, [filter, schema]);

  const allLabels = schema?.labels ?? [];
  const allRelTypes = schema?.relTypes ?? [];
  const allPropertyKeys = schema?.propertyKeys ?? [];
  const propertiesByLabel = schema?.propertiesByLabel ?? {};
  const propertiesByRelType = schema?.propertiesByRelType ?? {};

  const handleRefresh = (): void => {
    void refreshSchema();
  };

  const hasAny =
    allLabels.length > 0 ||
    allRelTypes.length > 0 ||
    allPropertyKeys.length > 0;
  const filterActive = filter.trim().length > 0;
  const hasMatches =
    labels.length > 0 || relTypes.length > 0 || propertyKeys.length > 0;

  return (
    <Stack gap={0} style={{ flex: 1, minHeight: 0 }}>
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
            Schema
          </Text>
          {freshness ? (
            <Text
              size="xs"
              c={tokens.fg.subtle}
              component="time"
              dateTime={
                schema ? new Date(schema.fetchedAt).toISOString() : undefined
              }
              style={{ fontSize: 10 }}
            >
              updated {freshness}
            </Text>
          ) : null}
        </Stack>
        <Group gap={4} wrap="nowrap">
          {refreshing ? <Loader size="xs" /> : null}
          <Tooltip label="Refresh schema" withArrow>
            <ActionIcon
              variant="subtle"
              size="sm"
              color="gray"
              onClick={handleRefresh}
              aria-label="Refresh schema"
              disabled={refreshing}
            >
              <IconRefresh size={14} />
            </ActionIcon>
          </Tooltip>
        </Group>
      </Group>

      {hasAny && (
        <Group
          px={8}
          py={6}
          style={{ borderBottom: `1px solid ${tokens.border.subtle}` }}
        >
          <TextInput
            size="xs"
            value={filter}
            onChange={(e) => setFilter(e.currentTarget.value)}
            placeholder="Filter schema"
            aria-label="Filter schema"
            leftSection={<IconSearch size={12} />}
            rightSection={
              filterActive ? (
                <CloseButton
                  size="xs"
                  onClick={() => setFilter("")}
                  aria-label="Clear filter"
                />
              ) : null
            }
            style={{ flex: 1 }}
          />
        </Group>
      )}

      <ScrollArea style={{ flex: 1, minHeight: 0 }}>
        {!hasAny ? (
          <Center p="md">
            <Stack gap="xs" align="center">
              <IconTag size={28} color={tokens.fg.subtle} stroke={1.5} />
              <Text size="xs" c={tokens.fg.subtle} ta="center">
                No schema yet — run a query first.
              </Text>
            </Stack>
          </Center>
        ) : filterActive && !hasMatches ? (
          <Center p="md">
            <Text size="xs" c={tokens.fg.subtle} ta="center">
              No schema entries match &ldquo;{filter}&rdquo;
            </Text>
          </Center>
        ) : (
          <Stack gap={4} p={8}>
            {/* Labels --------------------------------------------------- */}
            <SectionHeader
              icon={<IconTag size={12} />}
              iconColor={tokens.category.label}
              title="Labels"
              count={labels.length}
              tokens={tokens}
            />
            {labels.length === 0 ? (
              <Text size="xs" c={tokens.fg.subtle} px={8} py={4}>
                None
              </Text>
            ) : (
              <Accordion
                variant="filled"
                multiple
                chevronPosition="left"
                chevronSize={14}
                styles={{
                  item: { background: "transparent", border: "none" },
                  control: {
                    padding: "4px 8px",
                    minHeight: 0,
                    borderRadius: tokens.radius.sm,
                  },
                  label: { padding: 0 },
                  content: { padding: "2px 8px 6px 56px" },
                }}
              >
                {labels.map((label) => {
                  const count = countsByLabel[label] ?? 0;
                  const keys = propertiesByLabel[label] ?? [];
                  return (
                    <Accordion.Item key={label} value={label}>
                      <Accordion.Control>
                        <Group
                          justify="space-between"
                          wrap="nowrap"
                          gap={6}
                          onClick={(e) => {
                            // Forward double-clicks on the label text
                            // to the clipboard copy, but let single
                            // clicks open the accordion as usual.
                            if (e.detail === 2) {
                              e.preventDefault();
                              e.stopPropagation();
                              copyToClipboard(label, "Label");
                            }
                          }}
                        >
                          <Group gap={6} wrap="nowrap" style={{ minWidth: 0 }}>
                            <IconTag
                              size={12}
                              color={tokens.category.label}
                              stroke={2}
                            />
                            <Text
                              size="sm"
                              fw={500}
                              c={tokens.category.label}
                              truncate
                              title={label}
                            >
                              {label}
                            </Text>
                          </Group>
                          <Badge
                            size="xs"
                            variant="light"
                            color="gray"
                            radius="sm"
                          >
                            {count}
                          </Badge>
                        </Group>
                      </Accordion.Control>
                      <Accordion.Panel>
                        <PropertyKeyList keys={keys} tokens={tokens} />
                      </Accordion.Panel>
                    </Accordion.Item>
                  );
                })}
              </Accordion>
            )}

            {/* Relationship types -------------------------------------- */}
            <SectionHeader
              icon={<IconArrowRight size={12} />}
              iconColor={tokens.category.relType}
              title="Relationship types"
              count={relTypes.length}
              tokens={tokens}
            />
            {relTypes.length === 0 ? (
              <Text size="xs" c={tokens.fg.subtle} px={8} py={4}>
                None
              </Text>
            ) : (
              <Accordion
                variant="filled"
                multiple
                chevronPosition="left"
                chevronSize={14}
                styles={{
                  item: { background: "transparent", border: "none" },
                  control: {
                    padding: "4px 8px",
                    minHeight: 0,
                    borderRadius: tokens.radius.sm,
                  },
                  label: { padding: 0 },
                  content: { padding: "2px 8px 6px 56px" },
                }}
              >
                {relTypes.map((rt) => {
                  const keys = propertiesByRelType[rt] ?? [];
                  return (
                    <Accordion.Item key={rt} value={rt}>
                      <Accordion.Control>
                        <Group
                          gap={6}
                          wrap="nowrap"
                          style={{ minWidth: 0 }}
                          onClick={(e) => {
                            if (e.detail === 2) {
                              e.preventDefault();
                              e.stopPropagation();
                              copyToClipboard(rt, "Relationship type");
                            }
                          }}
                        >
                          <IconArrowRight
                            size={12}
                            color={tokens.category.relType}
                            stroke={2}
                          />
                          <Text
                            size="sm"
                            fw={500}
                            c={tokens.category.relType}
                            truncate
                            title={rt}
                          >
                            {rt}
                          </Text>
                        </Group>
                      </Accordion.Control>
                      <Accordion.Panel>
                        <PropertyKeyList keys={keys} tokens={tokens} />
                      </Accordion.Panel>
                    </Accordion.Item>
                  );
                })}
              </Accordion>
            )}

            {/* Property keys ------------------------------------------- */}
            <SectionHeader
              icon={<IconKey size={12} />}
              title="Property keys"
              count={propertyKeys.length}
              tokens={tokens}
            />
            {propertyKeys.length === 0 ? (
              <Text size="xs" c={tokens.fg.subtle} px={8} py={4}>
                None
              </Text>
            ) : (
              <Stack gap={0} px={4}>
                {propertyKeys.map((key) => (
                  <FlatRow
                    key={key}
                    icon={
                      <IconKey size={12} color={tokens.fg.subtle} stroke={2} />
                    }
                    label={key}
                    onClick={() => {
                      copyToClipboard(key, "Property key");
                    }}
                    tokens={tokens}
                  />
                ))}
              </Stack>
            )}
          </Stack>
        )}
      </ScrollArea>
    </Stack>
  );
}

// ---------------------------------------------------------------------------
// Internal subcomponents
// ---------------------------------------------------------------------------

interface SectionHeaderProps {
  icon: React.ReactNode;
  iconColor?: string;
  title: string;
  count: number;
  tokens: ReturnType<typeof usePlaygroundTheme>["tokens"];
}

function SectionHeader({ icon, iconColor, title, count, tokens }: SectionHeaderProps) {
  return (
    <Group
      gap={6}
      wrap="nowrap"
      px={8}
      pt={8}
      pb={2}
      style={{ alignItems: "center" }}
    >
      <span style={{ display: "inline-flex", color: iconColor ?? tokens.fg.muted }}>
        {icon}
      </span>
      <Text
        size="xs"
        fw={600}
        c={tokens.fg.muted}
        style={{ letterSpacing: 0.5, textTransform: "uppercase" }}
      >
        {title}
      </Text>
      <Badge size="xs" variant="light" color="gray" radius="sm">
        {count}
      </Badge>
    </Group>
  );
}

interface PropertyKeyListProps {
  keys: readonly string[];
  tokens: ReturnType<typeof usePlaygroundTheme>["tokens"];
}

/**
 * Render the per-label / per-rel-type property keys list. Displays
 * an explicit "(no properties)" hint when the introspected list is
 * empty so the user can tell the difference between "we don't know"
 * and "this label has no property keys".
 */
function PropertyKeyList({ keys, tokens }: PropertyKeyListProps) {
  if (keys.length === 0) {
    return (
      <Text size="xs" c={tokens.fg.subtle}>
        (no properties)
      </Text>
    );
  }
  return (
    <Stack gap={2}>
      {keys.map((key) => (
        <Group key={key} gap={6} wrap="nowrap" style={{ minWidth: 0 }}>
          <IconKey size={10} color={tokens.fg.subtle} stroke={2} />
          <Text size="xs" c={tokens.fg.muted} truncate title={key}>
            {key}
          </Text>
        </Group>
      ))}
    </Stack>
  );
}

interface FlatRowProps {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
  tokens: ReturnType<typeof usePlaygroundTheme>["tokens"];
}

function FlatRow({ icon, label, onClick, tokens }: FlatRowProps) {
  return (
    <UnstyledButton
      onClick={onClick}
      style={{
        padding: "4px 8px",
        borderRadius: tokens.radius.sm,
        color: tokens.fg.primary,
      }}
    >
      <Group gap={6} wrap="nowrap" style={{ minWidth: 0 }}>
        {icon}
        <Text size="sm" c={tokens.fg.primary} truncate title={label}>
          {label}
        </Text>
      </Group>
    </UnstyledButton>
  );
}
