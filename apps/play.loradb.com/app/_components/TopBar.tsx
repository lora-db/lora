"use client";

/**
 * Top chrome strip: app name, run button, color-scheme toggle, and a
 * tiny DB-status indicator dot.
 */

import { ActionIcon, Group, Text, Tooltip } from "@mantine/core";
import {
  IconBraces,
  IconKeyboard,
  IconLayoutColumns,
  IconLayoutRows,
  IconMoon,
  IconSun,
} from "@tabler/icons-react";

import { toggleRootOrientation } from "@/lib/actions/workspaceActions";
import { useStore } from "@/lib/state/store";
import { toggleParamsPanel } from "@/lib/actions/uiActions";
import { useActiveTabId, useDetectedParams } from "@/lib/state/selectors";
import { findLeaf, resolveActiveViewId } from "@/lib/state/workspace/tree";
import {
  useColorSchemeToggle,
  usePlaygroundTheme,
} from "@/lib/theme/usePlaygroundTheme";
import { useDbStatus } from "@/lib/hooks/useDbStatus";

import { openHotkeyHelpDialog } from "./Dialogs/HotkeyHelpDialog";
import { RunButton } from "./Editor/RunButton";

function dotColor(
  state: ReturnType<typeof useDbStatus>["state"],
  success: string,
  warning: string,
  danger: string,
): string {
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
  const rootDirection = useStore((s) =>
    s.workspace.type === "group" ? s.workspace.direction : null,
  );
  const isHorizontal = rootDirection === "row";
  const canToggleOrientation = rootDirection !== null;

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
        <Text
          fw={600}
          size="sm"
          c={tokens.fg.primary}
          style={{ letterSpacing: 0.2 }}
        >
          LoraDB Playground
        </Text>
      </Group>

      <Group gap="xs" align="center" wrap="nowrap">
        <RunButton />
        <ParamsToggleButton />
        {canToggleOrientation && (
          <Tooltip
            label={
              isHorizontal ? "Stack panes top/bottom" : "Place panes left/right"
            }
            withArrow
          >
            <ActionIcon
              variant="subtle"
              size="md"
              color="gray"
              onClick={() => toggleRootOrientation()}
              aria-label="Toggle split orientation"
              data-testid="toggle-orientation"
            >
              {isHorizontal ? (
                <IconLayoutRows size={16} />
              ) : (
                <IconLayoutColumns size={16} />
              )}
            </ActionIcon>
          </Tooltip>
        )}
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

/**
 * Params panel toggle. Bound-count badge appears as a coloured dot
 * when the active query references any `$param`, so the affordance
 * tells the user there's bindings to fill.
 */
function ParamsToggleButton() {
  const { tokens } = usePlaygroundTheme();
  const paramsOpen = useStore((s) => {
    const viewId = resolveActiveViewId(s.workspace, s.activePaneId);
    if (!viewId) return false;
    const leaf = findLeaf(s.workspace, s.activePaneId);
    return leaf?.views.find((v) => v.id === viewId)?.paramsPanelOpen ?? false;
  });
  const activeTabId = useActiveTabId();
  const detected = useDetectedParams(activeTabId ?? undefined);
  const hasParams = detected.length > 0;

  const tooltip = hasParams
    ? paramsOpen
      ? `Hide Params (${detected.length} bound)`
      : `Show Params (${detected.length} bound)`
    : paramsOpen
      ? "Hide Params panel"
      : "Show Params panel — write bindings before adding $params";

  return (
    <Tooltip label={tooltip} withArrow>
      <ActionIcon
        variant={paramsOpen ? "light" : "subtle"}
        size="md"
        color={hasParams ? "blue" : "gray"}
        onClick={() => toggleParamsPanel()}
        aria-label="Toggle Params panel"
        data-testid="toggle-params-panel"
        style={{ position: "relative" }}
      >
        <IconBraces size={16} />
        {hasParams && (
          <span
            aria-hidden
            style={{
              position: "absolute",
              top: 2,
              right: 2,
              width: 6,
              height: 6,
              borderRadius: "50%",
              background: tokens.accent.primary,
              border: `1px solid ${tokens.bg.panel}`,
            }}
          />
        )}
      </ActionIcon>
    </Tooltip>
  );
}
