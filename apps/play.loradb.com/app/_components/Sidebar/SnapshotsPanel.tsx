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

import { useCallback, useEffect, useRef, useState } from "react";
import {
  ActionIcon,
  Button,
  Center,
  Group,
  Loader,
  Menu,
  ScrollArea,
  Stack,
  Text,
  Tooltip,
  UnstyledButton,
} from "@mantine/core";
import { openConfirmModal } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import {
  IconCamera,
  IconDots,
  IconDownload,
  IconArchive,
  IconFileImport,
  IconLock,
  IconPlus,
  IconRefresh,
  IconRestore,
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
  const fileInputRef = useRef<HTMLInputElement | null>(null);

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
              <>
                {" "}
                You will be asked for the passphrase it was sealed with.
              </>
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
        <Text
          size="xs"
          fw={600}
          c={tokens.fg.muted}
          style={{ letterSpacing: 1, textTransform: "uppercase" }}
        >
          Snapshots
        </Text>
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
                No snapshots. Create one from the current database state, or
                import a <code>.lorasnap</code> file.
              </Text>
              <Group gap="xs">
                <Button
                  size="xs"
                  variant="light"
                  onClick={handleNew}
                  leftSection={<IconPlus size={14} />}
                >
                  New
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
        ) : (
          <Stack gap={0} p={4}>
            {items.map((record) => (
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
  if (header) {
    lines.push(
      `${header.nodeCount.toLocaleString()} nodes · ${header.relationshipCount.toLocaleString()} relationships`,
    );
  }
  lines.push(`${record.sizeBytes.toLocaleString()} bytes on disk`);
  if (header) {
    lines.push(`Body: ${formatCompressionLabel(header.compression)}`);
    lines.push(
      header.encrypted
        ? `Encrypted${header.keyId ? ` (key: ${header.keyId})` : ""}`
        : "Not encrypted",
    );
    lines.push(`Format v${header.formatVersion}`);
    if (header.walLsn !== null) lines.push(`WAL fence: ${header.walLsn}`);
  }
  return lines.join("\n");
}

function StatDot({ color }: { color: string }) {
  return (
    <span
      style={{
        display: "inline-block",
        width: 6,
        height: 6,
        borderRadius: "50%",
        backgroundColor: color,
        marginRight: 4,
        verticalAlign: "middle",
        flexShrink: 0,
      }}
      aria-hidden="true"
    />
  );
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
      style={{
        borderRadius: tokens.radius.sm,
        position: "relative",
      }}
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
      >
        <Stack gap={3}>
          <Group gap={6} wrap="nowrap" align="center">
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
            {header?.encrypted ? (
              <Tooltip
                label={`Encrypted${header.keyId ? ` · key ${header.keyId}` : ""}`}
                withArrow
                openDelay={300}
              >
                <IconLock
                  size={12}
                  stroke={1.8}
                  color={tokens.fg.muted}
                  aria-label="Encrypted snapshot"
                  style={{ flexShrink: 0 }}
                />
              </Tooltip>
            ) : null}
            {header && header.compression.format !== "none" ? (
              <Tooltip
                label={formatCompressionLabel(header.compression)}
                withArrow
                openDelay={300}
              >
                <IconArchive
                  size={12}
                  stroke={1.8}
                  color={tokens.fg.muted}
                  aria-label="Compressed snapshot"
                  style={{ flexShrink: 0 }}
                />
              </Tooltip>
            ) : null}
            <Tooltip
              label={format(record.createdAt, "PPpp")}
              withArrow
              openDelay={400}
            >
              <Text
                size="xs"
                c={tokens.fg.subtle}
                component="time"
                dateTime={new Date(record.createdAt).toISOString()}
                style={{ flexShrink: 0 }}
              >
                {formatDistanceToNowStrict(record.createdAt)}
              </Text>
            </Tooltip>
          </Group>
          <Tooltip
            label={formatStatTooltip(record, header)}
            multiline
            withArrow
            openDelay={400}
            w={240}
          >
            <Group
              gap={6}
              wrap="nowrap"
              style={{ fontVariantNumeric: "tabular-nums" }}
            >
              {header ? (
                <>
                  <Text size="xs" c={tokens.fg.muted} component="span">
                    <StatDot color={tokens.category.node} />
                    <span style={{ color: tokens.category.node, fontWeight: 500 }}>
                      {header.nodeCount.toLocaleString()}
                    </span>
                    <span style={{ color: tokens.fg.subtle, marginLeft: 3 }}>
                      n
                    </span>
                  </Text>
                  <Text size="xs" c={tokens.fg.muted} component="span">
                    <StatDot color={tokens.category.relationship} />
                    <span
                      style={{
                        color: tokens.category.relationship,
                        fontWeight: 500,
                      }}
                    >
                      {header.relationshipCount.toLocaleString()}
                    </span>
                    <span style={{ color: tokens.fg.subtle, marginLeft: 3 }}>
                      r
                    </span>
                  </Text>
                  <Text size="xs" c={tokens.fg.subtle}>
                    ·
                  </Text>
                </>
              ) : null}
              <Text size="xs" c={tokens.fg.subtle}>
                {formatBytes(record.sizeBytes)}
              </Text>
            </Group>
          </Tooltip>
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
