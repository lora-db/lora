"use client";

/**
 * `SavedQueriesPanel` — CRUD UI over the `savedQueries` IDB store.
 *
 * The panel listens for the `loradb:savedQueries` window event so any
 * action that mutates persistence (from anywhere in the app) triggers
 * a refresh here without prop-drilling a reload callback.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ActionIcon,
  Badge,
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
  IconBookmark,
  IconCopy,
  IconDeviceFloppy,
  IconDots,
  IconEdit,
  IconLink,
  IconRefresh,
  IconSearch,
  IconTrash,
} from "@tabler/icons-react";
import { formatDistanceToNowStrict } from "date-fns";

import * as savedQueries from "@/lib/persistence/savedQueries";
import {
  SAVED_QUERIES_EVENT,
  deleteSavedQuery,
  duplicateSavedQuery,
  openSavedQuery,
  renameSavedQuery,
  saveActiveTab,
  saveActiveTabAs,
} from "@/lib/actions/savedQueryActions";
import { encodeQuery } from "@/lib/share/encode";
import { useActiveTab } from "@/lib/state/selectors";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

import { openRenameQueryDialog } from "../Dialogs/RenameQueryDialog";
import { openSaveQueryDialog } from "../Dialogs/SaveQueryDialog";

function copyShareLink(record: savedQueries.SavedQuery): void {
  if (typeof window === "undefined" || !navigator.clipboard) return;
  const url = `${window.location.origin}/#q=${encodeQuery(record.body)}`;
  navigator.clipboard
    .writeText(url)
    .then(() => {
      notifications.show({
        color: "green",
        title: "Link copied",
        message: `Share link for "${record.name}" copied to clipboard.`,
      });
    })
    .catch((err: unknown) => {
      notifications.show({
        color: "red",
        title: "Copy failed",
        message: err instanceof Error ? err.message : String(err),
      });
    });
}

export function SavedQueriesPanel() {
  const { tokens } = usePlaygroundTheme();
  const activeTab = useActiveTab();
  const [items, setItems] = useState<savedQueries.SavedQuery[]>([]);
  const [loading, setLoading] = useState(true);
  const [filter, setFilter] = useState("");

  // Case-insensitive substring match against name, tags, and body so a
  // user looking for a half-remembered query can find it by any anchor.
  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (q.length === 0) return items;
    return items.filter((record) => {
      if (record.name.toLowerCase().includes(q)) return true;
      if (record.body.toLowerCase().includes(q)) return true;
      return record.tags.some((tag) => tag.toLowerCase().includes(q));
    });
  }, [items, filter]);

  const refresh = useCallback((): void => {
    savedQueries
      .list()
      .then((rows) => {
        setItems(rows);
        setLoading(false);
      })
      .catch((err: unknown) => {
        setLoading(false);
        notifications.show({
          color: "red",
          title: "Failed to load saved queries",
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
    window.addEventListener(SAVED_QUERIES_EVENT, handler);
    return () => {
      window.removeEventListener(SAVED_QUERIES_EVENT, handler);
    };
  }, [refresh]);

  const handleSaveCurrent = useCallback((): void => {
    if (!activeTab) {
      notifications.show({
        color: "yellow",
        title: "No active tab",
        message: "Open a query tab before saving.",
      });
      return;
    }
    if (activeTab.savedQueryId) {
      // Tab is already bound — update in place.
      saveActiveTab()
        .then((record) => {
          if (record) {
            notifications.show({
              color: "green",
              title: "Query saved",
              message: `Updated "${record.name}".`,
            });
          }
        })
        .catch((err: unknown) => {
          notifications.show({
            color: "red",
            title: "Save failed",
            message: err instanceof Error ? err.message : String(err),
          });
        });
      return;
    }
    openSaveQueryDialog({
      defaultName: activeTab.name,
      onSave: async (name, tags) => {
        try {
          const record = await saveActiveTabAs(name, tags);
          notifications.show({
            color: "green",
            title: "Query saved",
            message: `Saved as "${record.name}".`,
          });
        } catch (err) {
          notifications.show({
            color: "red",
            title: "Save failed",
            message: err instanceof Error ? err.message : String(err),
          });
          throw err;
        }
      },
    });
  }, [activeTab]);

  const handleRename = useCallback((record: savedQueries.SavedQuery): void => {
    openRenameQueryDialog({
      currentName: record.name,
      onRename: async (name) => {
        try {
          await renameSavedQuery(record.id, name);
          notifications.show({
            color: "green",
            title: "Renamed",
            message: `"${record.name}" → "${name}".`,
          });
        } catch (err) {
          notifications.show({
            color: "red",
            title: "Rename failed",
            message: err instanceof Error ? err.message : String(err),
          });
          throw err;
        }
      },
    });
  }, []);

  const handleDuplicate = useCallback(
    (record: savedQueries.SavedQuery): void => {
      duplicateSavedQuery(record.id)
        .then((copy) => {
          if (copy) {
            notifications.show({
              color: "green",
              title: "Duplicated",
              message: `Created "${copy.name}".`,
            });
          }
        })
        .catch((err: unknown) => {
          notifications.show({
            color: "red",
            title: "Duplicate failed",
            message: err instanceof Error ? err.message : String(err),
          });
        });
    },
    [],
  );

  const handleDelete = useCallback((record: savedQueries.SavedQuery): void => {
    openConfirmModal({
      title: "Delete saved query?",
      centered: true,
      children: (
        <Text size="sm" c={tokens.fg.muted}>
          Permanently delete <strong>{record.name}</strong>? Any open
          tab bound to it stays open but loses its saved-query link.
        </Text>
      ),
      labels: { confirm: "Delete", cancel: "Cancel" },
      confirmProps: { color: "red", "data-autofocus": "true" },
      onConfirm: () => {
        deleteSavedQuery(record.id)
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
  }, [tokens.fg.muted]);

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
          Queries
        </Text>
        <Group gap={4} wrap="nowrap">
          <Tooltip label="Save current query" withArrow>
            <ActionIcon
              variant="subtle"
              size="sm"
              color="gray"
              onClick={handleSaveCurrent}
              aria-label="Save current query"
            >
              <IconDeviceFloppy size={14} />
            </ActionIcon>
          </Tooltip>
          <Tooltip label="Refresh" withArrow>
            <ActionIcon
              variant="subtle"
              size="sm"
              color="gray"
              onClick={refresh}
              aria-label="Refresh saved queries"
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
            placeholder="Filter queries"
            aria-label="Filter saved queries"
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
              <IconBookmark size={28} color={tokens.fg.subtle} stroke={1.5} />
              <Text size="xs" c={tokens.fg.subtle} ta="center">
                No saved queries yet
              </Text>
              <Button
                size="xs"
                variant="light"
                onClick={handleSaveCurrent}
                leftSection={<IconDeviceFloppy size={14} />}
              >
                Save current query
              </Button>
            </Stack>
          </Center>
        ) : filtered.length === 0 ? (
          <Center p="md">
            <Text size="xs" c={tokens.fg.subtle} ta="center">
              No queries match &ldquo;{filter}&rdquo;
            </Text>
          </Center>
        ) : (
          <Stack gap={0} p={4}>
            {filtered.map((record) => {
              const isActive =
                activeTab !== null && activeTab.savedQueryId === record.id;
              return (
                <SavedQueryRow
                  key={record.id}
                  record={record}
                  isActive={isActive}
                  onOpen={() => {
                    openSavedQuery(record.id).catch((err: unknown) => {
                      notifications.show({
                        color: "red",
                        title: "Open failed",
                        message:
                          err instanceof Error ? err.message : String(err),
                      });
                    });
                  }}
                  onRename={() => {
                    handleRename(record);
                  }}
                  onDuplicate={() => {
                    handleDuplicate(record);
                  }}
                  onDelete={() => {
                    handleDelete(record);
                  }}
                  onCopyLink={() => {
                    copyShareLink(record);
                  }}
                />
              );
            })}
          </Stack>
        )}
      </ScrollArea>
    </Stack>
  );
}

interface SavedQueryRowProps {
  record: savedQueries.SavedQuery;
  isActive: boolean;
  onOpen: () => void;
  onRename: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
  onCopyLink: () => void;
}

function SavedQueryRow({
  record,
  isActive,
  onOpen,
  onRename,
  onDuplicate,
  onDelete,
  onCopyLink,
}: SavedQueryRowProps) {
  const { tokens } = usePlaygroundTheme();
  const [menuOpen, setMenuOpen] = useState(false);

  return (
    <Group
      gap={0}
      wrap="nowrap"
      align="stretch"
      style={{
        background: isActive ? tokens.bg.overlay : "transparent",
        borderRadius: tokens.radius.sm,
        position: "relative",
      }}
    >
      <UnstyledButton
        onClick={onOpen}
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
        <Stack gap={2}>
          <Text
            size="sm"
            fw={isActive ? 600 : 500}
            c={tokens.fg.primary}
            truncate
            title={record.name}
          >
            {record.name}
          </Text>
          {record.tags.length > 0 ? (
            <Group gap={4} wrap="wrap">
              {record.tags.map((tag) => (
                <Badge
                  key={tag}
                  size="xs"
                  variant="light"
                  color="gray"
                  radius="sm"
                >
                  {tag}
                </Badge>
              ))}
            </Group>
          ) : null}
          <Text size="xs" c={tokens.fg.subtle} component="time" dateTime={new Date(record.updatedAt).toISOString()}>
            {formatDistanceToNowStrict(record.updatedAt, { addSuffix: true })}
          </Text>
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
            <Menu.Item leftSection={<IconEdit size={14} />} onClick={onRename}>
              Rename
            </Menu.Item>
            <Menu.Item leftSection={<IconCopy size={14} />} onClick={onDuplicate}>
              Duplicate
            </Menu.Item>
            <Menu.Item leftSection={<IconLink size={14} />} onClick={onCopyLink}>
              Copy link
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
