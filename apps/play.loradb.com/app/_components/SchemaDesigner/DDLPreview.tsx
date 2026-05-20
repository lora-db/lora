"use client";

/**
 * Read-only Cypher block used by the wizards to show the DDL that
 * will be sent on submit. A "Copy" button drops the string into the
 * clipboard; an "Open in editor" button creates a new tab pre-filled
 * with the DDL so power users can tweak before running.
 */

import { useMemo } from "react";
import { ActionIcon, Box, Group, Stack, Text, Tooltip } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { IconArrowUpRight, IconCopy } from "@tabler/icons-react";
import { LoraQueryEditor, formatSync } from "@loradb/lora-query";

import { openTabInCell } from "@/lib/actions/tabActions";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

interface DDLPreviewProps {
  /** The Cypher to display. */
  ddl: string;
  /** Optional caption shown above the block. */
  caption?: string;
}

export function DDLPreview({ ddl, caption }: DDLPreviewProps) {
  const { tokens, editor } = usePlaygroundTheme();

  // Prettify the generated DDL through the WASM formatter. The
  // underlying call returns the original source on parse error, so
  // the worst case is the raw string we already had.
  const formatted = useMemo(() => {
    try {
      return formatSync(ddl);
    } catch {
      return ddl;
    }
  }, [ddl]);

  const copy = () => {
    if (typeof window === "undefined" || !navigator.clipboard) return;
    void navigator.clipboard.writeText(formatted).then(
      () => {
        notifications.show({
          color: "green",
          title: "Copied",
          message: "DDL copied to clipboard.",
          autoClose: 1500,
        });
      },
      (err: unknown) => {
        notifications.show({
          color: "red",
          title: "Copy failed",
          message: err instanceof Error ? err.message : String(err),
        });
      },
    );
  };

  const openInEditor = () => {
    openTabInCell({
      name: "Schema DDL",
      body: formatted,
    });
    notifications.show({
      color: "blue",
      title: "Opened in a new tab",
      message: "Run it manually if you want to tweak first.",
      autoClose: 2000,
    });
  };

  return (
    <Stack gap={4}>
      <Group gap={8} justify="space-between" align="center">
        <Text size="xs" c={tokens.fg.muted} tt="uppercase" fw={600}>
          {caption ?? "Generated Cypher"}
        </Text>
        <Group gap={4} wrap="nowrap">
          <Tooltip label="Copy" withArrow>
            <ActionIcon
              variant="subtle"
              size="sm"
              color="gray"
              onClick={copy}
              aria-label="Copy DDL"
            >
              <IconCopy size={12} />
            </ActionIcon>
          </Tooltip>
          <Tooltip label="Open in editor" withArrow>
            <ActionIcon
              variant="subtle"
              size="sm"
              color="gray"
              onClick={openInEditor}
              aria-label="Open in editor"
            >
              <IconArrowUpRight size={12} />
            </ActionIcon>
          </Tooltip>
        </Group>
      </Group>
      <Box
        style={{
          border: `1px solid ${tokens.border.subtle}`,
          borderRadius: tokens.radius.sm,
          overflow: "hidden",
        }}
      >
        <LoraQueryEditor
          value={formatted}
          readOnly
          theme={editor}
          showLineNumbers={false}
          minHeight="48px"
          maxHeight="160px"
        />
      </Box>
    </Stack>
  );
}
