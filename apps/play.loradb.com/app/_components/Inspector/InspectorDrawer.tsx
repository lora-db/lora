"use client";

/**
 * Right-anchored drawer that surfaces a node or relationship's
 * labels/type, IDs, and property bag. Opened by graph clicks and
 * Glide-grid cell clicks via the inspect actions.
 *
 * Mounted as a sibling of the result Tabs so the Mantine overlay
 * floats correctly above both editor and result panes.
 */

import { useCallback } from "react";
import {
  Badge,
  Button,
  Code,
  Drawer,
  Group,
  ScrollArea,
  Stack,
  Text,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";

import { closeInspect } from "@/lib/actions/inspectActions";
import { runActiveTab } from "@/lib/actions/runActiveTab";
import { useStore } from "@/lib/state/store";
import type { InspectTarget } from "@/lib/state/slices/inspect";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

const DRAWER_WIDTH = 360;

function renderValue(value: unknown): string {
  if (value === null) return "null";
  if (value === undefined) return "undefined";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  if (typeof value === "bigint") return `${value.toString()}n`;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function PropertyTable({
  properties,
  tokens,
}: {
  properties: Record<string, unknown>;
  tokens: ReturnType<typeof usePlaygroundTheme>["tokens"];
}) {
  const keys = Object.keys(properties);
  if (keys.length === 0) {
    return (
      <Text size="xs" c={tokens.fg.subtle}>
        No properties.
      </Text>
    );
  }
  return (
    <Stack gap={6}>
      {keys.map((k) => {
        const text = renderValue(properties[k]);
        return (
          <Group key={k} align="flex-start" gap="sm" wrap="nowrap">
            <Text
              size="xs"
              c={tokens.fg.muted}
              style={{
                minWidth: 96,
                maxWidth: 120,
                wordBreak: "break-word",
              }}
              ff={tokens.font.mono}
            >
              {k}
            </Text>
            <Code
              block
              style={{
                flex: 1,
                background: tokens.bg.panel,
                color: tokens.fg.primary,
                whiteSpace: "pre-wrap",
                wordBreak: "break-word",
                fontFamily: tokens.font.mono,
                fontSize: 12,
              }}
            >
              {text}
            </Code>
          </Group>
        );
      })}
    </Stack>
  );
}

function copyToClipboard(value: string, label: string): void {
  if (typeof navigator === "undefined" || !navigator.clipboard) {
    notifications.show({
      color: "red",
      title: "Copy failed",
      message: "Clipboard API unavailable in this context.",
    });
    return;
  }
  navigator.clipboard
    .writeText(value)
    .then(() => {
      notifications.show({
        color: "green",
        title: "Copied",
        message: `${label} copied to clipboard.`,
        autoClose: 1500,
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

function visualizeNeighbors(target: InspectTarget): void {
  const idValue = typeof target.id === "number" ? target.id : `"${String(target.id)}"`;
  const body = `MATCH (n)-[r]-(m)\nWHERE id(n) = ${idValue}\nRETURN n, r, m`;
  const name =
    target.kind === "node"
      ? `Neighbors of ${target.id}`
      : `Neighbors of ${target.type} ${target.id}`;
  const state = useStore.getState();
  state.openTab({ name, body });
  void runActiveTab();
}

function NodeBody({
  target,
  tokens,
}: {
  target: Extract<InspectTarget, { kind: "node" }>;
  tokens: ReturnType<typeof usePlaygroundTheme>["tokens"];
}) {
  return (
    <Stack gap="md">
      <Group gap={6} wrap="wrap">
        {target.labels.length === 0 ? (
          <Text size="xs" c={tokens.fg.subtle}>
            No labels.
          </Text>
        ) : (
          target.labels.map((l) => (
            <Badge key={l} color="blue" variant="light" size="sm">
              {l}
            </Badge>
          ))
        )}
      </Group>
      <Text size="xs" c={tokens.fg.muted} ff={tokens.font.mono}>
        id: {String(target.id)}
      </Text>
      <PropertyTable properties={target.properties} tokens={tokens} />
    </Stack>
  );
}

function RelationshipBody({
  target,
  tokens,
}: {
  target: Extract<InspectTarget, { kind: "relationship" }>;
  tokens: ReturnType<typeof usePlaygroundTheme>["tokens"];
}) {
  return (
    <Stack gap="md">
      <Group gap={6}>
        <Badge color="orange" variant="light" size="sm">
          :{target.type || "?"}
        </Badge>
      </Group>
      <Text size="xs" c={tokens.fg.muted} ff={tokens.font.mono}>
        from {String(target.startId)} → to {String(target.endId)}
      </Text>
      <Text size="xs" c={tokens.fg.muted} ff={tokens.font.mono}>
        id: {String(target.id)}
      </Text>
      <PropertyTable properties={target.properties} tokens={tokens} />
    </Stack>
  );
}

export function InspectorDrawer() {
  const { tokens } = usePlaygroundTheme();
  const target = useStore((s) => s.inspect);

  const onCopyId = useCallback(() => {
    if (!target) return;
    copyToClipboard(String(target.id), "ID");
  }, [target]);

  const onVisualize = useCallback(() => {
    if (!target) return;
    visualizeNeighbors(target);
  }, [target]);

  const title =
    target === null
      ? ""
      : target.kind === "node"
        ? `Node ${target.id}`
        : `Relationship :${target.type}`;

  return (
    <Drawer
      opened={target !== null}
      onClose={closeInspect}
      position="right"
      size={DRAWER_WIDTH}
      withinPortal
      padding="md"
      title={
        <Text fw={600} size="sm" ff={tokens.font.mono}>
          {title}
        </Text>
      }
      overlayProps={{ backgroundOpacity: 0.25, blur: 1 }}
    >
      {target === null ? null : (
        <Stack gap="md" style={{ height: "100%" }}>
          <ScrollArea style={{ flex: 1, minHeight: 0 }}>
            {target.kind === "node" ? (
              <NodeBody target={target} tokens={tokens} />
            ) : (
              <RelationshipBody target={target} tokens={tokens} />
            )}
          </ScrollArea>
          <Group justify="space-between">
            <Button size="xs" variant="default" onClick={onCopyId}>
              Copy ID
            </Button>
            <Button size="xs" variant="light" onClick={onVisualize}>
              Visualize neighbors
            </Button>
          </Group>
        </Stack>
      )}
    </Drawer>
  );
}
