"use client";

import {
  Accordion,
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Center,
  Checkbox,
  Code,
  FileButton,
  Group,
  Loader,
  NumberInput,
  Progress,
  ScrollArea,
  Select,
  Stack,
  Table,
  TagsInput,
  Text,
  Textarea,
  TextInput,
  ThemeIcon,
} from "@mantine/core";
import {
  IconAlertCircle,
  IconArrowsRightLeft,
  IconCheck,
  IconCircleX,
  IconCode,
  IconCpu,
  IconHexagon,
  IconUpload,
  IconX,
} from "@tabler/icons-react";
import type {
  RowFormat,
  RowImportProgress,
  RowParseError,
} from "@loradb/lora-wasm";

import {
  formatBytes,
  renderSampleCell,
  truncate,
  type MappingKind,
  type PreviewState,
} from "@/lib/importData/rowImport";
import {
  formatDuration,
  formatRate,
  formatRowsPerSecond,
  type ThroughputReading,
} from "@/lib/util/throughput";

const CSV_TYPE_CHOICES = [
  { value: "auto", label: "auto" },
  { value: "string", label: "string" },
  { value: "int", label: "int" },
  { value: "long", label: "long" },
  { value: "float", label: "float" },
  { value: "double", label: "double" },
  { value: "bool", label: "bool" },
  { value: "date", label: "date" },
  { value: "datetime", label: "datetime" },
  { value: "localdatetime", label: "localdatetime" },
  { value: "time", label: "time" },
  { value: "localtime", label: "localtime" },
  { value: "duration", label: "duration" },
  { value: "point", label: "point" },
  { value: "json", label: "json" },
];

// ---------------------------------------------------------------------------
// Step: File
// ---------------------------------------------------------------------------

export function FileStep({
  file,
  format,
  preview,
  previewError,
  onPickFile,
}: {
  file: File | null;
  format: RowFormat;
  preview: PreviewState | null;
  previewError: string | null;
  onPickFile: (f: File | null) => void;
}) {
  return (
    <Stack gap="md" pt="md">
      <Group justify="space-between" align="end" wrap="nowrap">
        <Stack gap={4} style={{ flex: 1 }}>
          <Text size="xs" c="dimmed">
            Source file
          </Text>
          {file ? (
            <Group gap="xs" wrap="nowrap" align="center">
              <Text size="sm" fw={500} style={{ wordBreak: "break-all" }}>
                {file.name}
              </Text>
              <Badge variant="light" color="gray">
                {formatBytes(file.size)}
              </Badge>
              <Badge variant="light" color="blue">
                {format.toUpperCase()}
              </Badge>
            </Group>
          ) : (
            <Text size="sm" c="dimmed">
              No file selected — pick one or drop a .csv / .jsonl / .json file
              anywhere on the page.
            </Text>
          )}
        </Stack>
        <FileButton accept=".jsonl,.ndjson,.json,.csv" onChange={onPickFile}>
          {(props) => (
            <Button
              {...props}
              variant="default"
              leftSection={<IconUpload size={14} />}
            >
              Choose file
            </Button>
          )}
        </FileButton>
      </Group>

      {previewError && (
        <Alert color="red" icon={<IconAlertCircle size={14} />}>
          Couldn&apos;t sniff this file: {previewError}
        </Alert>
      )}

      {preview && file && (
        <Stack gap={4}>
          <Group gap="xs" wrap="nowrap">
            <Badge variant="light" color="grape">
              {preview.columns.length} column
              {preview.columns.length === 1 ? "" : "s"}
            </Badge>
            <Badge variant="light" color="teal">
              ~
              {(
                preview.estimatedRows ?? preview.parsedSampleRows
              ).toLocaleString()}{" "}
              rows
            </Badge>
            <Text size="xs" c="dimmed">
              estimated from the first{" "}
              {formatBytes(Math.min(file.size, 256 * 1024))}
            </Text>
          </Group>
          <Code block style={{ fontSize: 11 }}>
            {preview.columns.join(", ")}
          </Code>
        </Stack>
      )}

      {!file && (
        <Card
          withBorder
          padding="lg"
          style={{ borderStyle: "dashed", textAlign: "center" }}
        >
          <Stack align="center" gap={6}>
            <IconUpload size={32} opacity={0.6} />
            <Text size="sm" c="dimmed">
              Drop a file here or click <strong>Choose file</strong> above.
            </Text>
            <Text size="xs" c="dimmed">
              Streaming formats (CSV / JSONL) skip the in-memory buffer
              entirely; JSON-array files load eagerly.
            </Text>
          </Stack>
        </Card>
      )}
    </Stack>
  );
}

