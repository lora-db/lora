"use client";

/**
 * `SchemaDesignPanel` — sidebar view over the cached index and
 * constraint catalogs plus the heuristic recommendation feed. Two
 * action buttons up top open the wizards as modals.
 *
 * Each row carries a kebab menu (Edit / Copy DDL / Open in editor /
 * Drop) and expands inline to show full name, property chips, and
 * cross-links between an index and the constraint that owns it.
 */

import { useEffect, useMemo, useState } from "react";
import {
  ActionIcon,
  Anchor,
  Badge,
  Button,
  Center,
  Collapse,
  Divider,
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
import { formatDistanceToNowStrict } from "date-fns";
import {
  IconAlertTriangle,
  IconArrowUpRight,
  IconBolt,
  IconChevronDown,
  IconChevronRight,
  IconDots,
  IconEdit,
  IconKey,
  IconPlus,
  IconRefresh,
  IconSearch,
  IconShare,
  IconTrash,
  IconX,
} from "@tabler/icons-react";

import { useStore } from "@/lib/state/store";
import { refreshSchemaDesign } from "@/lib/actions/schemaDesignActions";
import { openTabInCell } from "@/lib/actions/tabActions";
import {
  buildCreateConstraintDDL,
  buildCreateIndexDDL,
  constraintDefToDraft,
  indexDefToDraft,
} from "@/lib/schemaDesign/ddl";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import type {
  ConstraintDef,
  ConstraintKind,
  IndexDef,
  IndexKind,
} from "@/lib/schemaDesign/types";

import {
  openConfirmDropConstraint,
  openConfirmDropIndex,
} from "../SchemaDesigner/ConfirmDrop";
import { Recommendations } from "../SchemaDesigner/Recommendations";

const CONSTRAINT_KIND_LABEL: Record<ConstraintKind, string> = {
  UNIQUE: "UNIQUE",
  NODE_KEY: "NODE KEY",
  RELATIONSHIP_KEY: "REL KEY",
  NOT_NULL: "NOT NULL",
  PROPERTY_TYPE: "TYPE",
};

const INDEX_KIND_COLOR: Record<IndexKind, string> = {
  RANGE: "blue",
  TEXT: "teal",
  POINT: "violet",
  LOOKUP: "gray",
  FULLTEXT: "indigo",
  VECTOR: "grape",
};

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

export function SchemaDesignPanel() {
  const { tokens } = usePlaygroundTheme();
  const indexes = useStore((s) => s.indexes);
  const constraints = useStore((s) => s.constraints);
  const refreshing = useStore((s) => s.refreshing);
  const lastFetchedAt = useStore((s) => s.lastFetchedAt);
  const openNewIndex = useStore((s) => s.openNewIndexWizard);
  const openNewConstraint = useStore((s) => s.openNewConstraintWizard);

  useEffect(() => {
    if (indexes === null) void refreshSchemaDesign();
  }, [indexes]);

  const freshness = useFreshness(lastFetchedAt);
  const indexCount = indexes?.length ?? 0;
  const constraintCount = constraints?.length ?? 0;
  // Tracked below — declared up here so the section-header badges can
  // show "N of M" while a filter is active.
  const populating =
    indexes?.filter((idx) => idx.state === "populating").length ?? 0;

  // Index-by-name lookups so an owned index can navigate to its owner
  // constraint (and the reverse, for the constraint row's +idx badge).
  const constraintByName = useMemo(() => {
    const m = new Map<string, ConstraintDef>();
    for (const c of constraints ?? []) m.set(c.name, c);
    return m;
  }, [constraints]);
  const indexByName = useMemo(() => {
    const m = new Map<string, IndexDef>();
    for (const i of indexes ?? []) m.set(i.name, i);
    return m;
  }, [indexes]);

  const [filter, setFilter] = useState("");
  const filteredConstraints = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (q.length === 0) return constraints ?? [];
    return (constraints ?? []).filter(
      (c) =>
        c.name.toLowerCase().includes(q) ||
        c.label.toLowerCase().includes(q) ||
        c.properties.some((p) => p.toLowerCase().includes(q)),
    );
  }, [constraints, filter]);
  const filteredIndexes = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (q.length === 0) return indexes ?? [];
    return (indexes ?? []).filter(
      (i) =>
        i.name.toLowerCase().includes(q) ||
        (i.labelsOrTypes[0] ?? "").toLowerCase().includes(q) ||
        i.properties.some((p) => p.toLowerCase().includes(q)),
    );
  }, [indexes, filter]);

  // Row expansion is tracked at the panel level so cross-links can
  // open the target row.
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const toggleExpanded = (name: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  const revealRow = (name: string) => {
    setExpanded((prev) => {
      if (prev.has(name)) return prev;
      const next = new Set(prev);
      next.add(name);
      return next;
    });
    // Defer the scroll until the Collapse has had a chance to mount.
    requestAnimationFrame(() => {
      const el = document.querySelector<HTMLElement>(
        `[data-schema-row="${CSS.escape(name)}"]`,
      );
      el?.scrollIntoView({ block: "nearest", behavior: "smooth" });
    });
  };

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
            Schema design
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
              onClick={() => void refreshSchemaDesign()}
              aria-label="Refresh schema design"
              disabled={refreshing}
            >
              <IconRefresh size={14} />
            </ActionIcon>
          </Tooltip>
        </Group>
      </Group>

      <Stack gap={6} px={12} py={10}>
        <TextInput
          size="xs"
          placeholder="Filter by name, label, or property"
          value={filter}
          onChange={(e) => setFilter(e.currentTarget.value)}
          leftSection={<IconSearch size={12} />}
          rightSection={
            filter.length > 0 ? (
              <ActionIcon
                size="xs"
                variant="subtle"
                color="gray"
                onClick={() => setFilter("")}
                aria-label="Clear filter"
              >
                <IconX size={12} />
              </ActionIcon>
            ) : null
          }
          aria-label="Filter schema catalog"
        />
        {populating > 0 ? (
          <Group gap={4} wrap="nowrap">
            <IconAlertTriangle size={12} color={tokens.accent.warning} />
            <Text size="xs" c={tokens.accent.warning}>
              {populating} index{populating === 1 ? "" : "es"} still populating
            </Text>
          </Group>
        ) : null}
      </Stack>

      <Divider />

      <ScrollArea style={{ flex: 1, minHeight: 0 }}>
        <Stack gap={8} px={12} py={10}>
          <Recommendations />
        </Stack>

        <Divider />

        <Stack gap={4} px={8} pb={8}>
          <SectionHeader
            title="Constraints"
            count={constraintCount}
            filteredCount={
              filter.length > 0 ? filteredConstraints.length : null
            }
            tokens={tokens}
            icon={<IconKey size={12} />}
            onAdd={constraintCount > 0 ? () => openNewConstraint() : undefined}
            addLabel="New constraint"
            addColor="grape"
          />
          {constraints === null ? (
            <Hint>Loading…</Hint>
          ) : constraints.length === 0 ? (
            <EmptyStateCTA
              cta="Add your first constraint"
              onClick={() => openNewConstraint()}
            />
          ) : filteredConstraints.length === 0 ? (
            <Hint>No constraints match “{filter}”.</Hint>
          ) : (
            filteredConstraints.map((c) => (
              <ConstraintRow
                key={c.name}
                def={c}
                ownedIndex={
                  c.ownedIndex ? indexByName.get(c.ownedIndex) : undefined
                }
                expanded={expanded.has(c.name)}
                onToggle={() => toggleExpanded(c.name)}
                onRevealIndex={revealRow}
              />
            ))
          )}

          <SectionHeader
            title="Indexes"
            count={indexCount}
            filteredCount={filter.length > 0 ? filteredIndexes.length : null}
            tokens={tokens}
            icon={<IconBolt size={12} />}
            onAdd={indexCount > 0 ? () => openNewIndex() : undefined}
            addLabel="New index"
            addColor="blue"
          />
          {indexes === null ? (
            <Hint>Loading…</Hint>
          ) : indexes.length === 0 ? (
            <EmptyStateCTA
              cta="Add your first index"
              onClick={() => openNewIndex()}
            />
          ) : filteredIndexes.length === 0 ? (
            <Hint>No indexes match “{filter}”.</Hint>
          ) : (
            filteredIndexes.map((idx) => (
              <IndexRow
                key={idx.name}
                def={idx}
                ownerConstraint={
                  idx.ownerConstraint
                    ? constraintByName.get(idx.ownerConstraint)
                    : undefined
                }
                expanded={expanded.has(idx.name)}
                onToggle={() => toggleExpanded(idx.name)}
                onRevealConstraint={revealRow}
              />
            ))
          )}
        </Stack>
      </ScrollArea>
    </Stack>
  );
}

