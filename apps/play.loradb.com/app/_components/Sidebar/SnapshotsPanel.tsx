"use client";

/**
 * `SnapshotsPanel` — CRUD UI over the `snapshots` IDB store.
 *
 * The panel listens for the `loradb:snapshots` window event so any
 * action that mutates persistence (from anywhere in the app) triggers
 * a refresh here without prop-drilling a reload callback.
 *
 * Load is gated behind a confirm modal because it replaces the live
 * database contents — there is no undo.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ActionIcon,
  Button,
  Center,
  CloseButton,
  Group,
  Loader,
  Menu,
  ScrollArea,
  Stack,
  Text,
  TextInput,
  Tooltip,
  UnstyledButton,
} from "@mantine/core";
import { openConfirmModal } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import {
  IconCamera,
  IconDots,
  IconDownload,
  IconFileImport,
  IconLock,
  IconPlus,
  IconRefresh,
  IconRestore,
  IconSearch,
  IconTrash,
} from "@tabler/icons-react";
import { format, formatDistanceToNowStrict } from "date-fns";

import * as snapshots from "@/lib/persistence/snapshots";
import {
  SNAPSHOTS_EVENT,
  SnapshotPasswordRequiredError,
  createSnapshotFromDb,
  deleteSnapshotById,
  exportSnapshotToFile,
  importSnapshotFromFile,
  loadSnapshotById,
} from "@/lib/actions/snapshotActions";
import { formatBytes } from "@/lib/util/format";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

import { openNewSnapshotDialog } from "../Dialogs/NewSnapshotDialog";
import { openSnapshotPasswordDialog } from "../Dialogs/SnapshotPasswordDialog";

/**
 * Strip the `.lorasnap` suffix (if present) from a picked file name so
 * the import dialog defaults to a clean snapshot name.
 */
function defaultNameFromFile(file: File): string {
  const dot = file.name.lastIndexOf(".");
  return dot > 0 ? file.name.slice(0, dot) : file.name;
}

