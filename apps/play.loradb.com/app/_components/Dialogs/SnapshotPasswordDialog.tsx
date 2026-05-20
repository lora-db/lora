"use client";

/**
 * `SnapshotPasswordDialog` — Mantine modal that prompts the user for the
 * passphrase needed to unseal an encrypted snapshot on load. Rendered by
 * the Snapshots sidebar when `loadSnapshotById` raises
 * `SnapshotPasswordRequiredError`.
 */

import { useState } from "react";
import { Button, Group, PasswordInput, Stack, Text } from "@mantine/core";
import { modals } from "@mantine/modals";

interface SnapshotPasswordDialogProps {
  modalId: string;
  snapshotName: string;
  keyId: string | null;
  onSubmit: (password: string) => void | Promise<void>;
}

function SnapshotPasswordDialog({
  modalId,
  snapshotName,
  keyId,
  onSubmit,
}: SnapshotPasswordDialogProps) {
  const [password, setPassword] = useState("");
  const [submitting, setSubmitting] = useState(false);

  const valid = password.length > 0;

  const handleSubmit = (): void => {
    if (!valid || submitting) return;
    setSubmitting(true);
    Promise.resolve(onSubmit(password))
      .then(() => {
        modals.close(modalId);
      })
      .catch(() => {
        // Wrong passphrase notification is surfaced by the caller — leave
        // the form open so the user can retry without re-typing the name.
        setSubmitting(false);
        setPassword("");
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
        <Text size="sm">
          <strong>{snapshotName}</strong> is encrypted. Enter the passphrase it
          was sealed with to load it.
        </Text>
        {keyId ? (
          <Text size="xs" c="dimmed">
            Key id: <code>{keyId}</code>
          </Text>
        ) : null}
        <PasswordInput
          label="Passphrase"
          value={password}
          onChange={(e) => {
            setPassword(e.currentTarget.value);
          }}
          data-autofocus
          required
          autoComplete="current-password"
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
            Load
          </Button>
        </Group>
      </Stack>
    </form>
  );
}

/** Open the passphrase prompt. The caller owns the load call. */
export function openSnapshotPasswordDialog(opts: {
  snapshotName: string;
  keyId: string | null;
  onSubmit: (password: string) => void | Promise<void>;
}): void {
  const id = "loradb-snapshot-password-dialog";
  modals.open({
    modalId: id,
    title: "Unlock snapshot",
    centered: true,
    children: (
      <SnapshotPasswordDialog
        modalId={id}
        snapshotName={opts.snapshotName}
        keyId={opts.keyId}
        onSubmit={opts.onSubmit}
      />
    ),
  });
}
