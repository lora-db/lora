"use client";

/**
 * `HotkeyHelpDialog` — `?` cheat sheet for every chord we register.
 *
 * Reads {@link HOTKEYS} and groups by the `group` field so the dialog
 * stays in sync with `bindings.ts` automatically. Add a binding to the
 * map and it shows up here on next render.
 */

import { useMemo } from "react";
import { Group, Kbd, Stack, Text } from "@mantine/core";
import { modals } from "@mantine/modals";

import {
  HOTKEYS,
  chordFor,
  type HotkeyId,
  type HotkeyMeta,
} from "@/lib/hotkeys/labels";

const MODAL_ID = "loradb-hotkey-help";

interface Row {
  id: HotkeyId;
  meta: HotkeyMeta;
}

function groupRows(): Record<HotkeyMeta["group"], Row[]> {
  const buckets: Record<HotkeyMeta["group"], Row[]> = {
    Run: [],
    Tabs: [],
    Navigation: [],
    View: [],
    Editor: [],
    Panes: [],
    App: [],
  };
  for (const id of Object.keys(HOTKEYS) as HotkeyId[]) {
    const meta = HOTKEYS[id];
    buckets[meta.group].push({ id, meta });
  }
  return buckets;
}

function HotkeyHelpDialogBody() {
  const grouped = useMemo(groupRows, []);
  const order: HotkeyMeta["group"][] = [
    "Run",
    "Tabs",
    "Editor",
    "View",
    "Panes",
    "Navigation",
    "App",
  ];
  return (
    <Stack
      gap="md"
      tabIndex={-1}
      data-autofocus
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          modals.close(MODAL_ID);
        }
      }}
    >
      <Text size="xs" c="dimmed">
        Every chord the workbench listens for. Combinations render in the
        active platform&rsquo;s convention.
      </Text>
      {order.map((bucket) => {
        const rows = grouped[bucket];
        if (rows.length === 0) return null;
        return (
          <Stack key={bucket} gap={4}>
            <Text size="xs" fw={600} tt="uppercase" c="dimmed">
              {bucket}
            </Text>
            <Stack gap={2}>
              {rows.map(({ id, meta }) => (
                <Group key={id} justify="space-between" wrap="nowrap" gap="md">
                  <Text size="sm">{meta.description}</Text>
                  <Kbd style={{ fontSize: 11 }}>{chordFor(id)}</Kbd>
                </Group>
              ))}
            </Stack>
          </Stack>
        );
      })}
    </Stack>
  );
}

/** Open (or focus) the keyboard-shortcuts dialog. Idempotent. */
export function openHotkeyHelpDialog(): void {
  modals.open({
    modalId: MODAL_ID,
    title: "Keyboard shortcuts",
    centered: true,
    size: "md",
    children: <HotkeyHelpDialogBody />,
  });
}