export function SnapshotsPanel() {
  const { tokens } = usePlaygroundTheme();
  const [items, setItems] = useState<snapshots.SnapshotMeta[]>([]);
  const [loading, setLoading] = useState(true);
  const [filter, setFilter] = useState("");
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (q.length === 0) return items;
    return items.filter((record) => record.name.toLowerCase().includes(q));
  }, [items, filter]);

  const latest = useMemo(
    () =>
      items.reduce<snapshots.SnapshotMeta | null>((winner, item) => {
        if (!winner) return item;
        return item.createdAt > winner.createdAt ? item : winner;
      }, null),
    [items],
  );

  const refresh = useCallback((): void => {
    snapshots
      .list()
      .then((rows) => {
        setItems(rows);
        setLoading(false);
      })
      .catch((err: unknown) => {
        setLoading(false);
        notifications.show({
          color: "red",
          title: "Failed to load snapshots",
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
    window.addEventListener(SNAPSHOTS_EVENT, handler);
    return () => {
      window.removeEventListener(SNAPSHOTS_EVENT, handler);
    };
  }, [refresh]);

  const handleNew = useCallback((): void => {
    openNewSnapshotDialog({
      defaultName: `Snapshot ${new Date().toLocaleString()}`,
      onCreate: async (name, protection) => {
        try {
          const record = await createSnapshotFromDb(name, protection);
          notifications.show({
            color: "green",
            title: "Snapshot created",
            message: `Saved "${record.name}" (${formatBytes(record.sizeBytes)})${
              protection ? " — passphrase-protected" : ""
            }.`,
          });
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
  }, []);

  const handleImportClick = useCallback((): void => {
    fileInputRef.current?.click();
  }, []);

  const handleFilePicked = useCallback(
    (event: React.ChangeEvent<HTMLInputElement>): void => {
      const file = event.currentTarget.files?.[0];
      // Reset so picking the same file twice still fires `change`.
      event.currentTarget.value = "";
      if (!file) return;
      openNewSnapshotDialog({
        defaultName: defaultNameFromFile(file),
        // The file already carries its own envelope (encrypted or not).
        // Re-encrypting client-side would require decoding + re-encoding,
        // which we don't expose.
        allowEncryption: false,
        onCreate: async (name) => {
          try {
            const record = await importSnapshotFromFile(file, name);
            notifications.show({
              color: "green",
              title: "Snapshot imported",
              message: `Imported "${record.name}" (${formatBytes(
                record.sizeBytes,
              )})${record.header?.encrypted ? " — encrypted" : ""}.`,
            });
          } catch (err) {
            notifications.show({
              color: "red",
              title: "Import failed",
              message: err instanceof Error ? err.message : String(err),
            });
            throw err;
          }
        },
      });
    },
    [],
  );

  const handleLoad = useCallback(
    (record: snapshots.SnapshotMeta): void => {
      const finishLoad = (): void => {
        notifications.show({
          color: "green",
          title: "Snapshot loaded",
          message: `Restored "${record.name}".`,
        });
      };
      const reportFailure = (err: unknown): void => {
        notifications.show({
          color: "red",
          title: "Load failed",
          message: err instanceof Error ? err.message : String(err),
        });
      };

      const promptForPassword = (keyId: string | null): void => {
        openSnapshotPasswordDialog({
          snapshotName: record.name,
          keyId,
          onSubmit: async (password) => {
            try {
              await loadSnapshotById(record.id, { password });
              finishLoad();
            } catch (err) {
              // Surface the failure but keep the dialog open so the user
              // can retype the passphrase. Throwing keeps the modal mounted.
              reportFailure(err);
              throw err;
            }
          },
        });
      };

      openConfirmModal({
        title: "Load snapshot?",
        centered: true,
        children: (
          <Text size="sm" c={tokens.fg.muted}>
            Loading <strong>{record.name}</strong> will replace the current
            database contents. This cannot be undone.
            {record.header?.encrypted ? (
              <> You will be asked for the passphrase it was sealed with.</>
            ) : null}
          </Text>
        ),
        labels: { confirm: "Load", cancel: "Cancel" },
        confirmProps: { color: "blue", "data-autofocus": "true" },
        onConfirm: () => {
          loadSnapshotById(record.id)
            .then(finishLoad)
            .catch((err: unknown) => {
              if (err instanceof SnapshotPasswordRequiredError) {
                promptForPassword(err.keyId);
                return;
              }
              reportFailure(err);
            });
        },
      });
    },
    [tokens.fg.muted],
  );

  const handleExport = useCallback((record: snapshots.SnapshotMeta): void => {
    exportSnapshotToFile(record.id).catch((err: unknown) => {
      notifications.show({
        color: "red",
        title: "Export failed",
        message: err instanceof Error ? err.message : String(err),
      });
    });
  }, []);

  const handleDelete = useCallback(
    (record: snapshots.SnapshotMeta): void => {
      openConfirmModal({
        title: "Delete snapshot?",
        centered: true,
        children: (
          <Text size="sm" c={tokens.fg.muted}>
            Permanently delete <strong>{record.name}</strong>? This cannot be
            undone.
          </Text>
        ),
        labels: { confirm: "Delete", cancel: "Cancel" },
        confirmProps: { color: "red", "data-autofocus": "true" },
        onConfirm: () => {
          deleteSnapshotById(record.id)
            .then(() => {
              notifications.show({
                color: "green",
                title: "Deleted",
                message: `"${record.name}" was deleted.`,
              });
            })
            .catch((err: unknown) => {
              notifications.show({
                color: "red",
                title: "Delete failed",
                message: err instanceof Error ? err.message : String(err),
              });
            });
        },
      });
    },
    [tokens.fg.muted],
  );

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
        <Stack gap={0}>
          <Text
            size="xs"
            fw={600}
            c={tokens.fg.muted}
            style={{ letterSpacing: 1, textTransform: "uppercase" }}
          >
            Snapshots
          </Text>
          {latest ? (
            <Text
              size="xs"
              c={tokens.fg.subtle}
              component="time"
              dateTime={new Date(latest.createdAt).toISOString()}
              style={{ fontSize: 10 }}
            >
              latest{" "}
              {formatDistanceToNowStrict(latest.createdAt, { addSuffix: true })}
            </Text>
          ) : null}
        </Stack>
        <Group gap={4} wrap="nowrap">
          <Tooltip label="New snapshot from current DB" withArrow>
            <ActionIcon
              variant="subtle"
              size="sm"
              color="gray"
              onClick={handleNew}
              aria-label="New snapshot"
            >
              <IconPlus size={14} />
            </ActionIcon>
          </Tooltip>
          <Tooltip label="Import .lorasnap file" withArrow>
            <ActionIcon
              variant="subtle"
              size="sm"
              color="gray"
              onClick={handleImportClick}
              aria-label="Import snapshot"
            >
              <IconFileImport size={14} />
            </ActionIcon>
          </Tooltip>
          <Tooltip label="Refresh" withArrow>
            <ActionIcon
              variant="subtle"
              size="sm"
              color="gray"
              onClick={refresh}
              aria-label="Refresh snapshots"
            >
              <IconRefresh size={14} />
            </ActionIcon>
          </Tooltip>
        </Group>
      </Group>

      <input
        ref={fileInputRef}
        type="file"
        accept=".lorasnap,application/octet-stream"
        hidden
        onChange={handleFilePicked}
      />

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
            placeholder="Filter snapshots"
            aria-label="Filter snapshots"
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
              <IconCamera size={28} color={tokens.fg.subtle} stroke={1.5} />
              <Text size="xs" c={tokens.fg.subtle} ta="center">
                No snapshots yet — capture the current database or import a{" "}
                <code>.lorasnap</code> file.
              </Text>
              <Group gap={6}>
                <Button
                  size="xs"
                  variant="light"
                  onClick={handleNew}
                  leftSection={<IconPlus size={14} />}
                >
                  New snapshot
                </Button>
                <Button
                  size="xs"
                  variant="default"
                  onClick={handleImportClick}
                  leftSection={<IconFileImport size={14} />}
                >
                  Import
                </Button>
              </Group>
            </Stack>
          </Center>
        ) : filtered.length === 0 ? (
          <Center p="md">
            <Text size="xs" c={tokens.fg.subtle} ta="center">
              No snapshots match &ldquo;{filter}&rdquo;
            </Text>
          </Center>
        ) : (
          <Stack gap={0} p={4}>
            {filtered.map((record) => (
              <SnapshotRow
                key={record.id}
                record={record}
                onLoad={() => {
                  handleLoad(record);
                }}
                onExport={() => {
                  handleExport(record);
                }}
                onDelete={() => {
                  handleDelete(record);
                }}
              />
            ))}
          </Stack>
        )}
      </ScrollArea>
    </Stack>
  );
}

interface SnapshotRowProps {
  record: snapshots.SnapshotMeta;
  onLoad: () => void;
  onExport: () => void;
  onDelete: () => void;
}

function formatCompressionLabel(
  compression: snapshots.SnapshotHeader["compression"],
): string {
  if (compression.format === "gzip") {
    return `gzip · lvl ${compression.level}`;
  }
  return "uncompressed";
}

function formatStatTooltip(
  record: snapshots.SnapshotMeta,
  header: snapshots.SnapshotHeader | undefined,
): string {
  const lines: string[] = [];
  lines.push(format(record.createdAt, "PPpp"));
  if (header) {
    lines.push(
      `${header.nodeCount.toLocaleString()} nodes · ${header.relationshipCount.toLocaleString()} relationships`,
    );
    lines.push(`Body: ${formatCompressionLabel(header.compression)}`);
    lines.push(
      header.encrypted
        ? `Encrypted${header.keyId ? ` (key: ${header.keyId})` : ""}`
        : "Not encrypted",
    );
    lines.push(`Format v${header.formatVersion}`);
    if (header.walLsn !== null) lines.push(`WAL fence: ${header.walLsn}`);
  }
  lines.push(`${record.sizeBytes.toLocaleString()} bytes on disk`);
  return lines.join("\n");
}

function SnapshotRow({ record, onLoad, onExport, onDelete }: SnapshotRowProps) {
  const { tokens } = usePlaygroundTheme();
  const [menuOpen, setMenuOpen] = useState(false);
  const header = record.header;

  return (
    <Group
      gap={0}
      wrap="nowrap"
      align="stretch"
      style={{ borderRadius: tokens.radius.sm, position: "relative" }}
    >
      <UnstyledButton
        onClick={onLoad}
        onContextMenu={(e) => {
          e.preventDefault();
          setMenuOpen(true);
        }}
        style={{
          flex: 1,
          minWidth: 0,
          padding: "8px 10px",
          color: tokens.fg.primary,
          borderRadius: tokens.radius.sm,
        }}
        title={formatStatTooltip(record, header)}
      >
        <Stack gap={2}>
          <Group gap={6} wrap="nowrap" align="center" style={{ minWidth: 0 }}>
            {header?.encrypted ? (
              <IconLock
                size={12}
                stroke={1.8}
                color={tokens.accent.warning}
                style={{ flexShrink: 0 }}
                aria-label="Encrypted"
              />
            ) : null}
            <Text
              size="sm"
              fw={500}
              c={tokens.fg.primary}
              truncate
              title={record.name}
              style={{ flex: 1, minWidth: 0 }}
            >
              {record.name}
            </Text>
            <Text
              size="xs"
              c={tokens.fg.subtle}
              component="time"
              dateTime={new Date(record.createdAt).toISOString()}
              style={{ flexShrink: 0 }}
            >
              {formatDistanceToNowStrict(record.createdAt, {
                addSuffix: true,
              })}
            </Text>
          </Group>
          <Group
            gap={6}
            wrap="nowrap"
            style={{ minWidth: 0, fontVariantNumeric: "tabular-nums" }}
          >
            {header ? (
              <>
                <Text size="xs" c={tokens.category.node} fw={600}>
                  {header.nodeCount.toLocaleString()}{" "}
                  <Text span size="xs" c={tokens.fg.subtle} fw={500}>
                    nodes
                  </Text>
                </Text>
                <Text size="xs" c={tokens.fg.subtle}>
                  ·
                </Text>
                <Text size="xs" c={tokens.category.relationship} fw={600}>
                  {header.relationshipCount.toLocaleString()}{" "}
                  <Text span size="xs" c={tokens.fg.subtle} fw={500}>
                    rels
                  </Text>
                </Text>
                <Text size="xs" c={tokens.fg.subtle}>
                  ·
                </Text>
              </>
            ) : null}
            <Text size="xs" c={tokens.fg.muted}>
              {formatBytes(record.sizeBytes)}
            </Text>
          </Group>
        </Stack>
      </UnstyledButton>
      <div style={{ display: "flex", alignItems: "center", paddingRight: 4 }}>
        <Menu
          opened={menuOpen}
          onChange={setMenuOpen}
          position="bottom-end"
          shadow="md"
          width={180}
        >
          <Menu.Target>
            <ActionIcon
              variant="subtle"
              size="sm"
              color="gray"
              aria-label={`Actions for ${record.name}`}
            >
              <IconDots size={14} />
            </ActionIcon>
          </Menu.Target>
          <Menu.Dropdown>
            <Menu.Item leftSection={<IconRestore size={14} />} onClick={onLoad}>
              Load
            </Menu.Item>
            <Menu.Item
              leftSection={<IconDownload size={14} />}
              onClick={onExport}
            >
              Export
            </Menu.Item>
            <Menu.Divider />
            <Menu.Item
              color="red"
              leftSection={<IconTrash size={14} />}
              onClick={onDelete}
            >
              Delete
            </Menu.Item>
          </Menu.Dropdown>
        </Menu>
      </div>
    </Group>
  );
}
