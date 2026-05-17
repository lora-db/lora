"use client";

/**
 * Bottom status strip: DB state + counts on the left, last-run timing
 * in the middle, graph-mode + color-scheme toggles on the right.
 */

import { ActionIcon, Group, Text, Tooltip } from "@mantine/core";
import { IconCube, IconMoon, IconSquare, IconSun } from "@tabler/icons-react";

import { useStore } from "@/lib/state/store";
import { useActiveResult } from "@/lib/state/selectors";
import { useDbStatus } from "@/lib/hooks/useDbStatus";
import { formatCount, formatMs } from "@/lib/util/format";
import { useColorSchemeToggle, usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

export function StatusBar() {
  const { tokens, scheme } = usePlaygroundTheme();
  const toggle = useColorSchemeToggle();
  const status = useDbStatus();
  const result = useActiveResult();
  const graphMode = useStore((s) => s.graphMode);
  const setPref = useStore((s) => s.setPref);

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
        <Text size="xs" c={tokens.fg.muted} ff={tokens.font.mono}>
          db: {status.state}
        </Text>
        <Text size="xs" c={tokens.fg.subtle} ff={tokens.font.mono}>
          · nodes {formatCount(status.nodes)} · rels {formatCount(status.rels)}
        </Text>
      </Group>

      <Group gap="xs" align="center" wrap="nowrap">
        {ms !== null && (
          <Text size="xs" c={tokens.fg.muted} ff={tokens.font.mono}>
            {formatMs(ms)}
            {rows !== null && ` · ${formatCount(rows)} row${rows === 1 ? "" : "s"}`}
          </Text>
        )}
      </Group>

      <Group gap="xs" align="center" wrap="nowrap">
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
