"use client";

/**
 * `NewSnapshotDialog` — Mantine modal for naming a new snapshot, with an
 * optional passphrase that seals the snapshot body with ChaCha20-Poly1305
 * (Argon2-derived key). When enabled, the same passphrase is required to
 * load the snapshot later.
 *
 * Invoked via `openNewSnapshotDialog({ defaultName, onCreate })` from
 * the Snapshots sidebar panel (both the "New" button and the import
 * flow, which passes the picked file's name as `defaultName`).
 */

import { useState } from "react";
import {
  Alert,
  Button,
  Checkbox,
  Group,
  PasswordInput,
  Stack,
  Text,
  TextInput,
} from "@mantine/core";
import { modals } from "@mantine/modals";
import { IconLock, IconShieldLock } from "@tabler/icons-react";

import type { SnapshotProtection } from "@/lib/actions/snapshotActions";

interface NewSnapshotDialogProps {
  modalId: string;
  defaultName: string;
  /** When true (default) the encryption toggle is shown. Import flows that
   * receive an already-sealed `.lorasnap` should hide it — re-encrypting
   * an encrypted blob isn't supported. */
  allowEncryption: boolean;
  onCreate: (
    name: string,
    protection?: SnapshotProtection,
  ) => void | Promise<void>;
}

function NewSnapshotDialog({
  modalId,
  defaultName,
  allowEncryption,
  onCreate,
}: NewSnapshotDialogProps) {
  const [name, setName] = useState(defaultName);
  const [protect, setProtect] = useState(false);
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [submitting, setSubmitting] = useState(false);

  const trimmedName = name.trim();
  const nameValid = trimmedName.length > 0;

  // Passphrase requirements are intentionally light here — Argon2 makes
  // short passwords merely slow to crack rather than insecure, and the
  // playground is a local-only tool. A 4-char minimum catches typos
  // without nagging.
  const passwordValid = !protect || password.length >= 4;
  const confirmValid = !protect || password === confirm;
  const valid = nameValid && passwordValid && confirmValid;

  const handleSubmit = (): void => {
    if (!valid || submitting) return;
    setSubmitting(true);
    const protection: SnapshotProtection | undefined = protect
      ? { password }
      : undefined;
    Promise.resolve(onCreate(trimmedName, protection))
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
          error={
            !nameValid && name.length > 0 ? "Name cannot be empty" : undefined
          }
        />

        {allowEncryption ? (
          <>
            <Checkbox
              label={
                <Group gap={6} wrap="nowrap">
                  <IconLock size={14} />
                  <Text size="sm">Protect with a passphrase</Text>
                </Group>
              }
              checked={protect}
              onChange={(e) => {
                setProtect(e.currentTarget.checked);
                if (!e.currentTarget.checked) {
                  setPassword("");
                  setConfirm("");
                }
              }}
            />

            {protect ? (
              <Stack gap="xs">
                <PasswordInput
                  label="Passphrase"
                  placeholder="At least 4 characters"
                  value={password}
                  onChange={(e) => {
                    setPassword(e.currentTarget.value);
                  }}
                  error={
                    !passwordValid && password.length > 0
                      ? "Passphrase must be at least 4 characters"
                      : undefined
                  }
                  autoComplete="new-password"
                />
                <PasswordInput
                  label="Confirm passphrase"
                  value={confirm}
                  onChange={(e) => {
                    setConfirm(e.currentTarget.value);
                  }}
                  error={
                    !confirmValid && confirm.length > 0
                      ? "Passphrases do not match"
                      : undefined
                  }
                  autoComplete="new-password"
                />
                <Alert
                  icon={<IconShieldLock size={14} />}
                  color="yellow"
                  variant="light"
                  p="xs"
                >
                  <Text size="xs">
                    Loading this snapshot will require the same passphrase. It
                    is not recoverable — keep a copy somewhere safe.
                  </Text>
                </Alert>
              </Stack>
            ) : null}
          </>
        ) : null}

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
  /** Hide the passphrase controls — used by the import flow because an
   * already-sealed `.lorasnap` cannot be re-encrypted client-side. */
  allowEncryption?: boolean;
  onCreate: (
    name: string,
    protection?: SnapshotProtection,
  ) => void | Promise<void>;
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
        allowEncryption={opts.allowEncryption ?? true}
        onCreate={opts.onCreate}
      />
    ),
  });
}
