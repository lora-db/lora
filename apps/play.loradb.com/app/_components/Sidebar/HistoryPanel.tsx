"use client";

/**
 * `HistoryPanel` — newest-first view of the per-tab query run log.
 *
 * Listens for the `loradb:history` window event so any action that
 * appends or clears persistence (most notably `runActiveTab`) triggers
 * a refresh here without prop-drilling a reload callback.
 *
 * Each row's first line shows the run timestamp + the first physical
 * line of the body (60-char cap, mono); the second line shows an
 * ok/error pill, the duration, and either the row count or the head
 * of the error message.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ActionIcon,
  Badge,
  Center,
  CloseButton,
  Group,
  Loader,
  ScrollArea,
  Stack,
  Text,
  TextInput,
  Tooltip,
  UnstyledButton,
} from "@mantine/core";
import { openConfirmModal } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { IconHistory, IconRefresh, IconSearch, IconTrash } from "@tabler/icons-react";

import type { HistoryEntry } from "@/lib/persistence/history";
import {
  HISTORY_EVENT,
  clearHistory,
  listHistory,
  openHistoryEntryInNewTab,
} from "@/lib/actions/historyActions";
import { formatCount, formatMs } from "@/lib/util/format";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import { hexA } from "@/lib/theme/util";
import type { Tokens } from "@/lib/theme/tokens";

/** Same thresholds as `ResultPane` so the per-run ms in a history row
 *  reads the same colour as the live result strip. */
function speedColor(ms: number, tokens: Tokens): string {
  if (ms < 50) return tokens.accent.success;
  if (ms < 200) return tokens.accent.warning;
  return tokens.accent.danger;
}

const SNIPPET_CAP = 60;

/** "12:34 PM" — locale-stable wall clock for the row header. */
function formatTime(ts: number): string {
  return new Date(ts).toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
}

/** First non-empty line of `body`, truncated to {@link SNIPPET_CAP} characters. */
function snippet(body: string): string {
  for (const line of body.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (trimmed.length === 0) continue;
    if (trimmed.length <= SNIPPET_CAP) return trimmed;
    return `${trimmed.slice(0, SNIPPET_CAP - 1)}…`;
  }
  return "(empty)";
}

/** Short error head — bounded so the second-line pill stays readable. */
function errorHead(message: string): string {
  const first = message.split(/\r?\n/)[0]?.trim() ?? "";
  if (first.length <= 60) return first;
  return `${first.slice(0, 59)}…`;
}

