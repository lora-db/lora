"use client";

/**
 * Per-pane Params panel — sits to the right of the EditorPane and
 * holds the raw JSON payload for the active query's `$param`
 * bindings.
 *
 * The panel is dynamic: when the Cypher analyser reports any `$param`
 * for the tab, those names become both:
 *   - the autocomplete source (`knownKeys`) inside the JSON editor,
 *   - the `allowedKeys` whitelist (extra keys lint as errors),
 *   - the `requiredKeys` list (missing keys lint as warnings).
 *
 * The JSON source is stored on the tab record (`tab.params`) so it
 * persists with the rest of the workspace. `runActiveTab` parses the
 * source at run time and forwards the result to the driver.
 */

import { useCallback, useMemo, useRef } from "react";
import dynamic from "next/dynamic";
import {
  ActionIcon,
  Box,
  Divider,
  Group,
  Menu,
  Text,
  Tooltip,
} from "@mantine/core";
import {
  IconChevronRight,
  IconDots,
  IconQuote,
  IconSortAscending,
  IconWand,
} from "@tabler/icons-react";

import type { LoraJsonEditorHandle } from "@loradb/lora-query";

import { useStore } from "@/lib/state/store";
import {
  useDetectedParams,
  useTabById,
  useTabParams,
} from "@/lib/state/selectors";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

// LoraJsonEditor mounts CodeMirror, which can't render on the server.
const LoraJsonEditor = dynamic(
  () =>
    import("@loradb/lora-query").then((m) => ({
      default: m.LoraJsonEditor,
    })),
  { ssr: false },
);

interface ParamsPanelProps {
  /** Tab whose params payload this panel edits. */
  tabId: string | undefined;
  /** Editor view that hosts this panel. Drives the per-view open/size state. */
  viewId: string;
}

/**
 * Count how many of the detected `$param` names are already present
 * as top-level keys in the JSON source. Best-effort regex — bails on
 * unparseable source rather than throwing.
 */
function countFilled(
  source: string,
  detected: readonly string[],
): { filled: number; total: number } {
  const total = detected.length;
  if (total === 0 || source.trim().length === 0) {
    return { filled: 0, total };
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch {
    return { filled: 0, total };
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    return { filled: 0, total };
  }
  const obj = parsed as Record<string, unknown>;
  let n = 0;
  for (const name of detected) {
    if (name in obj && obj[name] !== undefined) n++;
  }
  return { filled: n, total };
}

/**
 * Build a skeleton payload that lists every detected `$param`,
 * preserving any values the user has already typed in. Used by the
 * "Generate skeleton" header action — a low-friction way to seed
 * the JSON without typing key by key.
 */
function buildSkeleton(source: string, detected: readonly string[]): string {
  const indent = 2;
  // Try to keep existing values where we can.
  let existing: Record<string, unknown> = {};
  try {
    const parsed = JSON.parse(source);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      existing = parsed as Record<string, unknown>;
    }
  } catch {
    /* fall through to the empty record. */
  }
  const next: Record<string, unknown> = {};
  for (const name of detected) {
    next[name] = name in existing ? existing[name] : null;
  }
  return JSON.stringify(next, null, indent);
}

