"use client";

/**
 * `SchemaBrowserPanel` — interactive tree over the cached
 * {@link SchemaSnapshot}. The first mount kicks off an introspection
 * pass if the slice has never been populated; from then on the panel
 * lives entirely off the store (other surfaces, like the editor's
 * mutation hook, drive subsequent refreshes).
 *
 * Interactions:
 *  - Click a label / rel-type → insert `MATCH (n:Label) RETURN n`
 *    into the editor. Smart placement: empty tab is overwritten,
 *    non-empty tab spawns a new one.
 *  - Click a property → insert a projection on its parent label
 *    (or a distinct-values query when in the flat list).
 *  - Right-click or click the kebab → context menu with count,
 *    sample, neighbors, distinct, copy.
 *  - Modifiers on any insert: `shift` appends to the active tab,
 *    `alt`/`option` always opens a new tab.
 *  - Chevron toggles property expansion.
 */

import { useEffect, useMemo, useState } from "react";
import {
  ActionIcon,
  Badge,
  Box,
  Center,
  CloseButton,
  Collapse,
  Group,
  Loader,
  Menu,
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
  IconChevronDown,
  IconChevronRight,
  IconCopy,
  IconDots,
  IconFilter,
  IconKey,
  IconList,
  IconPlayerPlay,
  IconRefresh,
  IconRoute,
  IconSearch,
  IconSortAscending,
  IconSum,
  IconTag,
} from "@tabler/icons-react";
import { formatDistanceToNowStrict } from "date-fns";

import { useStore } from "@/lib/state/store";
import { refreshSchema } from "@/lib/actions/schemaActions";
import {
  insertSnippet,
  modeFromEvent,
  type InsertMode,
} from "@/lib/actions/snippetActions";
import {
  labelCount,
  labelDistinctProperty,
  labelMatch,
  labelNeighbors,
  labelSample,
  propertyDistinctAny,
  relTypeCount,
  relTypeDistinctProperty,
  relTypeEndpoints,
  relTypeMatch,
  relTypeProjection,
} from "@/lib/snippets/cypher";
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

function notifyInserted(name: string): void {
  notifications.show({
    color: "blue",
    title: "Inserted",
    message: `Snippet for "${name}" added to the editor.`,
    autoClose: 1800,
  });
}

