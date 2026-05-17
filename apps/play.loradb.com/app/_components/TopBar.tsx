"use client";

/**
 * Top chrome strip: app name, run button, color-scheme toggle, and a
 * tiny DB-status indicator dot.
 */

import { ActionIcon, Group, Text, Tooltip } from "@mantine/core";
import { IconKeyboard, IconMoon, IconSun } from "@tabler/icons-react";

import { useColorSchemeToggle, usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import { useDbStatus } from "@/lib/hooks/useDbStatus";

import { openHotkeyHelpDialog } from "./Dialogs/HotkeyHelpDialog";
import { RunButton } from "./Editor/RunButton";

function dotColor(state: ReturnType<typeof useDbStatus>["state"], success: string, warning: string, danger: string): string {
  switch (state) {
    case "ready":
      return success;
    case "booting":
    case "idle":
      return warning;
    case "error":
      return danger;
  }
}

export function TopBar() {
  const { tokens, scheme } = usePlaygroundTheme();
  const toggle = useColorSchemeToggle();
  const status = useDbStatus();

  return (
    <Group
      h="100%"
      px="md"
      justify="space-between"
      align="center"
      wrap="nowrap"
      style={{ borderBottom: `1px solid ${tokens.border.subtle}` }}
    >
      <Group gap="sm" align="center" wrap="nowrap">
        <Text fw={600} size="sm" c={tokens.fg.primary} style={{ letterSpacing: 0.2 }}>
          LoraDB Playground
        </Text>
      </Group>

      <Group gap="xs" align="center" wrap="nowrap">
        <RunButton />
      </Group>

      <Group gap="xs" align="center" wrap="nowrap">
        <Tooltip
          label={
            status.state === "ready"
              ? "DB ready"
              : status.state === "booting"
                ? "Booting WASM…"
                : status.state === "error"
                  ? `DB error: ${status.error ?? "unknown"}`
                  : "Idle"
          }
          withArrow
        >
          <Group gap={6} align="center" wrap="nowrap">
            <span
              aria-hidden="true"
              style={{
                width: 8,
                height: 8,
                borderRadius: "50%",
                background: dotColor(
                  status.state,
                  tokens.accent.success,
                  tokens.accent.warning,
                  tokens.accent.danger,
                ),
                display: "inline-block",
              }}
            />
            <Text size="xs" c={tokens.fg.muted} ff={tokens.font.mono}>
              {status.state === "ready" ? "ready" : status.state}
            </Text>
          </Group>
        </Tooltip>
        <Tooltip label="Keyboard shortcuts" withArrow>
          <ActionIcon
            variant="subtle"
            size="md"
            color="gray"
            onClick={() => openHotkeyHelpDialog()}
            aria-label="Show keyboard shortcuts"
          >
            <IconKeyboard size={16} />
          </ActionIcon>
        </Tooltip>
        <Tooltip
          label={scheme === "dark" ? "Light mode" : "Dark mode"}
          withArrow
        >
          <ActionIcon
            variant="subtle"
            size="md"
            color="gray"
            onClick={toggle}
            aria-label="Toggle color scheme"
          >
            {scheme === "dark" ? <IconSun size={16} /> : <IconMoon size={16} />}
          </ActionIcon>
        </Tooltip>
      </Group>
    </Group>
  );
}