// ---------------------------------------------------------------------------
// Subcomponents
// ---------------------------------------------------------------------------

function Hint({ children }: { children: React.ReactNode }) {
  const { tokens } = usePlaygroundTheme();
  return (
    <Center py={6}>
      <Text size="xs" c={tokens.fg.subtle}>
        {children}
      </Text>
    </Center>
  );
}

function EmptyStateCTA({ cta, onClick }: { cta: string; onClick: () => void }) {
  return (
    <Stack gap={2} align="center" py={8}>
      <Button
        size="compact-xs"
        variant="subtle"
        leftSection={<IconPlus size={12} />}
        onClick={onClick}
      >
        {cta}
      </Button>
    </Stack>
  );
}

interface SectionHeaderProps {
  icon: React.ReactNode;
  title: string;
  count: number;
  /** When non-null, the badge shows "N of M" to surface filter narrowing. */
  filteredCount?: number | null;
  tokens: ReturnType<typeof usePlaygroundTheme>["tokens"];
  onAdd?: () => void;
  addLabel?: string;
  /** Mantine color token for the add ActionIcon — matches the section's badge palette. */
  addColor?: string;
}

function SectionHeader({
  icon,
  title,
  count,
  filteredCount = null,
  tokens,
  onAdd,
  addLabel,
  addColor = "gray",
}: SectionHeaderProps) {
  return (
    <Group gap={6} wrap="nowrap" px={4} pt={8} pb={2}>
      <span style={{ display: "inline-flex", color: tokens.fg.muted }}>
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
        {filteredCount === null ? count : `${filteredCount} of ${count}`}
      </Badge>
      {onAdd ? (
        <Tooltip label={addLabel ?? `Add ${title.toLowerCase()}`} withArrow>
          <ActionIcon
            size="sm"
            variant="light"
            color={addColor}
            onClick={onAdd}
            aria-label={addLabel ?? `Add ${title.toLowerCase()}`}
            style={{ marginLeft: "auto" }}
          >
            <IconPlus size={12} />
          </ActionIcon>
        </Tooltip>
      ) : null}
    </Group>
  );
}

