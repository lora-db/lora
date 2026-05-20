"use client";

/**
 * `UsageExamples` — small read-only stack of Cypher snippets shown on
 * the wizard's confirmation step. Each snippet is auto-prettified
 * through the WASM formatter and rendered with the standard editor
 * for syntax highlighting parity with `DDLPreview`.
 */

import { Box, Paper, Stack, Text } from "@mantine/core";
import { LoraQueryEditor, formatSync } from "@loradb/lora-query";

import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import type { UsageExample } from "@/lib/schemaDesign/examples";

interface UsageExamplesProps {
  /** Heading shown above the list. */
  caption?: string;
  /** Ordered list of snippets. Render order matches array order. */
  examples: readonly UsageExample[];
}

function prettify(source: string): string {
  try {
    return formatSync(source);
  } catch {
    return source;
  }
}

export function UsageExamples({ caption, examples }: UsageExamplesProps) {
  const { tokens, editor } = usePlaygroundTheme();
  if (examples.length === 0) return null;

  return (
    <Paper withBorder p="xs">
      <Text size="xs" c={tokens.fg.muted} tt="uppercase" fw={600} mb={6}>
        {caption ?? "Queries this accelerates"}
      </Text>
      <Stack gap={8}>
        {examples.map((ex) => (
          <Stack gap={2} key={ex.caption}>
            <Text size="xs" c={tokens.fg.muted}>
              {ex.caption}
            </Text>
            <Box
              style={{
                border: `1px solid ${tokens.border.subtle}`,
                borderRadius: tokens.radius.sm,
                overflow: "hidden",
              }}
            >
              <LoraQueryEditor
                value={prettify(ex.cypher)}
                readOnly
                theme={editor}
                showLineNumbers={false}
                minHeight="32px"
                maxHeight="120px"
              />
            </Box>
          </Stack>
        ))}
      </Stack>
    </Paper>
  );
}