// ---------------------------------------------------------------------------
// Step: Target
// ---------------------------------------------------------------------------

export function TargetStep({
  mappingKind,
  onPick,
}: {
  mappingKind: MappingKind;
  onPick: (k: MappingKind) => void;
}) {
  const choices: Array<{
    value: MappingKind;
    title: string;
    description: string;
    icon: React.ReactNode;
  }> = [
    {
      value: "node",
      title: "Nodes",
      description:
        "Each row becomes a node with the chosen label + properties.",
      icon: <IconHexagon size={28} />,
    },
    {
      value: "relationship",
      title: "Relationships",
      description:
        "Each row connects two existing nodes (matched by an id property).",
      icon: <IconArrowsRightLeft size={28} />,
    },
    {
      value: "template",
      title: "Custom Cypher",
      description: "Write your own UNWIND $rows AS r … query for full control.",
      icon: <IconCode size={28} />,
    },
  ];
  return (
    <Stack gap="md" pt="md">
      <Text size="sm" c="dimmed">
        What kind of import is this?
      </Text>
      <Group grow align="stretch">
        {choices.map((c) => (
          <Card
            key={c.value}
            withBorder
            padding="md"
            style={{
              cursor: "pointer",
              borderColor:
                mappingKind === c.value
                  ? "var(--mantine-color-blue-6)"
                  : undefined,
              borderWidth: mappingKind === c.value ? 2 : 1,
            }}
            onClick={() => onPick(c.value)}
            role="button"
            tabIndex={0}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onPick(c.value);
              }
            }}
          >
            <Stack gap="xs" align="center">
              <ThemeIcon variant="light" size={56} radius="md">
                {c.icon}
              </ThemeIcon>
              <Text size="sm" fw={600}>
                {c.title}
              </Text>
              <Text size="xs" c="dimmed" ta="center">
                {c.description}
              </Text>
            </Stack>
          </Card>
        ))}
      </Group>
    </Stack>
  );
}

// ---------------------------------------------------------------------------
// Step: Mapping
// ---------------------------------------------------------------------------

interface MappingStepProps {
  mappingKind: MappingKind;
  format: RowFormat;
  preview: PreviewState | null;
  previewCypher: string | null;
  label: string;
  setLabel: (s: string) => void;
  idColumn: string | null;
  setIdColumn: (s: string | null) => void;
  propertyColumns: string[];
  setPropertyColumns: (s: string[]) => void;
  relType: string;
  setRelType: (s: string) => void;
  startLabel: string;
  setStartLabel: (s: string) => void;
  startColumn: string | null;
  setStartColumn: (s: string | null) => void;
  startMatchProperty: string;
  setStartMatchProperty: (s: string) => void;
  endLabel: string;
  setEndLabel: (s: string) => void;
  endColumn: string | null;
  setEndColumn: (s: string | null) => void;
  endMatchProperty: string;
  setEndMatchProperty: (s: string) => void;
  template: string;
  setTemplate: (s: string) => void;
  columnTypes: Record<string, string>;
  setColumnTypes: React.Dispatch<React.SetStateAction<Record<string, string>>>;
}

