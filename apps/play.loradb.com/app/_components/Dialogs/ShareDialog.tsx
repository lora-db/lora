"use client";

/**
 * `ShareDialog` — Mantine modal that surfaces a copyable share URL
 * for a query body. The URL is built via `buildShareLink` so the
 * encoding scheme (lz-string `#q=` hash) lives in exactly one place.
 *
 * Invoked from `app/_components/TopBar.tsx` (Phase 4) or from the
 * `mod+shift+L` hotkey via `openShareDialog({ body })`.
 */

import { useRef, useState } from "react";
import {
  ActionIcon,
  Alert,
  Button,
  Group,
  Stack,
  Text,
  TextInput,
  Tooltip,
} from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import {
  IconAlertTriangle,
  IconCamera,
  IconCheck,
  IconCopy,
} from "@tabler/icons-react";

import { buildShareLink, copyShareLink } from "@/lib/actions/shareActions";
import { openNewSnapshotDialog } from "./NewSnapshotDialog";
import { createSnapshotFromDb } from "@/lib/actions/snapshotActions";
import { setActivity } from "@/lib/actions/uiActions";

interface ShareDialogProps {
  modalId: string;
  body: string;
  /** Optional raw JSON `$param` payload to embed alongside the body. */
  params?: string;
}

/**
 * Soft threshold for share-URL length. Stays well under the
 * conservative 2083-char IE limit and the limits enforced by common
 * chat / email clients. Beyond this we surface a warning + a snapshot
 * fallback rather than pretending the link will paste cleanly.
 */
const SAFE_URL_LENGTH = 2000;

function ShareDialog({ modalId, body, params }: ShareDialogProps) {
  const url = buildShareLink(body, params);
  const [copied, setCopied] = useState(false);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const oversized = url.length > SAFE_URL_LENGTH;
  const hasParams =
    params !== undefined && params.trim() !== "" && params.trim() !== "{}";

  const handleCopy = (): void => {
    void copyShareLink(body, params)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => {
          setCopied(false);
        }, 1500);
      })
      .catch(() => {
        // Notification already surfaced by copyShareLink.
      });
  };

  const handleSnapshotFallback = (): void => {
    modals.close(modalId);
    openNewSnapshotDialog({
      defaultName: `Shared query ${new Date().toLocaleString()}`,
      onCreate: async (name) => {
        try {
          const record = await createSnapshotFromDb(name);
          notifications.show({
            color: "green",
            title: "Snapshot saved",
            message: `Saved "${record.name}" — open the Snapshots panel to export or restore it.`,
          });
          setActivity("snapshots");
        } catch (err) {
          notifications.show({
            color: "red",
            title: "Snapshot failed",
            message: err instanceof Error ? err.message : String(err),
          });
          throw err;
        }
      },
    });
  };

  return (
    <Stack gap="sm">
      <Text size="xs" c="dimmed">
        Share this link; the recipient sees a tab pre-populated with your query.
      </Text>
      <Group gap={4} wrap="nowrap" align="flex-end">
        <TextInput
          ref={inputRef}
          style={{ flex: 1 }}
          value={url}
          readOnly
          onFocus={(e) => {
            e.currentTarget.select();
          }}
          aria-label="Share URL"
        />
        <Tooltip label={copied ? "Copied!" : "Copy link"} withArrow>
          <ActionIcon
            variant="light"
            color={copied ? "green" : "blue"}
            size="lg"
            onClick={handleCopy}
            aria-label="Copy share link"
          >
            {copied ? <IconCheck size={16} /> : <IconCopy size={16} />}
          </ActionIcon>
        </Tooltip>
      </Group>
      <Text size="xs" c="dimmed">
        {url.length.toLocaleString()} characters
        {hasParams ? " · params included" : ""}
      </Text>
      {oversized ? (
        <Alert
          variant="light"
          color="yellow"
          icon={<IconAlertTriangle size={16} />}
          title="This link is long"
        >
          <Stack gap="xs">
            <Text size="xs">
              Some chat clients and email providers truncate URLs past about{" "}
              {SAFE_URL_LENGTH.toLocaleString()} characters. If recipients see a
              broken link, save the current database as a snapshot and share the
              exported <code>.lorasnap</code> file instead.
            </Text>
            <Group gap="xs">
              <Button
                size="xs"
                variant="light"
                color="yellow"
                leftSection={<IconCamera size={14} />}
                onClick={handleSnapshotFallback}
              >
                Save as snapshot
              </Button>
            </Group>
          </Stack>
        </Alert>
      ) : null}
      <Group justify="flex-end" gap="xs" mt="xs">
        <Button
          variant="default"
          size="xs"
          onClick={() => {
            modals.close(modalId);
          }}
        >
          Close
        </Button>
        <Button size="xs" color="blue" onClick={handleCopy}>
          Copy link
        </Button>
      </Group>
    </Stack>
  );
}

/**
 * Opens the share-link modal for a query body, optionally with the
 * raw JSON params payload to embed in the URL.
 */
export function openShareDialog(opts: { body: string; params?: string }): void {
  const id = "loradb-share-dialog";
  modals.open({
    modalId: id,
    title: "Share query",
    centered: true,
    children: (
      <ShareDialog
        modalId={id}
        body={opts.body}
        {...(opts.params !== undefined && { params: opts.params })}
      />
    ),
  });
}
