"use client";

/**
 * Wraps `LoraQueryEditor` (dynamically imported, client-only). Pipes
 * the active tab's body through the store and registers the `⌘↵`
 * keybind to run the active query. Also fans the cached
 * {@link SchemaSnapshot} into the editor's completion providers and
 * starts the post-mutation schema-refresh listener.
 */

import { useCallback, useEffect, useMemo } from "react";
import { Center, Stack, Text } from "@mantine/core";
import { useHotkeys } from "@mantine/hooks";

import { LoraQueryEditor } from "@loradb/lora-query";
import type { PropertyContext } from "@loradb/lora-query";
import { useActiveTab } from "@/lib/state/selectors";
import { useStore } from "@/lib/state/store";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import { runActiveTab } from "@/lib/actions/runActiveTab";
import {
  attachSchemaMutationListener,
  refreshSchema,
} from "@/lib/actions/schemaActions";

export function EditorPane() {
  const tab = useActiveTab();
  const schema = useStore((s) => s.schema);
  const { editor, tokens } = usePlaygroundTheme();

  useHotkeys([
    [
      "mod+Enter",
      () => {
        void runActiveTab();
      },
    ],
  ]);

  // Kick off the first introspection and subscribe to subsequent
  // mutation events. Both run exactly once across the lifetime of the
  // mounted pane (StrictMode double-mount is absorbed by the
  // schema slice — `refreshSchema` is idempotent).
  useEffect(() => {
    void refreshSchema();
    return attachSchemaMutationListener();
  }, []);

  // Pull the schema into stable arrays for the editor's completion
  // providers. The editor reconfigures its provider Facet whenever
  // these identities change, so we only rebuild on real updates.
  const labels = useMemo(
    () => schema?.labels ?? [],
    [schema?.labels],
  );
  const relTypes = useMemo(
    () => schema?.relTypes ?? [],
    [schema?.relTypes],
  );

  // The editor's `getPropertyKeys` is the property-map completer.
  // We narrow the suggestion list by the surrounding label (or
  // rel-type) when the editor can identify one — otherwise we fall
  // back to the union of all known property keys so the user still
  // gets useful completions in ambiguous positions.
  const getPropertyKeys = useCallback(
    (ctx: PropertyContext): readonly string[] => {
      if (!schema) return [];
      const keys = new Set<string>();
      if (ctx.label !== null) {
        const bucket =
          ctx.kind === "relationship"
            ? schema.propertiesByRelType[ctx.label]
            : schema.propertiesByLabel[ctx.label];
        for (const k of bucket ?? []) keys.add(k);
      }
      if (keys.size === 0) {
        for (const k of schema.propertyKeys) keys.add(k);
      }
      return Array.from(keys).sort();
    },
    [schema],
  );

  if (!tab) {
    return (
      <Center h="100%" style={{ background: tokens.bg.editor }}>
        <Stack align="center" gap={4}>
          <Text size="sm" c={tokens.fg.muted}>
            No editor tab open
          </Text>
          <Text size="xs" c={tokens.fg.subtle}>
            Click + to start a new query
          </Text>
        </Stack>
      </Center>
    );
  }

  return (
    <div
      style={{
        flex: 1,
        minHeight: 0,
        display: "flex",
        flexDirection: "column",
        background: tokens.bg.editor,
      }}
    >
      <LoraQueryEditor
        key={tab.id}
        value={tab.body}
        onChange={(next) => useStore.getState().setBody(tab.id, next)}
        onRun={() => {
          void runActiveTab();
        }}
        theme={editor}
        labels={labels}
        relTypes={relTypes}
        getPropertyKeys={getPropertyKeys}
        minHeight="100%"
        placeholder="-- Type Cypher here, then press ⌘↵ to run"
        style={{ flex: 1, minHeight: 0 }}
      />
    </div>
  );
}