export function HistoryPanel() {
  const { tokens } = usePlaygroundTheme();
  const [items, setItems] = useState<HistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [filter, setFilter] = useState("");

  // Filter on body text and (for failed runs) the error message so users
  // can find "that delete query I ran an hour ago" or "the one that
  // blew up with a SyntaxError".
  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (q.length === 0) return items;
    return items.filter((entry) => {
      if (entry.body.toLowerCase().includes(q)) return true;
      if (entry.errorMessage && entry.errorMessage.toLowerCase().includes(q)) {
        return true;
      }
      return false;
    });
  }, [items, filter]);

  const refresh = useCallback((): void => {
    listHistory(200)
      .then((rows) => {
        setItems(rows);
        setLoading(false);
      })
      .catch((err: unknown) => {
        setLoading(false);
        notifications.show({
          color: "red",
          title: "Failed to load history",
          message: err instanceof Error ? err.message : String(err),
        });
      });
  }, []);

  useEffect(() => {
    refresh();
    if (typeof window === "undefined") return;
    const handler = (): void => {
      refresh();
    };
    window.addEventListener(HISTORY_EVENT, handler);
    return () => {
      window.removeEventListener(HISTORY_EVENT, handler);
    };
  }, [refresh]);

  const handleClear = useCallback((): void => {
    if (items.length === 0) return;
    openConfirmModal({
      title: "Clear history?",
      centered: true,
      children: (
        <Text size="sm" c={tokens.fg.muted}>
          Permanently delete all {formatCount(items.length)} history entries?
          This cannot be undone.
        </Text>
      ),
      labels: { confirm: "Clear", cancel: "Cancel" },
      confirmProps: { color: "red", "data-autofocus": "true" },
      onConfirm: () => {
        clearHistory()
          .then(() => {
            notifications.show({
              color: "green",
              title: "History cleared",
              message: "All entries removed.",
            });
          })
          .catch((err: unknown) => {
            notifications.show({
              color: "red",
              title: "Clear failed",
              message: err instanceof Error ? err.message : String(err),
            });
          });
      },
    });
  }, [items.length, tokens.fg.muted]);

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
        <Text
          size="xs"
          fw={600}
          c={tokens.fg.muted}
          style={{ letterSpacing: 1, textTransform: "uppercase" }}
        >
          History
        </Text>
        <Group gap={4} wrap="nowrap">
          <Tooltip label="Clear history" withArrow>
            <ActionIcon
              variant="subtle"
              size="sm"
              color="gray"
              onClick={handleClear}
              disabled={items.length === 0}
              aria-label="Clear history"
            >
              <IconTrash size={14} />
            </ActionIcon>
          </Tooltip>
          <Tooltip label="Refresh" withArrow>
            <ActionIcon
              variant="subtle"
              size="sm"
              color="gray"
              onClick={refresh}
              aria-label="Refresh history"
            >
              <IconRefresh size={14} />
            </ActionIcon>
          </Tooltip>
        </Group>
      </Group>

      {items.length > 0 && (
        <Group
          px={8}
          py={6}
          style={{ borderBottom: `1px solid ${tokens.border.subtle}` }}
        >
          <TextInput
            size="xs"
            value={filter}
            onChange={(e) => setFilter(e.currentTarget.value)}
            placeholder="Filter history"
            aria-label="Filter history"
            leftSection={<IconSearch size={12} />}
            rightSection={
              filter.length > 0 ? (
                <CloseButton
                  size="xs"
                  onClick={() => setFilter("")}
                  aria-label="Clear filter"
                />
              ) : null
            }
            style={{ flex: 1 }}
          />
        </Group>
      )}

      <ScrollArea style={{ flex: 1, minHeight: 0 }}>
        {loading ? (
          <Center p="md">
            <Loader size="sm" />
          </Center>
        ) : items.length === 0 ? (
          <Center p="md">
            <Stack gap="xs" align="center">
              <IconHistory size={28} color={tokens.fg.subtle} stroke={1.5} />
              <Text size="xs" c={tokens.fg.subtle} ta="center">
                No runs yet — press <strong>⌘↵</strong> in the editor to record
                your first one.
              </Text>
            </Stack>
          </Center>
        ) : filtered.length === 0 ? (
          <Center p="md">
            <Text size="xs" c={tokens.fg.subtle} ta="center">
              No history matches &ldquo;{filter}&rdquo;
            </Text>
          </Center>
        ) : (
          <Stack gap={0} p={4}>
            {filtered.map((entry) => (
              <HistoryRow
                key={entry.id}
                entry={entry}
                onOpen={() => {
                  openHistoryEntryInNewTab(entry.id).catch((err: unknown) => {
                    notifications.show({
                      color: "red",
                      title: "Open failed",
                      message:
                        err instanceof Error ? err.message : String(err),
                    });
                  });
                }}
              />
            ))}
          </Stack>
        )}
      </ScrollArea>
    </Stack>
  );
}

interface HistoryRowProps {
  entry: HistoryEntry;
  onOpen: () => void;
}

function HistoryRow({ entry, onOpen }: HistoryRowProps) {
  const { tokens } = usePlaygroundTheme();
  const time = formatTime(entry.startedAt);
  const ms = formatMs(entry.ms);
  const head = entry.ok
    ? `${formatCount(entry.rowCount)} ${entry.rowCount === 1 ? "row" : "rows"}`
    : errorHead(entry.errorMessage ?? "error");

  return (
    <UnstyledButton
      onClick={onOpen}
      style={{
        padding: "6px 10px",
        borderRadius: tokens.radius.sm,
        color: tokens.fg.primary,
      }}
      title={entry.body}
    >
      <Stack gap={2}>
        <Group gap={6} wrap="nowrap" style={{ minWidth: 0 }}>
          <Text
            size="xs"
            c={tokens.fg.subtle}
            component="time"
            dateTime={new Date(entry.startedAt).toISOString()}
            style={{ flexShrink: 0 }}
          >
            {time}
          </Text>
          <Text
            size="xs"
            c={tokens.fg.primary}
            truncate
            ff="monospace"
            style={{ minWidth: 0 }}
          >
            {snippet(entry.body)}
          </Text>
        </Group>
        <Group gap={6} wrap="nowrap" style={{ minWidth: 0 }}>
          <Badge
            size="xs"
            variant="light"
            radius="sm"
            style={{
              color: entry.ok ? tokens.accent.success : tokens.accent.danger,
              background: hexA(
                entry.ok ? tokens.accent.success : tokens.accent.danger,
                0.1,
              ),
              borderColor: "transparent",
            }}
          >
            {entry.ok ? "ok" : "error"}
          </Badge>
          <Text size="xs" c={speedColor(entry.ms, tokens)} fw={600}>
            {ms}
          </Text>
          <Text size="xs" c={tokens.fg.subtle}>
            ·
          </Text>
          <Text
            size="xs"
            c={entry.ok ? tokens.fg.muted : tokens.accent.danger}
            truncate
            style={{ minWidth: 0 }}
          >
            {head}
          </Text>
        </Group>
      </Stack>
    </UnstyledButton>
  );
}
