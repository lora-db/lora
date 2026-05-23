"use client";

/**
 * Editable companion to the inspector. Rendered inside `NodeCard` when
 * the user enters edit mode. Pure presentation — all save / cancel /
 * dirty-state handling lives in the host card; this component owns the
 * row-form state and surfaces it through callbacks.
 *
 * UX:
 *   - Read-only header strip echoes `id` and `labels` / `type` /
 *     endpoints so the user can't accidentally re-target the edit.
 *   - One row per editable property. Per-kind input — TextInput for
 *     strings, NumberInput for numbers, Switch for booleans, Textarea
 *     (parsed as JSON) for arrays / objects / engine shapes.
 *   - Inline error under any offending row plus a banner at the top
 *     for cross-row issues (missing required key).
 *   - "Add property" button at the bottom. Empty-state CTA when there
 *     are no rows yet.
 */

import { useMemo } from "react";
import {
  ActionIcon,
  Alert,
  Badge,
  Code,
  Group,
  NumberInput,
  Select,
  Stack,
  Switch,
  Text,
  Textarea,
  TextInput,
  Tooltip,
} from "@mantine/core";
import {
  IconAlertCircle,
  IconKey,
  IconPlus,
  IconTrash,
} from "@tabler/icons-react";

import type { InspectTarget } from "@/lib/state/slices/inspect";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

import type { EditableKind, EditRow, RowError } from "./editForm";
import { emptyRow } from "./editForm";

const KIND_OPTIONS: { value: EditableKind; label: string }[] = [
  { value: "string", label: "String" },
  { value: "integer", label: "Integer" },
  { value: "float", label: "Number" },
  { value: "boolean", label: "Boolean" },
  { value: "json", label: "JSON" },
  { value: "null", label: "Null" },
];

interface EditPanelProps {
  target: InspectTarget;
  rows: EditRow[];
  onRowsChange: (rows: EditRow[]) => void;
  rowErrors: RowError[];
  globalError: string | null;
  requiredKeys: ReadonlySet<string>;
  disabled?: boolean;
}

export function EditPanel({
  target,
  rows,
  onRowsChange,
  rowErrors,
  globalError,
  requiredKeys,
  disabled = false,
}: EditPanelProps) {
  const { tokens } = usePlaygroundTheme();
  const errorByUid = useMemo(() => {
    const map = new Map<string, string>();
    for (const e of rowErrors) map.set(e.uid, e.message);
    return map;
  }, [rowErrors]);

  const updateRow = (uid: string, patch: Partial<EditRow>): void => {
    onRowsChange(
      rows.map((row) => (row.uid === uid ? { ...row, ...patch } : row)),
    );
  };

  const removeRow = (uid: string): void => {
    onRowsChange(rows.filter((row) => row.uid !== uid));
  };

  const addRow = (): void => {
    onRowsChange([...rows, emptyRow("string")]);
  };

  return (
    <Stack gap="sm">
      <ReadOnlyHeader target={target} />
      {globalError ? (
        <Alert
          variant="light"
          color="red"
          icon={<IconAlertCircle size={14} />}
          py={6}
        >
          <Text size="xs">{globalError}</Text>
        </Alert>
      ) : null}

      {rows.length === 0 ? (
        <Stack gap={6} align="center" py="sm">
          <Text size="xs" c={tokens.fg.subtle}>
            No properties yet.
          </Text>
          <ActionIcon
            variant="light"
            color="blue"
            onClick={addRow}
            disabled={disabled}
            aria-label="Add property"
          >
            <IconPlus size={14} />
          </ActionIcon>
        </Stack>
      ) : (
        <Stack gap={8}>
          {rows.map((row) => (
            <RowEditor
              key={row.uid}
              row={row}
              required={requiredKeys.has(row.key)}
              error={errorByUid.get(row.uid) ?? null}
              disabled={disabled}
              onChange={(patch) => updateRow(row.uid, patch)}
              onRemove={() => removeRow(row.uid)}
            />
          ))}
        </Stack>
      )}

      {rows.length > 0 ? (
        <Group justify="flex-start">
          <ActionIcon
            variant="subtle"
            color="blue"
            onClick={addRow}
            disabled={disabled}
            aria-label="Add property"
          >
            <IconPlus size={14} />
          </ActionIcon>
          <Text size="xs" c={tokens.fg.subtle}>
            Add property
          </Text>
        </Group>
      ) : null}
    </Stack>
  );
}

// ---------------------------------------------------------------------------
// Read-only header
// ---------------------------------------------------------------------------