function copyDdl(ddl: string): void {
  if (typeof window === "undefined" || !navigator.clipboard) return;
  void navigator.clipboard.writeText(ddl).then(
    () => {
      notifications.show({
        color: "green",
        title: "Copied",
        message: "DDL copied to clipboard.",
        autoClose: 1500,
      });
    },
    (err: unknown) => {
      notifications.show({
        color: "red",
        title: "Copy failed",
        message: err instanceof Error ? err.message : String(err),
      });
    },
  );
}

function ConstraintRow({
  def,
  ownedIndex,
  expanded,
  onToggle,
  onRevealIndex,
}: {
  def: ConstraintDef;
  ownedIndex?: IndexDef;
  expanded: boolean;
  onToggle: () => void;
  onRevealIndex: (name: string) => void;
}) {
  const { tokens } = usePlaygroundTheme();
  const openEdit = useStore((s) => s.openEditConstraintWizard);
  const schemaSummary = `${def.label}(${def.properties.join(", ")})`;
  const ddl = useMemo(
    () => buildCreateConstraintDDL(constraintDefToDraft(def)),
    [def],
  );
  return (
    <Stack gap={0} data-schema-row={def.name}>
      <Group
        gap={6}
        wrap="nowrap"
        px={6}
        py={4}
        style={{ borderRadius: tokens.radius.sm }}
      >
        <UnstyledButton
          onClick={onToggle}
          aria-expanded={expanded}
          aria-label={`Toggle constraint ${def.name}`}
          style={{
            display: "inline-flex",
            alignItems: "center",
            color: tokens.fg.muted,
          }}
        >
          {expanded ? (
            <IconChevronDown size={12} />
          ) : (
            <IconChevronRight size={12} />
          )}
        </UnstyledButton>
        <Badge size="xs" variant="light" color="grape" radius="sm">
          {CONSTRAINT_KIND_LABEL[def.kind]}
        </Badge>
        <UnstyledButton
          onClick={onToggle}
          style={{
            flex: 1,
            minWidth: 0,
            textAlign: "left",
          }}
        >
          <Text
            size="xs"
            c={tokens.fg.primary}
            truncate
            title={`${def.name}: ${schemaSummary}`}
          >
            {schemaSummary}
          </Text>
        </UnstyledButton>
        {ownedIndex ? (
          <Tooltip label={`Backed by index ${ownedIndex.name}`} withArrow>
            <Badge size="xs" variant="outline" color="gray" radius="sm">
              +idx
            </Badge>
          </Tooltip>
        ) : null}
        <ShareDdlButton
          ddl={ddl}
          ariaLabel={`Copy DDL for constraint ${def.name}`}
        />
        <RowMenu
          ariaLabel={`Actions for constraint ${def.name}`}
          ddl={ddl}
          defaultTabName={`Constraint ${def.name}`}
          onEdit={() => openEdit(def)}
          onDrop={() => openConfirmDropConstraint(def)}
        />
      </Group>
      <Collapse in={expanded}>
        <Stack gap={4} px={28} pb={6}>
          <DetailLine label="Name" value={def.name} tokens={tokens} />
          <DetailLine
            label={def.entity === "NODE" ? "Label" : "Rel type"}
            value={def.label}
            tokens={tokens}
          />
          <DetailChips
            label="Properties"
            values={def.properties}
            tokens={tokens}
          />
          {def.propertyType ? (
            <DetailLine label="Type" value={def.propertyType} tokens={tokens} />
          ) : null}
          {ownedIndex ? (
            <Group gap={4}>
              <Text size="xs" c={tokens.fg.muted}>
                Backed by:
              </Text>
              <Anchor
                size="xs"
                onClick={() => onRevealIndex(ownedIndex.name)}
                style={{ cursor: "pointer" }}
              >
                {ownedIndex.name}
              </Anchor>
            </Group>
          ) : null}
        </Stack>
      </Collapse>
    </Stack>
  );
}

