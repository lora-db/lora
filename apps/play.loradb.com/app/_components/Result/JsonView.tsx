"use client";

/**
 * Plain text dump of the adapted rows, paired with column names so each
 * row reads like `{ "n": {...}, "m": {...} }`. Includes a small "Copy"
 * button at the top right.
 */

import { useMemo, useState } from "react";
import { ActionIcon, Box, ScrollArea, Tooltip } from "@mantine/core";
import { IconCheck, IconCopy } from "@tabler/icons-react";

import type { AdaptedResult } from "@/lib/db/types";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

export function JsonView({ result }: { result: AdaptedResult }) {
  const { tokens } = usePlaygroundTheme();
  const [copied, setCopied] = useState(false);

  const text = useMemo(() => {
    const rows = result.rows.map((row) => {
      const obj: Record<string, unknown> = {};
      for (let i = 0; i < result.columns.length; i++) {
        const col = result.columns[i];
        if (col === undefined) continue;
        obj[col] = row.values[i];
      }
      return obj;
    });
    return JSON.stringify(rows, null, 2);
  }, [result]);

  const onCopy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1200);
    } catch {
      /* clipboard refused — silent no-op */
    }
  };

  return (
    <Box style={{ position: "relative", height: "100%", background: tokens.bg.editor }}>
      <Tooltip label={copied ? "Copied" : "Copy"} withArrow position="left">
        <ActionIcon
          variant="subtle"
          size="sm"
          color="gray"
          onClick={() => void onCopy()}
          aria-label="Copy JSON to clipboard"
          style={{
            position: "absolute",
            top: 8,
            right: 12,
            zIndex: 2,
          }}
        >
          {copied ? <IconCheck size={14} /> : <IconCopy size={14} />}
        </ActionIcon>
      </Tooltip>
      <ScrollArea h="100%" w="100%" type="auto">
        <pre
          style={{
            margin: 0,
            padding: "12px 16px",
            fontFamily: tokens.font.mono,
            fontSize: 12,
            color: tokens.fg.primary,
            background: "transparent",
            whiteSpace: "pre",
          }}
        >
          {text}
        </pre>
      </ScrollArea>
    </Box>
  );
}