export function ParamsPanel({ tabId, viewId }: ParamsPanelProps) {
  const { tokens, jsonEditor: jsonEditorTheme } = usePlaygroundTheme();
  const tab = useTabById(tabId);
  const source = useTabParams(tabId);
  const detected = useDetectedParams(tabId);
  const setParams = useStore((s) => s.setParams);
  const setParamsPanelOpenForView = useStore(
    (s) => s.setParamsPanelOpenForView,
  );

  const editorRef = useRef<LoraJsonEditorHandle | null>(null);

  const counts = useMemo(
    () => countFilled(source, detected),
    [source, detected],
  );

  const onSortKeys = useCallback(() => {
    editorRef.current?.sortKeys();
  }, []);

  const onToggleQuotes = useCallback(() => {
    editorRef.current?.toggleQuotes();
  }, []);

  const onGenerateSkeleton = useCallback(() => {
    if (!tab) return;
    if (detected.length === 0) {
      // Nothing detected — seed with a minimal example so users have
      // something concrete to edit.
      setParams(tab.id, `{\n  "userId": null\n}`);
      return;
    }
    setParams(tab.id, buildSkeleton(source, detected));
  }, [tab, source, detected, setParams]);

  const onFormat = useCallback(() => {
    void editorRef.current?.prettify();
  }, []);

  const onMinify = useCallback(() => {
    void editorRef.current?.minify();
  }, []);

  if (!tab) {
    return (
      <Box
        style={{
          flex: 1,
          minHeight: 0,
          minWidth: 0,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          color: tokens.fg.muted,
          fontSize: 12,
          background: tokens.bg.editor,
        }}
      >
        No tab open
      </Box>
    );
  }

  // Skeleton button is the "primary" affordance when bindings are
  // detected but the payload doesn't carry them yet — it surfaces the
  // happy path with a single click.
  const showSkeletonCta =
    detected.length > 0 && counts.filled < detected.length;

  return (
    <Box
      style={{
        flex: 1,
        minHeight: 0,
        minWidth: 0,
        display: "flex",
        flexDirection: "column",
        background: tokens.bg.editor,
        borderLeft: `1px solid ${tokens.border.subtle}`,
      }}
      data-testid="params-panel"
    >
      <Group
        justify="space-between"
        align="center"
        wrap="nowrap"
        gap={6}
        style={{
          padding: "4px 6px 4px 10px",
          background: tokens.bg.panel,
          borderBottom: `1px solid ${tokens.border.subtle}`,
          flexShrink: 0,
        }}
      >
        <Group gap={8} align="center" wrap="nowrap">
          <Text size="xs" fw={600} c={tokens.fg.primary} ff={tokens.font.ui}>
            Params
          </Text>
          <ParamsCountChip
            filled={counts.filled}
            total={counts.total}
            okColor={tokens.accent.success}
            mutedColor={tokens.fg.muted}
            warnColor={tokens.accent.warning}
            fontFamily={tokens.font.mono}
          />
        </Group>
        <Group gap={2} align="center" wrap="nowrap">
          <Tooltip
            label={
              showSkeletonCta
                ? `Fill in ${detected.length - counts.filled} missing param(s)`
                : "Generate a starter payload"
            }
            openDelay={400}
            withArrow
          >
            <ActionIcon
              variant={showSkeletonCta ? "light" : "subtle"}
              color={showSkeletonCta ? "blue" : "gray"}
              size="sm"
              onClick={onGenerateSkeleton}
              aria-label="Generate params skeleton"
              data-testid="params-generate-skeleton"
            >
              <IconWand size={14} />
            </ActionIcon>
          </Tooltip>
          <Menu position="bottom-end" withArrow shadow="md">
            <Menu.Target>
              <ActionIcon
                variant="subtle"
                color="gray"
                size="sm"
                aria-label="More params actions"
              >
                <IconDots size={14} />
              </ActionIcon>
            </Menu.Target>
            <Menu.Dropdown>
              <Menu.Label>Transform</Menu.Label>
              <Menu.Item
                leftSection={<IconSortAscending size={14} />}
                onClick={onSortKeys}
              >
                Sort keys A→Z
              </Menu.Item>
              <Menu.Item
                leftSection={<IconQuote size={14} />}
                onClick={onToggleQuotes}
              >
                Normalize single→double quotes
              </Menu.Item>
              <Menu.Divider />
              <Menu.Label>Format</Menu.Label>
              <Menu.Item onClick={onFormat}>Prettify (⇧⌥F)</Menu.Item>
              <Menu.Item onClick={onMinify}>Minify</Menu.Item>
            </Menu.Dropdown>
          </Menu>
          <Divider orientation="vertical" mx={2} />
          <Tooltip label="Hide Params panel" openDelay={400} withArrow>
            <ActionIcon
              variant="subtle"
              color="gray"
              size="sm"
              aria-label="Hide Params panel"
              onClick={() => setParamsPanelOpenForView(viewId, false)}
            >
              <IconChevronRight size={14} />
            </ActionIcon>
          </Tooltip>
        </Group>
      </Group>

      <Box style={{ flex: 1, minHeight: 0, display: "flex" }}>
        <LoraJsonEditor
          ref={editorRef}
          // Remount on tab switch so the editor's compartmented state
          // doesn't bleed between tabs.
          key={tab.id}
          value={source}
          onChange={(next) => setParams(tab.id, next)}
          theme={jsonEditorTheme}
          knownKeys={detected}
          allowedKeys={detected.length > 0 ? detected : undefined}
          requiredKeys={detected.length > 0 ? detected : undefined}
          formatOnPaste
          placeholder={
            detected.length === 0
              ? '{ "userId": "alice" }'
              : `{ ${detected.map((n) => `"${n}": …`).join(", ")} }`
          }
          minHeight="100%"
          style={{ flex: 1, minHeight: 0 }}
        />
      </Box>
    </Box>
  );
}

function ParamsCountChip({
  filled,
  total,
  okColor,
  mutedColor,
  warnColor,
  fontFamily,
}: {
  filled: number;
  total: number;
  okColor: string;
  mutedColor: string;
  warnColor: string;
  fontFamily: string;
}) {
  const ok = total > 0 && filled === total;
  const color = total === 0 ? mutedColor : ok ? okColor : warnColor;
  const label = total === 0 ? "none detected" : `${filled}/${total} filled`;
  return (
    <Text
      span
      ff={fontFamily}
      size="xs"
      c={color}
      title={
        total === 0
          ? "Type a $param in the query, or add a key here."
          : `${filled} of ${total} required parameters present.`
      }
    >
      {label}
    </Text>
  );
}

/**
 * Thin restore strip displayed when the Params panel is collapsed.
 * Mirrors the result-minimize affordance — clicking opens the panel.
 */
export function ParamsPanelCollapsedStrip({ viewId }: { viewId: string }) {
  const { tokens } = usePlaygroundTheme();
  const setParamsPanelOpenForView = useStore(
    (s) => s.setParamsPanelOpenForView,
  );
  return (
    <button
      type="button"
      onClick={() => setParamsPanelOpenForView(viewId, true)}
      style={{
        all: "unset",
        cursor: "pointer",
        width: 22,
        height: "100%",
        flexShrink: 0,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: 6,
        background: tokens.bg.sidebar,
        borderLeft: `1px solid ${tokens.border.subtle}`,
        color: tokens.fg.muted,
      }}
      aria-label="Show Params panel"
      title="Show Params panel"
    >
      <Text
        size="xs"
        c={tokens.fg.muted}
        ff={tokens.font.ui}
        style={{
          writingMode: "vertical-rl",
          transform: "rotate(180deg)",
          letterSpacing: 1,
        }}
      >
        Params
      </Text>
    </button>
  );
}
