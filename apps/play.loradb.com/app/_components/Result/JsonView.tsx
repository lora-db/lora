"use client";

/**
 * Read-only JSON viewer for the active result.
 *
 * Backed by `LoraJsonEditor` in read-only mode so we get:
 *   - Syntax highlighting on keys/strings/numbers/booleans/null
 *   - Fold-gutter affordances with the `{ N items }` placeholder
 *   - Search inside the result
 *   - Copy on selection + copy-all overlay
 */

import { useMemo } from "react";
import dynamic from "next/dynamic";
import { Box } from "@mantine/core";

import type { AdaptedResult } from "@/lib/db/types";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

const LoraJsonEditor = dynamic(
  () =>
    import("@loradb/lora-query").then((m) => ({
      default: m.LoraJsonEditor,
    })),
  { ssr: false },
);

export function JsonView({ result }: { result: AdaptedResult }) {
  const { tokens, jsonEditor: jsonEditorTheme } = usePlaygroundTheme();

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

  return (
    <Box style={{ flex: 1, minHeight: 0, display: "flex", background: tokens.bg.editor }}>
      <LoraJsonEditor
        value={text}
        readOnly
        theme={jsonEditorTheme}
        showLineNumbers={false}
        minHeight="100%"
        style={{ flex: 1, minHeight: 0 }}
      />
    </Box>
  );
}