/** Wrap an insertSnippet call with the standard "inserted X" toast. */
function runInsert(snippet: string, name: string, mode: InsertMode): void {
  insertSnippet(snippet, { mode, name });
  notifyInserted(name);
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
          l.toLowerCase().includes(q) || matchKeys(propertiesByLabel[l] ?? []),
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
              <Stack gap={0}>
                {labels.map((label) => (
                  <LabelRow
                    key={label}
                    label={label}
                    count={countsByLabel[label] ?? 0}
                    properties={propertiesByLabel[label] ?? []}
                    tokens={tokens}
                  />
                ))}
              </Stack>
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
              <Stack gap={0}>
                {relTypes.map((rt) => (
                  <RelTypeRow
                    key={rt}
                    relType={rt}
                    properties={propertiesByRelType[rt] ?? []}
                    tokens={tokens}
                  />
                ))}
              </Stack>
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
                  <PropertyFlatRow key={key} property={key} tokens={tokens} />
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

type Tokens = ReturnType<typeof usePlaygroundTheme>["tokens"];

interface SectionHeaderProps {
  icon: React.ReactNode;
  iconColor?: string;
  title: string;
  count: number;
  tokens: Tokens;
}

function SectionHeader({
  icon,
  iconColor,
  title,
  count,
  tokens,
}: SectionHeaderProps) {
  return (
    <Group
      gap={6}
      wrap="nowrap"
      px={8}
      pt={8}
      pb={2}
      style={{ alignItems: "center" }}
    >
      <span
        style={{ display: "inline-flex", color: iconColor ?? tokens.fg.muted }}
      >
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

// ---------------------------------------------------------------------------
// Label row
// ---------------------------------------------------------------------------

interface LabelRowProps {
  label: string;
  count: number;
  properties: readonly string[];
  tokens: Tokens;
}

function LabelRow({ label, count, properties, tokens }: LabelRowProps) {
  const [expanded, setExpanded] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [hovered, setHovered] = useState(false);

  const onClick = (e: React.MouseEvent): void => {
    runInsert(labelMatch(label), label, modeFromEvent(e));
  };

  return (
    <Box>
      <Group
        gap={2}
        wrap="nowrap"
        onContextMenu={(e) => {
          e.preventDefault();
          setMenuOpen(true);
        }}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        style={{
          padding: "2px 6px",
          borderRadius: tokens.radius.sm,
          background: hovered || menuOpen ? tokens.bg.overlay : "transparent",
        }}
      >
        <ActionIcon
          variant="transparent"
          size="xs"
          color="gray"
          onClick={() => setExpanded((x) => !x)}
          aria-label={expanded ? "Collapse" : "Expand"}
        >
          {expanded ? (
            <IconChevronDown size={12} />
          ) : (
            <IconChevronRight size={12} />
          )}
        </ActionIcon>
        <Tooltip
          label="Click to insert MATCH query · Shift = append · Alt = new tab"
          openDelay={500}
          withArrow
        >
          <UnstyledButton
            onClick={onClick}
            style={{
              flex: 1,
              minWidth: 0,
              padding: "2px 4px",
              borderRadius: tokens.radius.sm,
            }}
          >
            <Group gap={6} wrap="nowrap" style={{ minWidth: 0 }}>
              <IconTag
                size={12}
                color={tokens.category.label}
                stroke={2}
                style={{ flexShrink: 0 }}
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
          </UnstyledButton>
        </Tooltip>
        <Badge size="xs" variant="light" color="gray" radius="sm">
          {count}
        </Badge>
        <LabelRowMenu
          label={label}
          properties={properties}
          opened={menuOpen}
          onOpenChange={setMenuOpen}
          visible={hovered || menuOpen}
        />
      </Group>
      <Collapse in={expanded}>
        <Stack gap={0} pl={32} pr={4} py={2}>
          {properties.length === 0 ? (
            <Text size="xs" c={tokens.fg.subtle}>
              (no properties)
            </Text>
          ) : (
            properties.map((key) => (
              <PropertyOnLabelRow
                key={key}
                label={label}
                kind="label"
                property={key}
                tokens={tokens}
              />
            ))
          )}
        </Stack>
      </Collapse>
    </Box>
  );
}

interface LabelRowMenuProps {
  label: string;
  properties: readonly string[];
  opened: boolean;
  onOpenChange: (v: boolean) => void;
  visible: boolean;
}

function LabelRowMenu({
  label,
  properties,
  opened,
  onOpenChange,
  visible,
}: LabelRowMenuProps) {
  return (
    <Menu
      opened={opened}
      onChange={onOpenChange}
      position="bottom-end"
      shadow="md"
      width={220}
      withinPortal
    >
      <Menu.Target>
        <ActionIcon
          variant="subtle"
          size="xs"
          color="gray"
          aria-label={`Actions for ${label}`}
          style={{
            opacity: visible ? 1 : 0,
            transition: "opacity 120ms",
          }}
        >
          <IconDots size={12} />
        </ActionIcon>
      </Menu.Target>
      <Menu.Dropdown>
        <Menu.Label>{label}</Menu.Label>
        <Menu.Item
          leftSection={<IconPlayerPlay size={14} />}
          onClick={() => runInsert(labelMatch(label), label, "smart")}
        >
          Match all (LIMIT 25)
        </Menu.Item>
        <Menu.Item
          leftSection={<IconSum size={14} />}
          onClick={() =>
            runInsert(labelCount(label), `count(${label})`, "smart")
          }
        >
          Count
        </Menu.Item>
        <Menu.Item
          leftSection={<IconList size={14} />}
          onClick={() =>
            runInsert(labelSample(label), `${label} sample`, "smart")
          }
        >
          Sample one
        </Menu.Item>
        <Menu.Item
          leftSection={<IconRoute size={14} />}
          onClick={() =>
            runInsert(labelNeighbors(label), `${label} neighbors`, "smart")
          }
        >
          Show neighbors
        </Menu.Item>
        {properties.length > 0 ? (
          <Menu
            position="left-start"
            offset={4}
            shadow="md"
            width={200}
            withinPortal
            trigger="hover"
            closeOnItemClick={false}
          >
            <Menu.Target>
              <Menu.Item
                leftSection={<IconFilter size={14} />}
                rightSection={<IconChevronRight size={12} />}
              >
                Distinct values…
              </Menu.Item>
            </Menu.Target>
            <Menu.Dropdown>
              {properties.map((p) => (
                <Menu.Item
                  key={p}
                  leftSection={<IconKey size={12} />}
                  onClick={() =>
                    runInsert(
                      labelDistinctProperty(label, p),
                      `${label}.${p}`,
                      "smart",
                    )
                  }
                >
                  {p}
                </Menu.Item>
              ))}
            </Menu.Dropdown>
          </Menu>
        ) : null}
        <Menu.Divider />
        <Menu.Item
          leftSection={<IconCopy size={14} />}
          onClick={() => copyToClipboard(label, "Label")}
        >
          Copy name
        </Menu.Item>
      </Menu.Dropdown>
    </Menu>
  );
}

// ---------------------------------------------------------------------------
// Rel-type row
// ---------------------------------------------------------------------------

interface RelTypeRowProps {
  relType: string;
  properties: readonly string[];
  tokens: Tokens;
}

function RelTypeRow({ relType, properties, tokens }: RelTypeRowProps) {
  const [expanded, setExpanded] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [hovered, setHovered] = useState(false);

  const onClick = (e: React.MouseEvent): void => {
    runInsert(relTypeMatch(relType), relType, modeFromEvent(e));
  };

  return (
    <Box>
      <Group
        gap={2}
        wrap="nowrap"
        onContextMenu={(e) => {
          e.preventDefault();
          setMenuOpen(true);
        }}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        style={{
          padding: "2px 6px",
          borderRadius: tokens.radius.sm,
          background: hovered || menuOpen ? tokens.bg.overlay : "transparent",
        }}
      >
        <ActionIcon
          variant="transparent"
          size="xs"
          color="gray"
          onClick={() => setExpanded((x) => !x)}
          aria-label={expanded ? "Collapse" : "Expand"}
        >
          {expanded ? (
            <IconChevronDown size={12} />
          ) : (
            <IconChevronRight size={12} />
          )}
        </ActionIcon>
        <Tooltip
          label="Click to insert MATCH query · Shift = append · Alt = new tab"
          openDelay={500}
          withArrow
        >
          <UnstyledButton
            onClick={onClick}
            style={{
              flex: 1,
              minWidth: 0,
              padding: "2px 4px",
              borderRadius: tokens.radius.sm,
            }}
          >
            <Group gap={6} wrap="nowrap" style={{ minWidth: 0 }}>
              <IconArrowRight
                size={12}
                color={tokens.category.relType}
                stroke={2}
                style={{ flexShrink: 0 }}
              />
              <Text
                size="sm"
                fw={500}
                c={tokens.category.relType}
                truncate
                title={relType}
              >
                {relType}
              </Text>
            </Group>
          </UnstyledButton>
        </Tooltip>
        <RelTypeRowMenu
          relType={relType}
          properties={properties}
          opened={menuOpen}
          onOpenChange={setMenuOpen}
          visible={hovered || menuOpen}
        />
      </Group>
      <Collapse in={expanded}>
        <Stack gap={0} pl={32} pr={4} py={2}>
          {properties.length === 0 ? (
            <Text size="xs" c={tokens.fg.subtle}>
              (no properties)
            </Text>
          ) : (
            properties.map((key) => (
              <PropertyOnLabelRow
                key={key}
                label={relType}
                kind="rel"
                property={key}
                tokens={tokens}
              />
            ))
          )}
        </Stack>
      </Collapse>
    </Box>
  );
}

interface RelTypeRowMenuProps {
  relType: string;
  properties: readonly string[];
  opened: boolean;
  onOpenChange: (v: boolean) => void;
  visible: boolean;
}

function RelTypeRowMenu({
  relType,
  properties,
  opened,
  onOpenChange,
  visible,
}: RelTypeRowMenuProps) {
  return (
    <Menu
      opened={opened}
      onChange={onOpenChange}
      position="bottom-end"
      shadow="md"
      width={220}
      withinPortal
    >
      <Menu.Target>
        <ActionIcon
          variant="subtle"
          size="xs"
          color="gray"
          aria-label={`Actions for ${relType}`}
          style={{
            opacity: visible ? 1 : 0,
            transition: "opacity 120ms",
          }}
        >
          <IconDots size={12} />
        </ActionIcon>
      </Menu.Target>
      <Menu.Dropdown>
        <Menu.Label>{relType}</Menu.Label>
        <Menu.Item
          leftSection={<IconPlayerPlay size={14} />}
          onClick={() => runInsert(relTypeMatch(relType), relType, "smart")}
        >
          Match all (LIMIT 25)
        </Menu.Item>
        <Menu.Item
          leftSection={<IconRoute size={14} />}
          onClick={() =>
            runInsert(
              relTypeEndpoints(relType),
              `${relType} endpoints`,
              "smart",
            )
          }
        >
          Show endpoints
        </Menu.Item>
        <Menu.Item
          leftSection={<IconSum size={14} />}
          onClick={() =>
            runInsert(relTypeCount(relType), `count(${relType})`, "smart")
          }
        >
          Count
        </Menu.Item>
        {properties.length > 0 ? (
          <Menu
            position="left-start"
            offset={4}
            shadow="md"
            width={200}
            withinPortal
            trigger="hover"
            closeOnItemClick={false}
          >
            <Menu.Target>
              <Menu.Item
                leftSection={<IconFilter size={14} />}
                rightSection={<IconChevronRight size={12} />}
              >
                Distinct values…
              </Menu.Item>
            </Menu.Target>
            <Menu.Dropdown>
              {properties.map((p) => (
                <Menu.Item
                  key={p}
                  leftSection={<IconKey size={12} />}
                  onClick={() =>
                    runInsert(
                      relTypeDistinctProperty(relType, p),
                      `${relType}.${p}`,
                      "smart",
                    )
                  }
                >
                  {p}
                </Menu.Item>
              ))}
            </Menu.Dropdown>
          </Menu>
        ) : null}
        <Menu.Divider />
        <Menu.Item
          leftSection={<IconCopy size={14} />}
          onClick={() => copyToClipboard(relType, "Relationship type")}
        >
          Copy name
        </Menu.Item>
      </Menu.Dropdown>
    </Menu>
  );
}

// ---------------------------------------------------------------------------
// Property rows
// ---------------------------------------------------------------------------

interface PropertyOnLabelRowProps {
  label: string;
  kind: "label" | "rel";
  property: string;
  tokens: Tokens;
}

/**
 * Property row rendered under an expanded label / rel-type. Clicking
 * projects the property; right-click offers ordering and distinct-
 * values variants without needing to bounce back to the parent menu.
 */
function PropertyOnLabelRow({
  label,
  kind,
  property,
  tokens,
}: PropertyOnLabelRowProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [hovered, setHovered] = useState(false);

  const projection = (): string =>
    kind === "label"
      ? labelMatch(label, { property })
      : relTypeProjection(label, property);

  const distinct = (): string =>
    kind === "label"
      ? labelDistinctProperty(label, property)
      : relTypeDistinctProperty(label, property);

  const niceName = `${label}.${property}`;

  return (
    <Menu
      opened={menuOpen}
      onChange={setMenuOpen}
      position="bottom-end"
      shadow="md"
      width={200}
      withinPortal
    >
      <Menu.Target>
        <UnstyledButton
          onClick={(e) => runInsert(projection(), niceName, modeFromEvent(e))}
          onContextMenu={(e) => {
            e.preventDefault();
            setMenuOpen(true);
          }}
          onMouseEnter={() => setHovered(true)}
          onMouseLeave={() => setHovered(false)}
          style={{
            padding: "2px 6px",
            borderRadius: tokens.radius.sm,
            background: hovered ? tokens.bg.overlay : "transparent",
          }}
        >
          <Group gap={6} wrap="nowrap" style={{ minWidth: 0 }}>
            <IconKey size={10} color={tokens.fg.subtle} stroke={2} />
            <Text size="xs" c={tokens.fg.muted} truncate title={property}>
              {property}
            </Text>
          </Group>
        </UnstyledButton>
      </Menu.Target>
      <Menu.Dropdown>
        <Menu.Label>{niceName}</Menu.Label>
        <Menu.Item
          leftSection={<IconPlayerPlay size={14} />}
          onClick={() => runInsert(projection(), niceName, "smart")}
        >
          Project ({kind === "label" ? "n" : "r"}.{property})
        </Menu.Item>
        <Menu.Item
          leftSection={<IconFilter size={14} />}
          onClick={() => runInsert(distinct(), `distinct ${niceName}`, "smart")}
        >
          Distinct values
        </Menu.Item>
        <Menu.Item
          leftSection={<IconSortAscending size={14} />}
          onClick={() => {
            const snippet =
              kind === "label"
                ? `${labelMatch(label, { property })}\n// ORDER BY ${property}`
                : `${projection()}\n// ORDER BY ${property}`;
            runInsert(snippet, niceName, "smart");
          }}
        >
          Ordered projection
        </Menu.Item>
        <Menu.Divider />
        <Menu.Item
          leftSection={<IconCopy size={14} />}
          onClick={() => copyToClipboard(property, "Property")}
        >
          Copy name
        </Menu.Item>
      </Menu.Dropdown>
    </Menu>
  );
}

interface PropertyFlatRowProps {
  property: string;
  tokens: Tokens;
}

/**
 * Property row in the flat "Property keys" list — there is no parent
 * label context here, so the default click runs a distinct-values
 * query across every node that carries the key.
 */
function PropertyFlatRow({ property, tokens }: PropertyFlatRowProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [hovered, setHovered] = useState(false);

  return (
    <Menu
      opened={menuOpen}
      onChange={setMenuOpen}
      position="bottom-end"
      shadow="md"
      width={200}
      withinPortal
    >
      <Menu.Target>
        <UnstyledButton
          onClick={(e) =>
            runInsert(
              propertyDistinctAny(property),
              `distinct ${property}`,
              modeFromEvent(e),
            )
          }
          onContextMenu={(e) => {
            e.preventDefault();
            setMenuOpen(true);
          }}
          onMouseEnter={() => setHovered(true)}
          onMouseLeave={() => setHovered(false)}
          style={{
            padding: "4px 8px",
            borderRadius: tokens.radius.sm,
            color: tokens.fg.primary,
            background: hovered ? tokens.bg.overlay : "transparent",
          }}
        >
          <Group gap={6} wrap="nowrap" style={{ minWidth: 0 }}>
            <IconKey size={12} color={tokens.fg.subtle} stroke={2} />
            <Text size="sm" c={tokens.fg.primary} truncate title={property}>
              {property}
            </Text>
          </Group>
        </UnstyledButton>
      </Menu.Target>
      <Menu.Dropdown>
        <Menu.Label>{property}</Menu.Label>
        <Menu.Item
          leftSection={<IconFilter size={14} />}
          onClick={() =>
            runInsert(
              propertyDistinctAny(property),
              `distinct ${property}`,
              "smart",
            )
          }
        >
          Distinct values across all nodes
        </Menu.Item>
        <Menu.Divider />
        <Menu.Item
          leftSection={<IconCopy size={14} />}
          onClick={() => copyToClipboard(property, "Property")}
        >
          Copy name
        </Menu.Item>
      </Menu.Dropdown>
    </Menu>
  );
}