export function MappingStep(props: MappingStepProps) {
  const {
    mappingKind,
    format,
    preview,
    previewCypher,
    template,
    setTemplate,
    columnTypes,
    setColumnTypes,
  } = props;
  return (
    <Stack gap="md" pt="md">
      {mappingKind === "node" && <NodeMappingForm {...props} />}
      {mappingKind === "relationship" && <RelationshipMappingForm {...props} />}
      {mappingKind === "template" && (
        <Stack gap="xs">
          <Text size="xs" c="dimmed">
            Template runs once per batch with <Code>$rows</Code> bound to the
            batch&apos;s rows. Use <Code>UNWIND $rows AS r</Code> to iterate.
          </Text>
          <Textarea
            minRows={8}
            autosize
            value={template}
            onChange={(e) => setTemplate(e.currentTarget.value)}
            styles={{
              input: { fontFamily: "var(--mantine-font-family-monospace)" },
            }}
          />
        </Stack>
      )}

      {previewCypher && mappingKind !== "template" && (
        <Box>
          <Text size="xs" c="dimmed" mb={4}>
            Generated Cypher
          </Text>
          <Code block style={{ whiteSpace: "pre-wrap" }}>
            {previewCypher}
          </Code>
        </Box>
      )}

      {preview && preview.sample.length > 0 && (
        <Stack gap={4}>
          <Text size="xs" c="dimmed">
            Sample rows ({preview.sample.length} shown)
            {format === "csv" ? " — set per-column types in the headers" : ""}
          </Text>
          <ScrollArea.Autosize mah={180} type="auto">
            <Table
              withColumnBorders
              striped
              stickyHeader
              style={{ fontSize: 12 }}
            >
              <Table.Thead>
                <Table.Tr>
                  {preview.columns.map((col) => (
                    <Table.Th key={col} style={{ whiteSpace: "nowrap" }}>
                      <Stack gap={2} align="flex-start">
                        <Text size="xs" fw={600}>
                          {col}
                        </Text>
                        {format === "csv" && !col.startsWith("_") && (
                          <Select
                            size="xs"
                            variant="filled"
                            value={columnTypes[col] ?? "auto"}
                            onChange={(v) =>
                              setColumnTypes((prev) => ({
                                ...prev,
                                [col]: v ?? "auto",
                              }))
                            }
                            data={CSV_TYPE_CHOICES}
                            allowDeselect={false}
                            w={120}
                          />
                        )}
                      </Stack>
                    </Table.Th>
                  ))}
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {preview.sample.map((row, ri) => (
                  <Table.Tr key={ri}>
                    {preview.columns.map((col) => (
                      <Table.Td
                        key={col}
                        style={{
                          maxWidth: 220,
                          overflow: "hidden",
                          textOverflow: "ellipsis",
                          whiteSpace: "nowrap",
                        }}
                        title={renderSampleCell(row[col])}
                      >
                        {truncate(renderSampleCell(row[col]), 120)}
                      </Table.Td>
                    ))}
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>
          </ScrollArea.Autosize>
        </Stack>
      )}
    </Stack>
  );
}

function NodeMappingForm({
  preview,
  label,
  setLabel,
  idColumn,
  setIdColumn,
  propertyColumns,
  setPropertyColumns,
}: MappingStepProps) {
  return (
    <Stack gap="sm">
      <TextInput
        label="Node label"
        description="Single label or colon-separated (e.g. `User:Admin`)."
        value={label}
        onChange={(e) => setLabel(e.currentTarget.value)}
        error={label.trim().length === 0 ? "Pick a label" : null}
      />
      <Select
        label="Identity column (optional)"
        description="If set, its value becomes a property — later relationship imports can MATCH on it."
        data={preview?.columns.map((c) => ({ value: c, label: c })) ?? []}
        value={idColumn}
        onChange={(v) => setIdColumn(v)}
        clearable
        searchable
      />
      <TagsInput
        label="Property columns"
        description="Every selected column becomes a property on the node."
        data={preview?.columns ?? []}
        value={propertyColumns}
        onChange={setPropertyColumns}
      />
    </Stack>
  );
}

function RelationshipMappingForm({
  preview,
  relType,
  setRelType,
  startLabel,
  setStartLabel,
  startColumn,
  setStartColumn,
  startMatchProperty,
  setStartMatchProperty,
  endLabel,
  setEndLabel,
  endColumn,
  setEndColumn,
  endMatchProperty,
  setEndMatchProperty,
  propertyColumns,
  setPropertyColumns,
}: MappingStepProps) {
  return (
    <Stack gap="sm">
      <TextInput
        label="Relationship type"
        description="The :TYPE label on the new relationship."
        value={relType}
        onChange={(e) => setRelType(e.currentTarget.value)}
        error={relType.trim().length === 0 ? "Pick a relationship type" : null}
      />
      <Group grow align="start">
        <Stack gap="xs">
          <Group gap={6} align="center">
            <ThemeIcon size="sm" variant="light" color="green">
              <IconHexagon size={12} />
            </ThemeIcon>
            <Text size="xs" c="dimmed" fw={600}>
              Start node
            </Text>
          </Group>
          <TextInput
            label="Label"
            value={startLabel}
            onChange={(e) => setStartLabel(e.currentTarget.value)}
            placeholder="User"
            error={startLabel.trim().length === 0 ? "Required" : null}
          />
          <Select
            label="Source column"
            data={preview?.columns.map((c) => ({ value: c, label: c })) ?? []}
            value={startColumn}
            onChange={(v) => setStartColumn(v)}
            searchable
            clearable
            error={!startColumn ? "Required" : null}
          />
          <TextInput
            label="Match property"
            value={startMatchProperty}
            onChange={(e) => setStartMatchProperty(e.currentTarget.value)}
          />
        </Stack>
        <Stack gap="xs">
          <Group gap={6} align="center">
            <ThemeIcon size="sm" variant="light" color="orange">
              <IconHexagon size={12} />
            </ThemeIcon>
            <Text size="xs" c="dimmed" fw={600}>
              End node
            </Text>
          </Group>
          <TextInput
            label="Label"
            value={endLabel}
            onChange={(e) => setEndLabel(e.currentTarget.value)}
            placeholder="User"
            error={endLabel.trim().length === 0 ? "Required" : null}
          />
          <Select
            label="Source column"
            data={preview?.columns.map((c) => ({ value: c, label: c })) ?? []}
            value={endColumn}
            onChange={(v) => setEndColumn(v)}
            searchable
            clearable
            error={!endColumn ? "Required" : null}
          />
          <TextInput
            label="Match property"
            value={endMatchProperty}
            onChange={(e) => setEndMatchProperty(e.currentTarget.value)}
          />
        </Stack>
      </Group>
      <TagsInput
        label="Property columns (optional)"
        description="Additional columns to store on the relationship itself."
        data={preview?.columns ?? []}
        value={propertyColumns}
        onChange={setPropertyColumns}
      />
    </Stack>
  );
}

// ---------------------------------------------------------------------------
// Step: Review
// ---------------------------------------------------------------------------

export function ReviewStep({
  file,
  format,
  preview,
  previewCypher,
  batchSize,
  setBatchSize,
  permissive,
  setPermissive,
  reviewing,
  reviewStats,
  reviewError,
}: {
  file: File | null;
  format: RowFormat;
  preview: PreviewState | null;
  previewCypher: string | null;
  batchSize: number | "";
  setBatchSize: (n: number | "") => void;
  permissive: boolean;
  setPermissive: (v: boolean) => void;
  reviewing: boolean;
  reviewStats: { rows: number; batches: number; skipped: number } | null;
  reviewError: string | null;
}) {
  return (
    <Stack gap="md" pt="md">
      <Group gap="xs" wrap="wrap">
        <Badge variant="light" color="gray">
          {file?.name ?? "no file"}
        </Badge>
        <Badge variant="light" color="blue">
          {format.toUpperCase()}
        </Badge>
        {preview && (
          <Badge variant="light" color="teal">
            ~
            {(
              preview.estimatedRows ?? preview.parsedSampleRows
            ).toLocaleString()}{" "}
            rows expected
          </Badge>
        )}
      </Group>

      {previewCypher && (
        <Box>
          <Text size="xs" c="dimmed" mb={4}>
            Cypher that will run once per batch
          </Text>
          <Code
            block
            style={{ whiteSpace: "pre-wrap", maxHeight: 180, overflow: "auto" }}
          >
            {previewCypher}
          </Code>
        </Box>
      )}

      <NumberInput
        label="Batch size"
        description="Rows per Cypher transaction. Larger batches mean fewer round trips but bigger transient memory."
        value={batchSize}
        onChange={(v) => setBatchSize(typeof v === "number" ? v : "")}
        min={1}
        max={100_000}
        step={100}
        w={240}
      />

      <Checkbox
        label="Skip rows that fail to parse"
        description="Continue importing when individual rows are malformed. The skipped rows + their parse errors are reported in the summary."
        checked={permissive}
        onChange={(e) => setPermissive(e.currentTarget.checked)}
      />

      {reviewing && (
        <Group gap="xs">
          <Loader size="xs" />
          <Text size="sm" c="dimmed">
            Running a dry-run through the parser — no data is being written.
          </Text>
        </Group>
      )}

      {reviewError && (
        <Alert color="red" icon={<IconAlertCircle size={14} />}>
          {reviewError}
        </Alert>
      )}

      {reviewStats && (
        <Alert color="teal" icon={<IconCheck size={14} />}>
          Parsed {reviewStats.rows.toLocaleString()} rows across{" "}
          {reviewStats.batches} batch{reviewStats.batches === 1 ? "" : "es"}
          {reviewStats.skipped > 0
            ? ` (${reviewStats.skipped.toLocaleString()} would be skipped)`
            : " — no errors"}
          . Click <strong>Run import</strong> to commit for real.
        </Alert>
      )}
    </Stack>
  );
}

// ---------------------------------------------------------------------------
// Phase: Running
// ---------------------------------------------------------------------------

export function RunningPhase({
  file,
  progress,
  throughput,
  previewCypher,
  estimatedRows,
}: {
  file: File | null;
  progress: RowImportProgress | null;
  throughput: ThroughputReading | null;
  previewCypher: string | null;
  estimatedRows: number | null;
}) {
  const fileSize = file?.size ?? 0;
  const bytesPct =
    fileSize > 0 && progress
      ? Math.min(100, (progress.bytesFed / fileSize) * 100)
      : 0;
  const rowsPct =
    estimatedRows && progress && estimatedRows > 0
      ? Math.min(100, (progress.rowsCommitted / estimatedRows) * 100)
      : null;

  return (
    <Stack gap="md" pt="sm">
      <Group gap="xs">
        <ThemeIcon variant="light" color="blue" size="lg">
          <IconCpu size={20} />
        </ThemeIcon>
        <Stack gap={2}>
          <Text size="sm" fw={600}>
            Streaming &amp; committing rows
          </Text>
          <Text size="xs" c="dimmed">
            Chunks flow file → worker → engine. Cancel anytime.
          </Text>
        </Stack>
      </Group>

      <Stack gap={4}>
        <Group justify="space-between">
          <Text size="xs" c="dimmed">
            Bytes streamed
          </Text>
          <Text size="xs" ff="monospace">
            {progress
              ? `${formatBytes(progress.bytesFed)} / ${formatBytes(fileSize)}`
              : `0 / ${formatBytes(fileSize)}`}
          </Text>
        </Group>
        <Progress value={bytesPct} animated striped size="md" />
      </Stack>

      {rowsPct !== null && (
        <Stack gap={4}>
          <Group justify="space-between">
            <Text size="xs" c="dimmed">
              Rows committed (estimated)
            </Text>
            <Text size="xs" ff="monospace">
              {progress
                ? `${progress.rowsCommitted.toLocaleString()} / ~${estimatedRows?.toLocaleString()}`
                : "—"}
            </Text>
          </Group>
          <Progress value={rowsPct} color="teal" size="sm" />
        </Stack>
      )}

      <Group grow>
        <Stat
          label="Throughput"
          value={
            throughput ? formatRate(throughput.bytesPerSecond) : "measuring…"
          }
        />
        <Stat
          label="Row rate"
          value={
            throughput
              ? formatRowsPerSecond(throughput.rowsPerSecond)
              : "measuring…"
          }
        />
        <Stat
          label="ETA"
          value={
            throughput?.etaSeconds != null
              ? formatDuration(throughput.etaSeconds)
              : "—"
          }
        />
        <Stat
          label="Elapsed"
          value={throughput ? formatDuration(throughput.elapsedSeconds) : "—"}
        />
      </Group>

      <Group grow>
        <Stat
          label="Rows parsed"
          value={progress ? progress.rowsSeen.toLocaleString() : "0"}
        />
        <Stat
          label="Rows committed"
          value={progress ? progress.rowsCommitted.toLocaleString() : "0"}
        />
        <Stat
          label="Batches"
          value={progress ? progress.batches.toLocaleString() : "0"}
        />
      </Group>

      {previewCypher && (
        <Box>
          <Text size="xs" c="dimmed" mb={4}>
            Running this Cypher per batch
          </Text>
          <Code
            block
            style={{
              whiteSpace: "pre-wrap",
              maxHeight: 90,
              overflow: "auto",
              fontSize: 11,
            }}
          >
            {previewCypher}
          </Code>
        </Box>
      )}
    </Stack>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <Card withBorder padding="xs">
      <Stack gap={2}>
        <Text size="xs" c="dimmed">
          {label}
        </Text>
        <Text size="sm" fw={600} ff="monospace">
          {value}
        </Text>
      </Stack>
    </Card>
  );
}

// ---------------------------------------------------------------------------
// Phase: Done
// ---------------------------------------------------------------------------

export function DonePhase({
  stats,
  onClose,
  onImportAnother,
}: {
  stats: {
    rows: number;
    batches: number;
    durationMs: number;
    skipped: number;
    errors: RowParseError[];
  };
  onClose: () => void;
  onImportAnother: () => void;
}) {
  const seconds = stats.durationMs / 1000;
  const rowsPerSec = seconds > 0 ? stats.rows / seconds : 0;
  const hasSkipped = stats.skipped > 0;
  return (
    <Center>
      <Stack align="center" gap="md" mt="md" maw={520}>
        <ThemeIcon variant="light" color="teal" size={64} radius={64}>
          <IconCheck size={36} />
        </ThemeIcon>
        <Stack gap={4} align="center">
          <Text size="lg" fw={700}>
            {stats.rows.toLocaleString()} rows imported
          </Text>
          <Text size="sm" c="dimmed">
            Across {stats.batches} batch{stats.batches === 1 ? "" : "es"} in{" "}
            {formatDuration(seconds)} ({formatRowsPerSecond(rowsPerSec)})
          </Text>
        </Stack>

        {hasSkipped && (
          <Alert
            color="yellow"
            icon={<IconAlertCircle size={14} />}
            w="100%"
            title={`${stats.skipped.toLocaleString()} row${stats.skipped === 1 ? "" : "s"} skipped`}
          >
            <Stack gap="xs">
              <Text size="xs" c="dimmed">
                {stats.errors.length < stats.skipped
                  ? `Showing the first ${stats.errors.length} of ${stats.skipped} parse errors.`
                  : "All parse errors are listed below."}
              </Text>
              <Accordion variant="separated" radius="sm">
                {stats.errors.map((err, idx) => (
                  <Accordion.Item key={idx} value={`err-${idx}`}>
                    <Accordion.Control>
                      <Text size="sm">
                        Row {err.row.toLocaleString()}
                        {err.column ? ` · column ${err.column}` : ""}:{" "}
                        {err.message}
                      </Text>
                    </Accordion.Control>
                    <Accordion.Panel>
                      <Code
                        block
                        style={{
                          whiteSpace: "pre-wrap",
                          wordBreak: "break-all",
                        }}
                      >
                        {err.rawSample || "(empty)"}
                      </Code>
                    </Accordion.Panel>
                  </Accordion.Item>
                ))}
              </Accordion>
            </Stack>
          </Alert>
        )}

        <Group>
          <Button variant="default" onClick={onImportAnother}>
            Import another
          </Button>
          <Button onClick={onClose}>Close</Button>
        </Group>
      </Stack>
    </Center>
  );
}

// ---------------------------------------------------------------------------
// Phase: Error
// ---------------------------------------------------------------------------

/**
 * Parse a row-attribution prefix out of an engine error message.
 * Errors produced by [`row_parse_io_error`] on the Rust side render
 * as `row N[, column \`C\`]: message (raw: \`...\`)`. Recovering the
 * pieces lets us surface them with structured affordances even though
 * the engine ships errors as plain strings.
 */
function parseStructuredRowError(message: string): {
  row: number;
  column: string | null;
  rawSample: string | null;
  detail: string;
} | null {
  const match =
    /^row (\d+)(?:, column `([^`]+)`)?: (.*?)(?: \(raw: `([\s\S]*)`\))?$/.exec(
      message,
    );
  if (!match) return null;
  return {
    row: Number(match[1]),
    column: match[2] ?? null,
    detail: match[3] ?? message,
    rawSample: match[4] ?? null,
  };
}

export function ErrorPhase({
  message,
  partialStats,
  onBack,
}: {
  message: string;
  partialStats: RowImportProgress | null;
  onBack: () => void;
}) {
  const structured = parseStructuredRowError(message);
  return (
    <Center>
      <Stack align="center" gap="md" mt="md" maw={560}>
        <ThemeIcon variant="light" color="red" size={64} radius={64}>
          <IconCircleX size={36} />
        </ThemeIcon>
        <Stack gap={4} align="center">
          <Text size="lg" fw={700}>
            Import stopped
          </Text>
          {structured ? (
            <Stack gap={4} align="center">
              <Text size="sm" c="dimmed" ta="center">
                Row {structured.row.toLocaleString()}
                {structured.column
                  ? ` · column ${structured.column}`
                  : ""}: {structured.detail}
              </Text>
              <Text size="xs" c="dimmed" mt={4}>
                Re-run with <em>Skip rows that fail to parse</em> enabled to
                continue past bad rows.
              </Text>
            </Stack>
          ) : (
            <Text size="sm" c="dimmed" ta="center">
              {message}
            </Text>
          )}
          {structured?.rawSample && (
            <Box w="100%" mt="xs">
              <Text size="xs" c="dimmed" mb={4}>
                Offending row (truncated)
              </Text>
              <Code
                block
                style={{ whiteSpace: "pre-wrap", wordBreak: "break-all" }}
              >
                {structured.rawSample}
              </Code>
            </Box>
          )}
          {partialStats && partialStats.rowsCommitted > 0 && (
            <Text size="xs" c="dimmed" mt={4}>
              {partialStats.rowsCommitted.toLocaleString()} rows had already
              been committed and remain in the graph.
            </Text>
          )}
        </Stack>
        <Button variant="default" onClick={onBack}>
          Back to mapping
        </Button>
      </Stack>
    </Center>
  );
}

// ---------------------------------------------------------------------------
// Footer (per-phase action buttons)
// ---------------------------------------------------------------------------

export function Footer({
  phase,
  step,
  canAdvanceFromFile,
  canAdvanceFromTarget,
  canAdvanceFromMapping,
  canRunFromReview,
  onBack,
  onNext,
  onRun,
  onCancel,
  onClose,
}: {
  phase: "config" | "running" | "done" | "error";
  step: number;
  canAdvanceFromFile: boolean;
  canAdvanceFromTarget: boolean;
  canAdvanceFromMapping: boolean;
  canRunFromReview: boolean;
  onBack: () => void;
  onNext: () => void;
  onRun: () => void;
  onCancel: () => void;
  onClose: () => void;
}) {
  if (phase === "running") {
    return (
      <Group justify="space-between">
        <Text size="xs" c="dimmed">
          The import keeps going if you close this dialog. Use Cancel to stop.
        </Text>
        <Button
          color="red"
          variant="light"
          leftSection={<IconX size={14} />}
          onClick={onCancel}
        >
          Cancel import
        </Button>
      </Group>
    );
  }
  if (phase === "done" || phase === "error") {
    // The Done/Error phases own their own action buttons; render
    // nothing in the footer so the user isn't presented with two
    // competing primary actions.
    return null;
  }
  // Config phase
  const isLast = step === 3;
  const canNext =
    (
      [
        canAdvanceFromFile,
        canAdvanceFromTarget,
        canAdvanceFromMapping,
        canRunFromReview,
      ] as const
    )[step] ?? false;
  return (
    <Group justify="space-between">
      <Button variant="subtle" onClick={onClose} color="gray">
        Close
      </Button>
      <Group>
        <Button variant="default" onClick={onBack} disabled={step === 0}>
          Back
        </Button>
        {isLast ? (
          <Button onClick={onRun} disabled={!canNext}>
            Run import
          </Button>
        ) : (
          <Button onClick={onNext} disabled={!canNext}>
            Next
          </Button>
        )}
      </Group>
    </Group>
  );
}
