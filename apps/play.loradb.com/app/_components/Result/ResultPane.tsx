"use client";

/**
 * The bottom half of the workbench. Switches between graph / table /
 * JSON / plan for the active tab's result, or shows an empty /
 * running / error state.
 *
 * The Plan tab always shows the parser's view of the active editor
 * body — it works even before the query has run, so the Tabs frame is
 * mounted in every state (with the data tabs disabled/replaced as
 * needed by the underlying outcome).
 */

import { useEffect } from "react";
import {
  ActionIcon,
  Box,
  Button,
  Center,
  Group,
  Loader,
  Stack,
  Tabs,
  Text,
  Tooltip,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { IconCamera, IconFileTypeCsv } from "@tabler/icons-react";

import { requestGraphPng } from "@/lib/actions/exportActions";
import type { AdaptedResult, RunOk } from "@/lib/db/types";
import { useActiveResult, useResultTab } from "@/lib/state/selectors";
import { useStore } from "@/lib/state/store";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

import { EmptyResult } from "./EmptyResult";
import { ErrorView } from "./ErrorView";
import { GraphView } from "./GraphView";
import { JsonView } from "./JsonView";
import { PlanView } from "./PlanView";
import { TableView } from "./TableView";
import { InspectorDrawer } from "../Inspector/InspectorDrawer";

/**
 * Convert an AdaptedResult into RFC-4180-ish CSV. Strings containing
 * commas, quotes or newlines are quoted with embedded quotes doubled.
 * Complex cell values (nodes/relationships/paths/arrays/objects) round-
 * trip through `JSON.stringify` so the export is lossless.
 */
function toCsv(result: AdaptedResult): string {
  const escape = (raw: string): string => {
    if (raw.length === 0) return "";
    if (/[",\r\n]/.test(raw)) {
      return `"${raw.replace(/"/g, '""')}"`;
    }
    return raw;
  };
  const stringify = (value: unknown): string => {
    if (value === null || value === undefined) return "";
    if (typeof value === "string") return value;
    if (typeof value === "number" || typeof value === "boolean") {
      return String(value);
    }
    try {
      return JSON.stringify(value);
    } catch {
      return String(value);
    }
  };
  const lines: string[] = [];
  lines.push(result.columns.map(escape).join(","));
  for (const row of result.rows) {
    lines.push(row.values.map((v) => escape(stringify(v))).join(","));
  }
  return lines.join("\n");
}

async function copyCsv(result: AdaptedResult): Promise<void> {
  if (typeof navigator === "undefined" || !navigator.clipboard) {
    notifications.show({
      color: "red",
      title: "Clipboard unavailable",
      message: "Your browser does not expose a clipboard API.",
    });
    return;
  }
  try {
    await navigator.clipboard.writeText(toCsv(result));
    notifications.show({
      color: "green",
      title: "Copied CSV",
      message: `${result.rows.length} ${result.rows.length === 1 ? "row" : "rows"} on the clipboard.`,
    });
  } catch (err) {
    notifications.show({
      color: "red",
      title: "Copy failed",
      message: err instanceof Error ? err.message : String(err),
    });
  }
}

export function ResultPane() {
  const { tokens } = usePlaygroundTheme();
  const result = useActiveResult();
  const activeId = useStore((s) => s.activeTabId);
  const resultTab = useResultTab();
  const setResultTab = useStore((s) => s.setResultTab);
  const clearResult = useStore((s) => s.clearResult);

  // If the active result has no graph data but the user is parked on the
  // "graph" tab, slide them over to "table" automatically.
  const hasGraph =
    result !== undefined && result.state === "ok" && result.result.graph !== null;
  useEffect(() => {
    if (resultTab === "graph" && result?.state === "ok" && !hasGraph) {
      setResultTab("table");
    }
  }, [resultTab, result, hasGraph, setResultTab]);

  if (!result) {
    return (
      <>
        <EmptyResult />
        <InspectorDrawer />
      </>
    );
  }

  if (result.state === "running") {
    return (
      <>
        <Center h="100%" style={{ background: tokens.bg.editor }}>
          <Stack align="center" gap={12}>
            <Loader size="sm" />
            <Text size="sm" c={tokens.fg.muted}>
              Running…
            </Text>
            <Tooltip
              label="Drops the result on the floor. The WASM query keeps running in the background until it finishes, but the workbench will ignore its output."
              multiline
              w={260}
              withArrow
              openDelay={400}
            >
              <Button
                variant="subtle"
                color="gray"
                size="xs"
                onClick={() => {
                  // The WASM `execute` call has no abort signal yet, so
                  // clearing the running marker is the best we can do.
                  // `runActiveTab` re-reads the marker after the await
                  // and drops the outcome when it's no longer there,
                  // which means the user can immediately kick off a
                  // fresh run without the old one stomping on it.
                  if (activeId !== null) clearResult(activeId);
                }}
              >
                Cancel
              </Button>
            </Tooltip>
          </Stack>
        </Center>
        <InspectorDrawer />
      </>
    );
  }

  if (result.state === "error") {
    return (
      <>
        <ErrorView outcome={result} />
        <InspectorDrawer />
      </>
    );
  }

  // Cast is sound because we've narrowed to RunOk above.
  const ok: RunOk = result;

  return (
    <>
      <Tabs
        value={resultTab}
        onChange={(v) => {
          if (v === "graph" || v === "table" || v === "json" || v === "plan") {
            setResultTab(v);
          }
        }}
        variant="default"
        keepMounted={false}
        style={{
          display: "flex",
          flexDirection: "column",
          height: "100%",
          background: tokens.bg.editor,
        }}
      >
        <Tabs.List
          style={{
            background: tokens.bg.panel,
            borderBottom: `1px solid ${tokens.border.subtle}`,
            paddingLeft: 6,
            flexShrink: 0,
          }}
        >
          <Tabs.Tab value="graph" disabled={!hasGraph}>
            Graph
          </Tabs.Tab>
          <Tabs.Tab value="table">Table</Tabs.Tab>
          <Tabs.Tab value="json">JSON</Tabs.Tab>
          <Tabs.Tab value="plan">Plan</Tabs.Tab>
          <Group ml="auto" pr="md" align="center" gap={8}>
            {resultTab === "graph" && hasGraph && (
              <Tooltip label="Export graph as PNG" openDelay={400}>
                <ActionIcon
                  size="sm"
                  variant="subtle"
                  color="gray"
                  aria-label="Export graph as PNG"
                  onClick={() => {
                    requestGraphPng();
                  }}
                >
                  <IconCamera size={14} />
                </ActionIcon>
              </Tooltip>
            )}
            {ok.result.rows.length > 0 && (
              <Tooltip label="Copy result as CSV" openDelay={400}>
                <ActionIcon
                  size="sm"
                  variant="subtle"
                  color="gray"
                  aria-label="Copy result as CSV"
                  onClick={() => {
                    void copyCsv(ok.result);
                  }}
                >
                  <IconFileTypeCsv size={14} />
                </ActionIcon>
              </Tooltip>
            )}
            <Text size="xs" c={tokens.fg.subtle} ff={tokens.font.mono}>
              {ok.result.stats.nodeCount} nodes · {ok.result.stats.relCount} rels ·{" "}
              {ok.result.stats.rowCount} rows · {ok.ms}ms
            </Text>
          </Group>
        </Tabs.List>

        <Tabs.Panel value="graph" style={panelStyle}>
          <Box style={fillStyle}>
            {/* `key={ok.runId}` remounts the canvas on every new query
              * run so its uncontrolled `defaultData` seed re-applies.
              * In between, local edits (delete / add / move) stay put. */}
            <GraphView key={ok.runId} result={ok.result} />
          </Box>
        </Tabs.Panel>
        <Tabs.Panel value="table" style={panelStyle}>
          <Box style={fillStyle}>
            <TableView result={ok.result} />
          </Box>
        </Tabs.Panel>
        <Tabs.Panel value="json" style={panelStyle}>
          <Box style={fillStyle}>
            <JsonView result={ok.result} />
          </Box>
        </Tabs.Panel>
        <Tabs.Panel value="plan" style={panelStyle}>
          <Box style={fillStyle}>
            <PlanView />
          </Box>
        </Tabs.Panel>
      </Tabs>
      <InspectorDrawer />
    </>
  );
}

const panelStyle = {
  flex: 1,
  minHeight: 0,
  display: "flex",
  flexDirection: "column" as const,
};

const fillStyle = {
  flex: 1,
  minHeight: 0,
  display: "flex",
  flexDirection: "column" as const,
  width: "100%",
  height: "100%",
};
