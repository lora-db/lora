"use client";

/**
 * Placeholder rendered when there's no run on the active tab yet.
 */

import { Center, Stack, Text } from "@mantine/core";
import { IconPlayerPlay } from "@tabler/icons-react";

import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

export function EmptyResult() {
  const { tokens } = usePlaygroundTheme();
  return (
    <Center h="100%" style={{ background: tokens.bg.editor }}>
      <Stack align="center" gap={8}>
        <IconPlayerPlay size={28} color={tokens.fg.subtle} />
        <Text size="sm" c={tokens.fg.muted}>
          No results yet
        </Text>
        <Text size="xs" c={tokens.fg.subtle}>
          Press ⌘↵ to run the current query
        </Text>
      </Stack>
    </Center>
  );
}
