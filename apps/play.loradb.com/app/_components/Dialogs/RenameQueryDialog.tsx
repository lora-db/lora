"use client";

/**
 * `RenameQueryDialog` — Mantine modal for renaming an existing saved
 * query. The input prefills with `currentName` and is auto-selected so
 * the user can type a replacement immediately.
 */

import { useState } from "react";
import { Button, Group, Stack, TextInput } from "@mantine/core";
import { modals } from "@mantine/modals";

interface RenameQueryDialogProps {
  modalId: string;
  currentName: string;
  onRename: (name: string) => void | Promise<void>;
}

function RenameQueryDialog({
  modalId,
  currentName,
  onRename,
}: RenameQueryDialogProps) {
  const [name, setName] = useState(currentName);
  const [submitting, setSubmitting] = useState(false);
  const trimmed = name.trim();
  const valid = trimmed.length > 0;

  const handleSubmit = (): void => {
    if (!valid || submitting) return;
    if (trimmed === currentName) {
      modals.close(modalId);
      return;
    }
    setSubmitting(true);
    Promise.resolve(onRename(trimmed))
      .then(() => {
        modals.close(modalId);
      })
      .catch(() => {
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
            color="blue"
            disabled={!valid || submitting}
            loading={submitting}
          >
            Rename
          </Button>
        </Group>
      </Stack>
    </form>
  );
}

/** Opens the rename-query modal. */
export function openRenameQueryDialog(opts: {
  currentName: string;
  onRename: (name: string) => void | Promise<void>;
}): void {
  const id = "loradb-rename-query-dialog";
  modals.open({
    modalId: id,
    title: "Rename query",
    centered: true,
    children: (
      <RenameQueryDialog
        modalId={id}
        currentName={opts.currentName}
        onRename={opts.onRename}
      />
    ),
  });
}