function IndexRow({
  def,
  ownerConstraint,
  expanded,
  onToggle,
  onRevealConstraint,
}: {
  def: IndexDef;
  ownerConstraint?: ConstraintDef;
  expanded: boolean;
  onToggle: () => void;
  onRevealConstraint: (name: string) => void;
}) {
  const { tokens } = usePlaygroundTheme();
  const openEdit = useStore((s) => s.openEditIndexWizard);
  const openEditConstraint = useStore((s) => s.openEditConstraintWizard);
  const schemaSummary =
    def.kind === "LOOKUP"
      ? def.entity === "NODE"
        ? "labels(n)"
        : "type(r)"
      : `${def.labelsOrTypes[0] ?? "*"}(${def.properties.join(", ")})`;
  const color = INDEX_KIND_COLOR[def.kind];
  const ddl = useMemo(() => buildCreateIndexDDL(indexDefToDraft(def)), [def]);
  return (
    <Stack gap={0} data-schema-row={def.name}>
      <Group
        gap={6}
        wrap="nowrap"
        px={6}
        py={4}
        style={{ borderRadius: tokens.radius.sm }}
      >
        <UnstyledButton
          onClick={onToggle}
          aria-expanded={expanded}
          aria-label={`Toggle index ${def.name}`}
          style={{
            display: "inline-flex",
            alignItems: "center",
            color: tokens.fg.muted,
          }}
        >
          {expanded ? (
            <IconChevronDown size={12} />
          ) : (
            <IconChevronRight size={12} />
          )}
        </UnstyledButton>
        <Badge size="xs" variant="light" color={color} radius="sm">
          {def.kind}
        </Badge>
        <UnstyledButton
          onClick={onToggle}
          style={{
            flex: 1,
            minWidth: 0,
            textAlign: "left",
          }}
        >
          <Text
            size="xs"
            c={tokens.fg.primary}
            truncate
            title={`${def.name}: ${schemaSummary}`}
          >
            {schemaSummary}
          </Text>
        </UnstyledButton>
        {def.owned ? (
          <Tooltip label={`Owned by ${def.ownerConstraint}`} withArrow>
            <Badge size="xs" variant="outline" color="gray" radius="sm">
              owned
            </Badge>
          </Tooltip>
        ) : null}
        {def.state === "populating" ? (
          <Tooltip
            label={`Populating ${Math.round(def.populationPercent)}%`}
            withArrow
          >
            <Loader size={10} />
          </Tooltip>
        ) : null}
        <ShareDdlButton
          ddl={ddl}
          ariaLabel={`Copy DDL for index ${def.name}`}
        />
        <RowMenu
          ariaLabel={`Actions for index ${def.name}`}
          ddl={ddl}
          defaultTabName={`Index ${def.name}`}
          onEdit={
            def.owned
              ? ownerConstraint
                ? () => openEditConstraint(ownerConstraint)
                : null
              : () => openEdit(def)
          }
          editLabel={def.owned ? "Edit owning constraint…" : "Edit index…"}
          editDisabledReason={
            def.owned && !ownerConstraint
              ? "The owning constraint isn't in the catalog yet — refresh."
              : null
          }
          onDrop={
            def.owned
              ? ownerConstraint
                ? () => openConfirmDropConstraint(ownerConstraint)
                : null
              : () => openConfirmDropIndex(def)
          }
          dropLabel={def.owned ? "Drop owning constraint…" : "Drop index…"}
          dropDisabledReason={
            def.owned && !ownerConstraint
              ? "The owning constraint isn't in the catalog yet — refresh."
              : null
          }
        />
      </Group>
      <Collapse in={expanded}>
        <Stack gap={4} px={28} pb={6}>
          <DetailLine label="Name" value={def.name} tokens={tokens} />
          {def.kind !== "LOOKUP" ? (
            <>
              <DetailLine
                label={def.entity === "NODE" ? "Label" : "Rel type"}
                value={def.labelsOrTypes[0] ?? "—"}
                tokens={tokens}
              />
              <DetailChips
                label="Properties"
                values={def.properties}
                tokens={tokens}
              />
            </>
          ) : (
            <DetailLine
              label="Scope"
              value={
                def.entity === "NODE"
                  ? "All labels (labels(n))"
                  : "All relationship types (type(r))"
              }
              tokens={tokens}
            />
          )}
          <Group gap={4}>
            <Text size="xs" c={tokens.fg.muted}>
              State:
            </Text>
            <Text size="xs" c={tokens.fg.primary}>
              {def.state === "populating"
                ? `Populating ${Math.round(def.populationPercent)}%`
                : "Online"}
            </Text>
          </Group>
          {def.owned && ownerConstraint ? (
            <Group gap={4}>
              <Text size="xs" c={tokens.fg.muted}>
                Owned by:
              </Text>
              <Anchor
                size="xs"
                onClick={() => onRevealConstraint(ownerConstraint.name)}
                style={{ cursor: "pointer" }}
              >
                {ownerConstraint.name}
              </Anchor>
            </Group>
          ) : def.owned && def.ownerConstraint ? (
            <Group gap={4}>
              <Text size="xs" c={tokens.fg.muted}>
                Owned by:
              </Text>
              <Text size="xs" c={tokens.fg.primary}>
                {def.ownerConstraint}
              </Text>
            </Group>
          ) : null}
        </Stack>
      </Collapse>
    </Stack>
  );
}