function ReadOnlyHeader({ target }: { target: InspectTarget }) {
  const { tokens } = usePlaygroundTheme();
  return (
    <Stack
      gap={4}
      style={{
        background: tokens.bg.app,
        border: `1px solid ${tokens.border.subtle}`,
        borderRadius: tokens.radius.sm,
        padding: 8,
      }}
    >
      <Text size="xs" c={tokens.fg.muted} tt="uppercase" fw={600}>
        {target.kind === "node" ? "Editing node" : "Editing relationship"}
      </Text>
      <Group gap={6} wrap="wrap">
        <Code style={{ background: "transparent", fontSize: 11 }}>
          id {String(target.id)}
        </Code>
        {target.kind === "node" ? (
          target.labels.map((l) => (
            <Badge key={l} size="xs" variant="light" color="gray">
              :{l}
            </Badge>
          ))
        ) : (
          <>
            <Badge size="xs" variant="light" color="grape">
              :{target.type || "?"}
            </Badge>
            <Code style={{ background: "transparent", fontSize: 11 }}>
              {String(target.startId)} → {String(target.endId)}
            </Code>
          </>
        )}
      </Group>
      <Text size="xs" c={tokens.fg.subtle}>
        {target.kind === "node"
          ? "Labels and id are immutable here."
          : "Type and endpoints are immutable here."}
      </Text>
    </Stack>
  );
}

// ---------------------------------------------------------------------------
// Per-row editor
// ---------------------------------------------------------------------------

interface RowEditorProps {
  row: EditRow;
  required: boolean;
  error: string | null;
  disabled: boolean;
  onChange: (patch: Partial<EditRow>) => void;
  onRemove: () => void;
}

function RowEditor({
  row,
  required,
  error,
  disabled,
  onChange,
  onRemove,
}: RowEditorProps) {
  const { tokens } = usePlaygroundTheme();
  return (
    <Stack
      gap={4}
      style={{
        padding: 6,
        background: error ? tokens.bg.overlay : "transparent",
        borderRadius: tokens.radius.sm,
        border: error
          ? `1px solid ${tokens.border.subtle}`
          : "1px solid transparent",
      }}
    >
      <Group gap={6} wrap="nowrap" align="flex-start">
        <TextInput
          size="xs"
          value={row.key}
          placeholder="key"
          onChange={(e) => onChange({ key: e.currentTarget.value })}
          disabled={disabled}
          style={{ flex: 1, minWidth: 96 }}
          leftSection={
            required ? (
              <Tooltip label="Required by a constraint" withArrow>
                <IconKey size={11} />
              </Tooltip>
            ) : undefined
          }
          rightSection={
            required ? null : (
              <Tooltip label="Remove property" withArrow>
                <ActionIcon
                  size="xs"
                  variant="subtle"
                  color="red"
                  onClick={onRemove}
                  disabled={disabled}
                  aria-label={`Remove ${row.key || "property"}`}
                >
                  <IconTrash size={11} />
                </ActionIcon>
              </Tooltip>
            )
          }
        />
        <Select
          size="xs"
          value={row.kind}
          onChange={(value) => {
            if (!value) return;
            onChange({ kind: value as EditableKind });
          }}
          data={KIND_OPTIONS}
          disabled={disabled}
          style={{ width: 96 }}
          aria-label="Property type"
          allowDeselect={false}
          comboboxProps={{ withinPortal: true }}
        />
      </Group>
      <ValueInput row={row} onChange={onChange} disabled={disabled} />
      {error ? (
        <Text size="xs" c="red">
          {error}
        </Text>
      ) : null}
    </Stack>
  );
}

function ValueInput({
  row,
  onChange,
  disabled,
}: {
  row: EditRow;
  onChange: (patch: Partial<EditRow>) => void;
  disabled: boolean;
}) {
  const { tokens } = usePlaygroundTheme();
  switch (row.kind) {
    case "null":
      return (
        <Text size="xs" c={tokens.fg.subtle} ff={tokens.font.mono}>
          null
        </Text>
      );
    case "boolean":
      return (
        <Switch
          size="xs"
          checked={row.bool}
          onChange={(e) => onChange({ bool: e.currentTarget.checked })}
          label={row.bool ? "true" : "false"}
          disabled={disabled}
        />
      );
    case "integer":
      return (
        <NumberInput
          size="xs"
          value={row.text === "" ? "" : Number(row.text)}
          onChange={(v) => onChange({ text: v === "" ? "" : String(v) })}
          disabled={disabled}
          allowDecimal={false}
          allowNegative
          hideControls
          placeholder="integer"
        />
      );
    case "float":
      return (
        <NumberInput
          size="xs"
          value={row.text === "" ? "" : Number(row.text)}
          onChange={(v) => onChange({ text: v === "" ? "" : String(v) })}
          disabled={disabled}
          decimalScale={10}
          allowNegative
          hideControls
          placeholder="number"
        />
      );
    case "string":
      return (
        <TextInput
          size="xs"
          value={row.text}
          onChange={(e) => onChange({ text: e.currentTarget.value })}
          disabled={disabled}
          placeholder="value"
        />
      );
    case "json":
      return (
        <Textarea
          size="xs"
          value={row.text}
          onChange={(e) => onChange({ text: e.currentTarget.value })}
          disabled={disabled}
          autosize
          minRows={2}
          maxRows={8}
          styles={{ input: { fontFamily: tokens.font.mono, fontSize: 11 } }}
          placeholder='e.g. ["a", "b"]'
        />
      );
  }
}
