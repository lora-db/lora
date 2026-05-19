"use client";

/**
 * `NewSnapshotDialog` — Mantine modal for naming a new snapshot.
 * Invoked via `openNewSnapshotDialog({ defaultName, onCreate })` from
 * the Snapshots sidebar panel (both the "New" button and the import
 * flow, which passes the picked file's name as `defaultName`).
 */

import { useState } from "react";
import { Button, Group, Stack, TextInput } from "@mantine/core";
import { modals } from "@mantine/modals";

interface NewSnapshotDialogProps {
  modalId: string;
  defaultName: string;
  onCreate: (name: string) => void | Promise<void>;
}

function NewSnapshotDialog({
  modalId,
  defaultName,
  onCreate,
}: NewSnapshotDialogProps) {
  const [name, setName] = useState(defaultName);
  const [submitting, setSubmitting] = useState(false);
  const trimmed = name.trim();
  const valid = trimmed.length > 0;

  const handleSubmit = (): void => {
    if (!valid || submitting) return;
    setSubmitting(true);
    Promise.resolve(onCreate(trimmed))
      .then(() => {
        modals.close(modalId);
      })
      .catch(() => {
        // Caller surfaces error notifications; we just unlock the form.
        setSubmitting(false);
      });
  };

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        handleSubmit();
      }}
    >
      <Stack gap="sm">
        <TextInput
          label="Name"
          placeholder="My snapshot"
          value={name}
          onChange={(e) => {
            setName(e.currentTarget.value);
          }}
          onFocus={(e) => {
            e.currentTarget.select();
          }}
          data-autofocus
          required
          error={!valid && name.length > 0 ? "Name cannot be empty" : undefined}
        />
        <Group justify="flex-end" gap="xs" mt="xs">
          <Button
            type="button"
            variant="default"
            size="xs"
            onClick={() => {
              modals.close(modalId);
            }}
          >
            Cancel
          </Button>
          <Button
            type="submit"
            size="xs"
            color="green"
            disabled={!valid || submitting}
            loading={submitting}
          >
            Create
          </Button>
        </Group>
      </Stack>
    </form>
  );
}

/** Opens the new-snapshot modal. The caller owns the persistence call. */
export function openNewSnapshotDialog(opts: {
  defaultName?: string;
  onCreate: (name: string) => void | Promise<void>;
}): void {
  const id = "loradb-new-snapshot-dialog";
  modals.open({
    modalId: id,
    title: "New snapshot",
    centered: true,
    children: (
      <NewSnapshotDialog
        modalId={id}
        defaultName={opts.defaultName ?? ""}
        onCreate={opts.onCreate}
      />
    ),
  });
}