function DetailLine({
  label,
  value,
  tokens,
}: {
  label: string;
  value: string;
  tokens: ReturnType<typeof usePlaygroundTheme>["tokens"];
}) {
  return (
    <Group gap={4} wrap="nowrap" align="flex-start">
      <Text size="xs" c={tokens.fg.muted} style={{ flexShrink: 0 }}>
        {label}:
      </Text>
      <Text size="xs" c={tokens.fg.primary} style={{ wordBreak: "break-word" }}>
        {value}
      </Text>
    </Group>
  );
}

function DetailChips({
  label,
  values,
  tokens,
}: {
  label: string;
  values: readonly string[];
  tokens: ReturnType<typeof usePlaygroundTheme>["tokens"];
}) {
  if (values.length === 0) return null;
  return (
    <Group gap={4} wrap="wrap" align="flex-start">
      <Text size="xs" c={tokens.fg.muted}>
        {label}:
      </Text>
      {values.map((v) => (
        <Badge key={v} size="xs" variant="light" radius="sm" color="gray">
          {v}
        </Badge>
      ))}
    </Group>
  );
}

function ShareDdlButton({
  ddl,
  ariaLabel,
}: {
  ddl: string;
  ariaLabel: string;
}) {
  return (
    <Tooltip label="Copy DDL" withArrow>
      <ActionIcon
        variant="subtle"
        size="sm"
        color="gray"
        aria-label={ariaLabel}
        onClick={() => copyDdl(ddl)}
      >
        <IconShare size={12} />
      </ActionIcon>
    </Tooltip>
  );
}

