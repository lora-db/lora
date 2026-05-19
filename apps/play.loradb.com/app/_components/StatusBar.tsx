"use client";

/**
 * Bottom status strip: DB state + counts on the left, last-run timing
 * in the middle, graph-mode + color-scheme toggles on the right.
 */

import { ActionIcon, Group, Text, Tooltip } from "@mantine/core";
import { IconBraces, IconCube, IconMoon, IconSquare, IconSun } from "@tabler/icons-react";

import { useStore } from "@/lib/state/store";
import {
  useActiveResult,
  useActiveTabId,
  useDetectedParams,
} from "@/lib/state/selectors";
import {
  toggleParamsPanel as toggleParamsPanelAction,
} from "@/lib/actions/uiActions";
import {
  findLeaf,
  resolveActiveViewId,
} from "@/lib/state/workspace/tree";
import { useDbStatus, type DbState } from "@/lib/hooks/useDbStatus";
import { formatCount, formatMs } from "@/lib/util/format";
import { useColorSchemeToggle, usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import type { Tokens } from "@/lib/theme/tokens";
import { LORA_WASM_VERSION } from "@/lib/version";

/** Maps a {@link DbState} to a semantic accent colour so the bottom-bar
 *  dot reads at a glance: green when the WASM DB is up, amber while it
 *  boots, red on a failed boot, neutral before the boot starts. */
function dbStateColor(state: DbState, tokens: Tokens): string {
  if (state === "ready") return tokens.accent.success;
  if (state === "booting") return tokens.accent.warning;
  if (state === "error") return tokens.accent.danger;
  return tokens.fg.subtle;
}

/** Same thresholds as the ResultPane summary strip so the last-run
 *  timing in the bottom bar reads the same colour as the top-of-pane
 *  chip for the same query. */
function speedColor(ms: number, tokens: Tokens): string {
  if (ms < 50) return tokens.accent.success;
  if (ms < 200) return tokens.accent.warning;
  return tokens.accent.danger;
}

/** Small filled circle used as the DB-state indicator. The
 *  `box-shadow` halo gives the dot a soft glow on dark surfaces so it
 *  reads even at 8px without looking pixel-thin. */
function StatusDot({ color }: { color: string }) {
  return (
    <span
      aria-hidden
      style={{
        display: "inline-block",
        width: 8,
        height: 8,
        borderRadius: "50%",
        background: color,
        boxShadow: `0 0 0 2px ${color}22`,
        flexShrink: 0,
      }}
    />
  );
}

export function StatusBar() {
  const { tokens, scheme } = usePlaygroundTheme();
  const toggle = useColorSchemeToggle();
  const status = useDbStatus();
  const result = useActiveResult();
  const graphMode = useStore((s) => s.graphMode);
  const setPref = useStore((s) => s.setPref);
  const activeTabId = useActiveTabId();
  const detected = useDetectedParams(activeTabId ?? undefined);
  const paramsOpen = useStore((s) => {
    const viewId = resolveActiveViewId(s.workspace, s.activePaneId);
    if (!viewId) return false;
    const leaf = findLeaf(s.workspace, s.activePaneId);
    return leaf?.views.find((v) => v.id === viewId)?.paramsPanelOpen ?? false;
  });

  const ms = result && (result.state === "ok" || result.state === "error") ? result.ms : null;
  const rows = result && result.state === "ok" ? result.result.rows.length : null;

  return (
    <Group
      h="100%"
      px="sm"
      gap="md"
      align="center"
      wrap="nowrap"
      justify="space-between"
      style={{
        borderTop: `1px solid ${tokens.border.subtle}`,
        background: tokens.bg.panel,
        fontSize: 11,
      }}
    >
      <Group gap="xs" align="center" wrap="nowrap">
        <Group gap={6} align="center" wrap="nowrap">
          <StatusDot color={dbStateColor(status.state, tokens)} />
          <Text size="xs" c={tokens.fg.muted} ff={tokens.font.mono}>
            db: {status.state}
          </Text>
        </Group>
        <Tooltip
          label={`@loradb/lora-wasm v${LORA_WASM_VERSION} — the WASM engine baked into this build`}
          withArrow
        >
          <Text size="xs" c={tokens.fg.subtle} ff={tokens.font.mono}>
            · wasm{" "}
            <Text span inherit c={tokens.fg.muted} fw={600}>
              v{LORA_WASM_VERSION}
            </Text>
          </Text>
        </Tooltip>
        <Text size="xs" c={tokens.fg.subtle} ff={tokens.font.mono}>
          · nodes{" "}
          <Text span inherit c={tokens.category.node} fw={600}>
            {formatCount(status.nodes)}
          </Text>{" "}
          · rels{" "}
          <Text span inherit c={tokens.category.relationship} fw={600}>
            {formatCount(status.rels)}
          </Text>
        </Text>
      </Group>

      <Group gap="xs" align="center" wrap="nowrap">
        {ms !== null && (
          <Text size="xs" c={tokens.fg.subtle} ff={tokens.font.mono}>
            {rows !== null && (
              <>
                <Text span inherit c={tokens.fg.primary} fw={600}>
                  {formatCount(rows)}
                </Text>{" "}
                row{rows === 1 ? "" : "s"}
                {" · "}
              </>
            )}
            <Text span inherit c={speedColor(ms, tokens)} fw={600}>
              {formatMs(ms)}
            </Text>
          </Text>
        )}
      </Group>

      <Group gap="xs" align="center" wrap="nowrap">
        <Tooltip
          label={
            detected.length === 0
              ? paramsOpen
                ? "Hide Params panel"
                : "Open Params panel"
              : paramsOpen
                ? `Hide Params (${detected.length} bound)`
                : `Open Params (${detected.length} bound)`
          }
          withArrow
        >
          <ActionIcon
            variant={paramsOpen ? "light" : "subtle"}
            size="sm"
            color={detected.length > 0 ? "blue" : "gray"}
            onClick={() => toggleParamsPanelAction()}
            aria-label="Toggle Params panel"
          >
            <IconBraces size={14} />
          </ActionIcon>
        </Tooltip>
        <Tooltip label={`Graph: ${graphMode}`} withArrow>
          <ActionIcon
            variant="subtle"
            size="sm"
            color="gray"
            onClick={() => setPref("graphMode", graphMode === "2d" ? "3d" : "2d")}
            aria-label={`Switch to ${graphMode === "2d" ? "3D" : "2D"} graph`}
          >
            {graphMode === "2d" ? <IconSquare size={14} /> : <IconCube size={14} />}
          </ActionIcon>
        </Tooltip>
        <Tooltip label={scheme === "dark" ? "Light mode" : "Dark mode"} withArrow>
          <ActionIcon
            variant="subtle"
            size="sm"
            color="gray"
            onClick={toggle}
            aria-label="Toggle color scheme"
          >
            {scheme === "dark" ? <IconSun size={14} /> : <IconMoon size={14} />}
          </ActionIcon>
        </Tooltip>
      </Group>
    </Group>
  );
}
