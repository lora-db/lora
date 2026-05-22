"use client";

/**
 * Result view (Graph / Table / JSON / Plan). Prop-driven so multiple
 * panes can each show a different inner tab for the same underlying
 * result. The Plan tab always shows the parser's view of the editor
 * body — it works even before the query has run, so the Tabs frame is
 * mounted in every state (with the data tabs disabled/replaced as
 * needed by the underlying outcome).
 */

import { useEffect, useRef, useState } from "react";
import {
  ActionIcon,
  Box,
  Button,
  Center,
  Group,
  Loader,
  Menu,
  Stack,
  Tabs,
  Text,
  Tooltip,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { IconCamera, IconDownload, IconFileTypeCsv } from "@tabler/icons-react";
import type { LoraParams, RowFormat } from "@loradb/lora-wasm";

import { requestGraphPng } from "@/lib/actions/exportActions";
import {
  exportRowsStream,
  rowFormatExtension,
  rowFormatMimeType,
} from "@/lib/db/client";
import type { AdaptedResult, RunOk } from "@/lib/db/types";
import { useActiveTab, useTabById, useViewResult } from "@/lib/state/selectors";
import { useStore } from "@/lib/state/store";
import type { PanelView, ResultTab } from "@/lib/state/slices/layout";
import type { Tokens } from "@/lib/theme/tokens";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

import { EmptyResult } from "./EmptyResult";
import { ErrorView } from "./ErrorView";
import { GraphView } from "./GraphView";
import { JsonView } from "./JsonView";
import { PlanView } from "./PlanView";
import { TableView } from "./TableView";

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

/**
 * Pick a token colour for the elapsed-ms stat. Thresholds are tuned
 * for in-process WASM where most queries finish in tens of ms.
 */
function speedColor(ms: number, tokens: Tokens): string {
  if (ms < 50) return tokens.accent.success;
  if (ms < 200) return tokens.accent.warning;
  return tokens.accent.danger;
}

export interface ResultPaneProps {
  view: PanelView;
  paneId: string;
}

export function ResultPane({ view, paneId }: ResultPaneProps) {
  const { tokens } = usePlaygroundTheme();
  const resultTab: ResultTab = view.resultTab ?? "graph";
  const activeTab = useActiveTab();
  const pinnedTab = useTabById(view.tabId);
  const tab = view.tabId === undefined ? activeTab : pinnedTab;
  const tabId = tab?.id ?? null;
  const result = useViewResult(view);
  const setResultTabForView = useStore((s) => s.setResultTabForView);
  const clearResult = useStore((s) => s.clearResult);

  // If the active result has no graph data but the pane is parked on
  // the "graph" tab, slide it over to "table" automatically.
  const hasGraph =
    result !== undefined &&
    result.state === "ok" &&
    result.result.graph !== null;
  useEffect(() => {
    if (resultTab === "graph" && result?.state === "ok" && !hasGraph) {
      setResultTabForView(view.id, "table");
    }
  }, [resultTab, result, hasGraph, setResultTabForView, view.id]);

  // First successful run of a tab in this pane: if the result has
  // graph data, jump to the graph tab even when this pane's last
  // selection was table/json/plan. Subsequent runs of the same tab
  // respect whatever the user picked. We key on tabId (not runId) so
  // re-running the same query doesn't override the user's manual
  // choice, and we use a ref instead of state so the effect doesn't
  // re-fire on the same render that triggers the update.
  const seenTabsRef = useRef<Set<string>>(new Set());
  useEffect(() => {
    if (tabId === null) return;
    if (result?.state !== "ok") return;
    if (seenTabsRef.current.has(tabId)) return;
    seenTabsRef.current.add(tabId);
    if (hasGraph && resultTab !== "graph") {
      setResultTabForView(view.id, "graph");
    }
  }, [tabId, result, hasGraph, resultTab, view.id, setResultTabForView]);

  if (!result) {
    return <EmptyResult />;
  }

  if (result.state === "running") {
    return (
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
                if (tabId !== null) clearResult(tabId);
              }}
            >
              Cancel
            </Button>
          </Tooltip>
        </Stack>
      </Center>
    );
  }

  if (result.state === "error") {
    return <ErrorView outcome={result} />;
  }

  // Cast is sound because we've narrowed to RunOk above.
  const ok: RunOk = result;

  return (
    <Tabs
      value={resultTab}
      onChange={(v) => {
        if (v === "graph" || v === "table" || v === "json" || v === "plan") {
          setResultTabForView(view.id, v);
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
                  requestGraphPng(paneId);
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
          {tab && ok.result.rows.length > 0 && (
            <ExportMenu
              tabName={tab.name}
              body={tab.body}
              params={tab.params}
            />
          )}
          <Text size="xs" c={tokens.fg.subtle} ff={tokens.font.mono}>
            <Text span inherit c={tokens.category.node} fw={600}>
              {ok.result.stats.nodeCount}
            </Text>{" "}
            nodes ·{" "}
            <Text span inherit c={tokens.category.relationship} fw={600}>
              {ok.result.stats.relCount}
            </Text>{" "}
            rels ·{" "}
            <Text span inherit c={tokens.fg.primary} fw={600}>
              {ok.result.stats.rowCount}
            </Text>{" "}
            rows ·{" "}
            <Text span inherit c={speedColor(ok.ms, tokens)} fw={600}>
              {ok.ms}ms
            </Text>
          </Text>
        </Group>
      </Tabs.List>

      <Tabs.Panel value="graph" style={panelStyle}>
        <Box style={fillStyle}>
          {/* `key={ok.runId}` remounts the canvas on every new query
           * run so its uncontrolled `defaultData` seed re-applies.
           * In between, local edits (delete / add / move) stay put. */}
          <GraphView key={ok.runId} result={ok.result} paneId={paneId} />
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
          <PlanView tabId={view.tabId} />
        </Box>
      </Tabs.Panel>
    </Tabs>
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

interface ExportMenuProps {
  tabName: string;
  body: string;
  /** Raw JSON string from the tab; same shape the Run action parses. */
  params: string;
}

/**
 * Re-runs the current tab's query through the WASM `exportRows`
 * pipeline and triggers a Blob download in the chosen format. Using
 * the native Rust encoders preserves tagged temporal / vector /
 * point values that a JS-side CSV writer would flatten.
 *
 * Re-running on export (rather than re-encoding the already-loaded
 * result) means edits to the editor body after the last run are
 * reflected — predictable: the menu always exports "what the editor
 * currently says."
 */
function ExportMenu({ tabName, body, params }: ExportMenuProps) {
  const [busy, setBusy] = useState(false);

  const trigger = async (format: RowFormat) => {
    if (busy) return;
    setBusy(true);
    try {
      const parsedParams = parseTabParams(params);
      const filename = `${sanitizeFilename(tabName)}.${rowFormatExtension(format)}`;
      const mime = rowFormatMimeType(format);

      // Streaming export: each chunk hops the worker boundary and
      // lands either directly on disk (File System Access API,
      // Chromium-family browsers) or in a fallback Blob that grows
      // one chunk at a time before triggering a normal download.
      // The Rust + WASM side never materialises the full payload —
      // only one chunk is in flight at any moment.
      const stream = await exportRowsStream(body, parsedParams, format);
      const stats = await drainExportStream(stream, filename, mime);

      notifications.show({
        color: "green",
        title: `Exported ${stats.rows} ${stats.rows === 1 ? "row" : "rows"}`,
        message: `Saved as ${format.toUpperCase()} (${formatBytes(stats.bytes)}).`,
      });
    } catch (err) {
      notifications.show({
        color: "red",
        title: "Export failed",
        message: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Menu shadow="md" position="bottom-end" withinPortal>
      <Menu.Target>
        <Tooltip label="Download result as JSONL / JSON / CSV" openDelay={400}>
          <ActionIcon
            size="sm"
            variant="subtle"
            color="gray"
            aria-label="Export result"
            loading={busy}
          >
            <IconDownload size={14} />
          </ActionIcon>
        </Tooltip>
      </Menu.Target>
      <Menu.Dropdown>
        <Menu.Label>Download result</Menu.Label>
        <Menu.Item onClick={() => void trigger("jsonl")}>
          JSON Lines (.jsonl)
        </Menu.Item>
        <Menu.Item onClick={() => void trigger("json")}>JSON (.json)</Menu.Item>
        <Menu.Item onClick={() => void trigger("csv")}>CSV (.csv)</Menu.Item>
      </Menu.Dropdown>
    </Menu>
  );
}

function parseTabParams(raw: string): LoraParams | undefined {
  const trimmed = raw.trim();
  if (trimmed.length === 0 || trimmed === "{}") return undefined;
  try {
    const v = JSON.parse(trimmed) as unknown;
    // The bridge accepts any structured-cloneable object as params;
    // anything malformed will be surfaced by the engine itself with
    // a clearer error than a JSON parse failure here.
    return v && typeof v === "object" && !Array.isArray(v)
      ? (v as LoraParams)
      : undefined;
  } catch {
    return undefined;
  }
}

function downloadBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  // Revoke after a short delay so Safari/Firefox have a chance to
  // actually fetch the blob before the URL is invalidated.
  setTimeout(() => URL.revokeObjectURL(url), 1_000);
}

interface ExportRunStats {
  /** Total bytes shipped through the stream. */
  bytes: number;
  /**
   * Approximate row count. JSONL-and-JSON chunks contain one row per
   * `\n` separator; CSV chunks contain rows minus the header. The
   * encoder doesn't emit a row count along with the bytes, so we
   * tally newlines as a useful approximation for the toast message.
   */
  rows: number;
}

/**
 * Drain a `ReadableStream<Uint8Array>` returned by the WASM streaming
 * exporter to disk (or to a Blob fallback) without ever buffering the
 * entire payload.
 *
 * Tries the File System Access API first — that's the only path that
 * lets the encoded bytes go straight from the worker to the file
 * handle without sitting in main-thread JS memory. When the API
 * isn't available (Safari, Firefox without flag, some embedded
 * webviews), we fall back to a `Blob` constructed from the streamed
 * chunks. The fallback still pulls one chunk at a time — so the
 * peak resident set is one chunk plus the growing Blob backing
 * (which the browser is free to spill to disk).
 */
async function drainExportStream(
  stream: ReadableStream<Uint8Array>,
  filename: string,
  mimeType: string,
): Promise<ExportRunStats> {
  const counted = countingTransform(stream);
  const fs = (
    window as unknown as {
      showSaveFilePicker?: (
        opts: ShowSaveFilePickerOptions,
      ) => Promise<FileSystemFileHandle>;
    }
  ).showSaveFilePicker;

  if (typeof fs === "function") {
    try {
      const handle = await fs({
        suggestedName: filename,
        types: [
          {
            description: `${mimeType.split("/")[1] ?? "data"} file`,
            accept: { [mimeType]: [`.${filename.split(".").pop() ?? "txt"}`] },
          },
        ],
      });
      const writable = await handle.createWritable();
      // `pipeTo` drives the source's `pull` one chunk at a time,
      // backpressuring when the writable can't keep up. Bytes hop
      // worker→main→disk one chunk per round trip.
      await counted.stream.pipeTo(writable);
      return counted.stats();
    } catch (err) {
      // User cancelled the save dialog or the API is gated; fall
      // through to the Blob path. AbortError is the expected
      // cancel; anything else we surface so the toast carries it.
      if ((err as { name?: string } | null)?.name !== "AbortError") {
        throw err;
      }
      // User cancelled — release the cursor and pretend nothing
      // happened (no toast about success).
      await counted.stream.cancel();
      return { bytes: 0, rows: 0 };
    }
  }

  // Blob fallback: still streams chunks (no full Vec<u8> in WASM
  // memory), but the browser-side Blob constructor accumulates them.
  // Modern browsers spill large Blobs to disk so this isn't as bad
  // as it sounds for big exports — but the File System Access path
  // is strictly better when available.
  const blob = await new Response(counted.stream).blob();
  downloadBlob(new Blob([blob], { type: mimeType }), filename);
  return counted.stats();
}

/**
 * Wrap a chunk stream with a passive observer that tallies the byte
 * count and an approximate row count (one row per `\n`). Returns the
 * wrapped stream and a closure that reports stats after the stream
 * has been drained.
 */
function countingTransform(source: ReadableStream<Uint8Array>): {
  stream: ReadableStream<Uint8Array>;
  stats: () => ExportRunStats;
} {
  let bytes = 0;
  let newlines = 0;
  const stream = source.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      transform(chunk, controller) {
        bytes += chunk.byteLength;
        for (let i = 0; i < chunk.byteLength; i += 1) {
          if (chunk[i] === 0x0a) newlines += 1;
        }
        controller.enqueue(chunk);
      },
    }),
  );
  // Drop the header newline for CSV (best-effort: the encoder always
  // emits a header line before any data rows; subtracting one keeps
  // the toast count human-friendly). JSON-array's bracket lines also
  // contribute small overcounts that we accept rather than parsing
  // the format on the JS side.
  return { stream, stats: () => ({ bytes, rows: Math.max(0, newlines - 1) }) };
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
}

interface ShowSaveFilePickerOptions {
  suggestedName?: string;
  types?: Array<{
    description?: string;
    accept: Record<string, string[]>;
  }>;
}

interface FileSystemFileHandle {
  createWritable(): Promise<WritableStream<Uint8Array>>;
}

function sanitizeFilename(name: string): string {
  const cleaned = name
    .replace(/[^A-Za-z0-9._-]+/g, "_")
    .replace(/^_+|_+$/g, "");
  return cleaned.length > 0 ? cleaned : "lora-result";
}
