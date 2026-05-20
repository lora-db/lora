"use client";

/**
 * Confirm dialog for `DROP INDEX` / `DROP CONSTRAINT`. Constraints
 * carry an extra "type the name" guard because losing one can
 * silently allow bad data; indexes are recreatable so the guard is
 * just a single click.
 *
 * Owned indexes refuse to drop here — the only correct action is
 * dropping the owning constraint, which the dialog offers as a
 * secondary CTA.
 */

import { useState } from "react";
import { Alert, Button, Group, Stack, Text, TextInput } from "@mantine/core";
import { modals } from "@mantine/modals";
import { IconAlertTriangle } from "@tabler/icons-react";

import { dropConstraint, dropIndex } from "@/lib/actions/schemaDesignActions";
import {
  buildDropConstraintDDL,
  buildDropIndexDDL,
} from "@/lib/schemaDesign/ddl";
import type { ConstraintDef, IndexDef } from "@/lib/schemaDesign/types";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

import { DDLPreview } from "./DDLPreview";

interface ConfirmDropIndexProps {
  modalId: string;
  index: IndexDef;
}

function ConfirmDropIndex({ modalId, index }: ConfirmDropIndexProps) {
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    setBusy(true);
    const ok = await dropIndex(index.name);
    setBusy(false);
    if (ok) modals.close(modalId);
  };

  return (
    <Stack gap="sm">
      <Text size="sm">
        Drop index <b>{index.name}</b>? Queries that relied on it will fall back
        to a full scan until it&apos;s recreated.
      </Text>
      <DDLPreview
        ddl={buildDropIndexDDL(index.name, true)}
        caption="Will run"
      />
      <Group justify="flex-end" gap="xs">
        <Button
          variant="default"
          size="xs"
          disabled={busy}
          onClick={() => modals.close(modalId)}
        >
          Cancel
        </Button>
        <Button
          color="red"
          size="xs"
          loading={busy}
          onClick={() => void submit()}
        >
          Drop index
        </Button>
      </Group>
    </Stack>
  );
}

interface ConfirmDropConstraintProps {
  modalId: string;
  constraint: ConstraintDef;
}

function ConfirmDropConstraint({
  modalId,
  constraint,
}: ConfirmDropConstraintProps) {
  const { tokens } = usePlaygroundTheme();
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const matches = typed === constraint.name;

  const submit = async () => {
    if (!matches) return;
    setBusy(true);
    const ok = await dropConstraint(constraint.name);
    setBusy(false);
    if (ok) modals.close(modalId);
  };

  const protection = (() => {
    switch (constraint.kind) {
      case "UNIQUE":
        return "duplicate values on the property";
      case "NODE_KEY":
      case "RELATIONSHIP_KEY":
        return "duplicate composite keys";
      case "NOT_NULL":
        return "missing values for the property";
      case "PROPERTY_TYPE":
        return "mismatched value types for the property";
    }
  })();

  return (
    <Stack gap="sm">
      <Alert
        icon={<IconAlertTriangle size={14} />}
        color="yellow"
        variant="light"
      >
        Dropping this constraint will allow {protection} to land in the graph
        again.
        {constraint.ownedIndex
          ? ` It will also remove the implicit ${constraint.ownedIndex} index.`
          : ""}
      </Alert>
      <Text size="sm" c={tokens.fg.muted}>
        Type <b>{constraint.name}</b> to confirm.
      </Text>
      <TextInput
        value={typed}
        onChange={(e) => setTyped(e.currentTarget.value)}
        autoFocus
        aria-label="Type the constraint name to confirm"
        placeholder={constraint.name}
        error={typed.length > 0 && !matches ? "Names don't match" : undefined}
      />
      <DDLPreview
        ddl={buildDropConstraintDDL(constraint.name, true)}
        caption="Will run"
      />
      <Group justify="flex-end" gap="xs">
        <Button
          variant="default"
          size="xs"
          disabled={busy}
          onClick={() => modals.close(modalId)}
        >
          Cancel
        </Button>
        <Button
          color="red"
          size="xs"
          loading={busy}
          disabled={!matches}
          onClick={() => void submit()}
        >
          Drop constraint
        </Button>
      </Group>
    </Stack>
  );
}

/**
 * Open a confirm dialog for dropping the given index. Owned indexes
 * cannot be dropped this way — the caller should route to
 * {@link openConfirmDropConstraint} for the owning constraint
 * instead.
 */
export function openConfirmDropIndex(index: IndexDef): void {
  if (index.owned) {
    // Caller bug — guarded by the UI but fall through to a clear toast.
    return;
  }
  const id = `loradb-confirm-drop-index-${index.name}`;
  modals.open({
    modalId: id,
    centered: true,
    title: "Drop index?",
    children: <ConfirmDropIndex modalId={id} index={index} />,
  });
}

/** Open a confirm dialog for dropping the given constraint. */
export function openConfirmDropConstraint(constraint: ConstraintDef): void {
  const id = `loradb-confirm-drop-constraint-${constraint.name}`;
  modals.open({
    modalId: id,
    centered: true,
    title: "Drop constraint?",
    children: <ConfirmDropConstraint modalId={id} constraint={constraint} />,
  });
}