interface RowMenuProps {
  ariaLabel: string;
  ddl: string;
  defaultTabName: string;
  /** Null disables Edit (e.g. owned index without resolvable owner). */
  onEdit: (() => void) | null;
  editLabel?: string;
  editDisabledReason?: string | null;
  /** Null disables the destructive action. */
  onDrop: (() => void) | null;
  dropLabel?: string;
  dropDisabledReason?: string | null;
}

function RowMenu({
  ariaLabel,
  ddl,
  defaultTabName,
  onEdit,
  editLabel = "Edit…",
  editDisabledReason = null,
  onDrop,
  dropLabel = "Drop…",
  dropDisabledReason = null,
}: RowMenuProps) {
  return (
    <Menu position="bottom-end" shadow="md" width={200} withinPortal>
      <Menu.Target>
        <ActionIcon
          variant="subtle"
          size="sm"
          color="gray"
          aria-label={ariaLabel}
        >
          <IconDots size={12} />
        </ActionIcon>
      </Menu.Target>
      <Menu.Dropdown>
        <Tooltip
          label={editDisabledReason ?? ""}
          disabled={!editDisabledReason}
          withArrow
        >
          <Menu.Item
            leftSection={<IconEdit size={14} />}
            disabled={onEdit === null}
            onClick={() => onEdit?.()}
          >
            {editLabel}
          </Menu.Item>
        </Tooltip>
        <Menu.Item
          leftSection={<IconArrowUpRight size={14} />}
          onClick={() => {
            openTabInCell({ name: defaultTabName, body: ddl });
            notifications.show({
              color: "blue",
              title: "Opened in a new tab",
              message: "Tweak the DDL before running if you like.",
              autoClose: 2000,
            });
          }}
        >
          Open in editor
        </Menu.Item>
        <Menu.Divider />
        <Tooltip
          label={dropDisabledReason ?? ""}
          disabled={!dropDisabledReason}
          withArrow
        >
          <Menu.Item
            color="red"
            leftSection={<IconTrash size={14} />}
            disabled={onDrop === null}
            onClick={() => onDrop?.()}
          >
            {dropLabel}
          </Menu.Item>
        </Tooltip>
      </Menu.Dropdown>
    </Menu>
  );
}
