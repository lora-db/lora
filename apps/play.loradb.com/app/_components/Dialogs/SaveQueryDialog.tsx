"use client";

/**
 * `SaveQueryDialog` — Mantine modal for naming + tagging a new saved
 * query. Invoked via `openSaveQueryDialog({ defaultName, onSave })`
 * from the Sidebar panel and from the save hotkey when the active
 * tab is not yet bound to a saved query.
 */

import { useState } from "react";
import { Button, Group, Stack, TagsInput, TextInput } from "@mantine/core";
import { modals } from "@mantine/modals";

interface SaveQueryDialogProps {
  modalId: string;
  defaultName: string;
  onSave: (name: string, tags: string[]) => void | Promise<void>;
}

function SaveQueryDialog({
  modalId,
  defaultName,
  onSave,
}: SaveQueryDialogProps) {
  const [name, setName] = useState(defaultName);
  const [tags, setTags] = useState<string[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const trimmed = name.trim();
  const valid = trimmed.length > 0;

  const handleSubmit = (): void => {
    if (!valid || submitting) return;
    setSubmitting(true);
    Promise.resolve(onSave(trimmed, tags))
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
          placeholder="My query"
          value={name}
          onChange={(e) => {
            setName(e.currentTarget.value);
          }}
          data-autofocus
          required
          error={!valid && name.length > 0 ? "Name cannot be empty" : undefined}
        />
        <TagsInput
          label="Tags"
          placeholder="Add tags (press enter)"
          value={tags}
          onChange={setTags}
          clearable
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
            Save
          </Button>
        </Group>
      </Stack>
    </form>
  );
}

/** Opens the save-query modal. The caller owns the actual persistence call. */
export function openSaveQueryDialog(opts: {
  defaultName?: string;
  onSave: (name: string, tags: string[]) => void | Promise<void>;
}): void {
  const id = "loradb-save-query-dialog";
  modals.open({
    modalId: id,
    title: "Save query",
    centered: true,
    children: (
      <SaveQueryDialog
        modalId={id}
        defaultName={opts.defaultName ?? ""}
        onSave={opts.onSave}
      />
    ),
  });
}
