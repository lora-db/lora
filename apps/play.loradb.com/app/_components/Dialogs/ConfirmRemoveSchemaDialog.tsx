"use client";

/**
 * `ConfirmRemoveSchemaDialog` — destructive guard for removing every
 * node of a label or every relationship of a rel-type from the
 * `SchemaBrowserPanel` kebab. Mirrors the layout of
 * `ConfirmDeleteDialog` so the playground has one consistent
 * "preview-the-cypher" pattern.
 *
 * Labels DETACH-delete so attached relationships cascade in the same
 * transaction; rel-type removal only drops the edges. The alert text
 * adapts to the kind so the user can predict the blast radius.
 */

import { useState } from "react";
import { Alert, Button, Group, Stack, Text } from "@mantine/core";
import { modals } from "@mantine/modals";
import { IconAlertTriangle } from "@tabler/icons-react";

import {
  deleteAllOfLabel,
  deleteAllOfRelType,
} from "@/lib/actions/schemaActions";
import { labelDeleteAll, relTypeDeleteAll } from "@/lib/snippets/cypher";

import { DDLPreview } from "../SchemaDesigner/DDLPreview";

type Kind = "label" | "relType";

interface ConfirmRemoveSchemaDialogProps {
  modalId: string;
  kind: Kind;
  name: string;
  /**
   * Total nodes (when `kind === "label"`) about to be removed, used to
   * sharpen the wording. Undefined when the introspection counts are
   * unknown — the dialog still works, the headline just doesn't carry
   * a number.
   */
  count?: number;
}

function ConfirmRemoveSchemaDialog({
  modalId,
  kind,
  name,
  count,
}: ConfirmRemoveSchemaDialogProps) {
  const [busy, setBusy] = useState(false);

  const ddl = kind === "label" ? labelDeleteAll(name) : relTypeDeleteAll(name);

  const headline =
    kind === "label"
      ? count !== undefined
        ? `Remove ${count} node${count === 1 ? "" : "s"} with label `
        : `Remove every node with label `
      : `Remove every relationship of type `;

  const cascadeNote =
    kind === "label"
      ? "Every relationship attached to one of these nodes is removed in the same transaction (DETACH DELETE)."
      : "Endpoint nodes are left in place — only the relationships of this type are removed.";

  const submit = async (): Promise<void> => {
    setBusy(true);
    const outcome =
      kind === "label"
        ? await deleteAllOfLabel(name)
        : await deleteAllOfRelType(name);
    setBusy(false);
    if (outcome.ok) modals.close(modalId);
  };

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        if (busy) return;
        void submit();
      }}
    >
      <Stack gap="sm">
        <Text size="sm">
          {headline}
          <b>{name}</b>?
        </Text>

        <Alert
          icon={<IconAlertTriangle size={14} />}
          color="yellow"
          variant="light"
        >
          {cascadeNote} The schema entry disappears once no rows reference it.
        </Alert>

        <DDLPreview ddl={ddl} caption="Will run" />

        <Text size="xs" c="dimmed">
          This runs against the in-memory LoraDB session. Save a snapshot
          afterwards to persist the deletion across reloads.
        </Text>

        <Group justify="flex-end" gap="xs" mt="xs">
          <Button
            type="button"
            variant="default"
            size="xs"
            disabled={busy}
            onClick={() => modals.close(modalId)}
          >
            Cancel
          </Button>
          <Button
            type="submit"
            color="red"
            size="xs"
            loading={busy}
            data-autofocus
          >
            Remove {kind === "label" ? "nodes" : "relationships"}
          </Button>
        </Group>
      </Stack>
    </form>
  );
}

/** Open the confirm dialog for removing every node of a label. */
export function openConfirmRemoveLabel(label: string, count?: number): void {
  const id = `loradb-confirm-remove-label-${label}`;
  modals.open({
    modalId: id,
    centered: true,
    title: `Remove label "${label}"?`,
    children: (
      <ConfirmRemoveSchemaDialog
        modalId={id}
        kind="label"
        name={label}
        count={count}
      />
    ),
  });
}

/** Open the confirm dialog for removing every relationship of a rel-type. */
export function openConfirmRemoveRelType(relType: string): void {
  const id = `loradb-confirm-remove-reltype-${relType}`;
  modals.open({
    modalId: id,
    centered: true,
    title: `Remove relationship type "${relType}"?`,
    children: (
      <ConfirmRemoveSchemaDialog modalId={id} kind="relType" name={relType} />
    ),
  });
}
