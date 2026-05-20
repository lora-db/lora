"use client";

/**
 * Renders a `RunErr` outcome as a Mantine `Alert`. The leading semantic
 * line is shown in monospace by default; users can expand to see the
 * raw multi-line message (parser stack, internal frames, etc.) on
 * demand. A "Copy error" button puts the full payload on the clipboard
 * in one click.
 */

import { useState } from "react";
import {
  ActionIcon,
  Alert,
  Code,
  Collapse,
  Group,
  ScrollArea,
  Stack,
  Text,
  Tooltip,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import {
  IconAlertTriangle,
  IconChevronDown,
  IconChevronRight,
  IconCopy,
} from "@tabler/icons-react";

import type { RunOutcome } from "@/lib/db/types";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

/**
 * Returns the first non-empty, non-noisy line of `message`. We treat
 * common Rust/WASM panic prefixes ("Error: ", "RuntimeError: ", etc.)
 * as scaffolding and skip them so the user lands on the substantive
 * description.
 */
function semanticHead(message: string): string {
  const lines = message.split(/\r?\n/);
  for (const raw of lines) {
    const line = raw.trim();
    if (line.length === 0) continue;
    // Strip a single layer of common error-prefix wrappers.
    const stripped = line.replace(
      /^(?:Error|RuntimeError|TypeError|SyntaxError|LoraError|Caused by)\s*:\s*/i,
      "",
    );
    if (stripped.length > 0) return stripped;
  }
  return message.trim() || "Unknown error";
}

export function ErrorView({ outcome }: { outcome: RunOutcome }) {
  const { tokens } = usePlaygroundTheme();
  const [showFull, setShowFull] = useState(false);

  if (outcome.state !== "error") return null;

  const head = semanticHead(outcome.message);
  const hasMore = head !== outcome.message.trim();

  const handleCopy = (): void => {
    if (typeof navigator === "undefined" || !navigator.clipboard) return;
    navigator.clipboard
      .writeText(outcome.message)
      .then(() => {
        notifications.show({
          color: "green",
          title: "Copied",
          message: "Error message copied to clipboard.",
        });
      })
      .catch((err: unknown) => {
        notifications.show({
          color: "red",
          title: "Copy failed",
          message: err instanceof Error ? err.message : String(err),
        });
      });
  };

  return (
    <ScrollArea h="100%" style={{ background: tokens.bg.editor }}>
      <Stack p="md" gap="sm">
        <Alert
          variant="light"
          color="red"
          icon={<IconAlertTriangle size={16} />}
          title={
            <Group justify="space-between" wrap="nowrap" gap="xs">
              <Text fw={600} size="sm">
                Query failed
              </Text>
              <Tooltip label="Copy full error" withArrow>
                <ActionIcon
                  size="sm"
                  variant="subtle"
                  color="gray"
                  onClick={handleCopy}
                  aria-label="Copy error message"
                >
                  <IconCopy size={14} />
                </ActionIcon>
              </Tooltip>
            </Group>
          }
        >
          <Code
            block
            style={{
              background: "transparent",
              whiteSpace: "pre-wrap",
              color: tokens.fg.primary,
            }}
          >
            {head}
          </Code>
          {outcome.position && (
            <Text size="xs" c={tokens.fg.muted} mt="xs" ff={tokens.font.mono}>
              line {outcome.position.line}, column {outcome.position.col}
            </Text>
          )}
          {hasMore && (
            <>
              <Group gap={4} mt="xs">
                <ActionIcon
                  size="xs"
                  variant="subtle"
                  color="gray"
                  onClick={() => setShowFull((v) => !v)}
                  aria-label={showFull ? "Hide full error" : "Show full error"}
                  aria-expanded={showFull}
                >
                  {showFull ? (
                    <IconChevronDown size={12} />
                  ) : (
                    <IconChevronRight size={12} />
                  )}
                </ActionIcon>
                <Text
                  size="xs"
                  c={tokens.fg.muted}
                  style={{ cursor: "pointer", userSelect: "none" }}
                  onClick={() => setShowFull((v) => !v)}
                >
                  {showFull ? "Hide details" : "Show full error"}
                </Text>
              </Group>
              <Collapse in={showFull}>
                <Code
                  block
                  mt="xs"
                  style={{
                    background: tokens.bg.panel,
                    whiteSpace: "pre-wrap",
                    color: tokens.fg.muted,
                    fontSize: 11,
                  }}
                >
                  {outcome.message}
                </Code>
              </Collapse>
            </>
          )}
        </Alert>
      </Stack>
    </ScrollArea>
  );
}
