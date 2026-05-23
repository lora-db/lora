import type { RowFormat, RowMapping } from "@loradb/lora-wasm";

export interface PreviewState {
  /** Column names sniffed from the file's header line (CSV) or from
   * the keys of the first object (JSONL/JSON). */
  columns: string[];
  /** First ~10 rows for the mapping editor's sample-data display. */
  sample: Array<Record<string, unknown>>;
  /** Row count projection extrapolated from the sniffed prefix and
   * the file's total byte size. `null` when extrapolation isn't
   * possible (json-array, file shorter than the sniff window). */
  estimatedRows: number | null;
  /** Number of rows we actually parsed during the sniff. Surfaced
   * for the row-count badge when extrapolation is unavailable. */
  parsedSampleRows: number;
}

export type MappingKind = "node" | "relationship" | "template";

export function detectRowFormat(name: string): RowFormat | null {
  const ext = name.toLowerCase().split(".").pop();
  if (!ext) return null;
  if (ext === "jsonl" || ext === "ndjson") return "jsonl";
  if (ext === "json") return "json";
  if (ext === "csv") return "csv";
  return null;
}

export async function buildPreview(
  file: File,
): Promise<{ preview: PreviewState; format: RowFormat | null }> {
  const detected = detectRowFormat(file.name);
  const chunk = file.slice(0, 256 * 1024);
  const text = await chunk.text();
  const fileSize = file.size;
  switch (detected) {
    case "csv":
      return {
        preview: withEstimate(sniffCsv(text), text, fileSize, "csv"),
        format: "csv",
      };
    case "jsonl":
      return {
        preview: withEstimate(sniffJsonl(text), text, fileSize, "jsonl"),
        format: "jsonl",
      };
    case "json":
      return {
        preview: withEstimate(sniffJsonArray(text), text, fileSize, "json"),
        format: "json",
      };
    case null: {
      try {
        return {
          preview: withEstimate(sniffJsonl(text), text, fileSize, "jsonl"),
          format: "jsonl",
        };
      } catch {
        try {
          return {
            preview: withEstimate(sniffJsonArray(text), text, fileSize, "json"),
            format: "json",
          };
        } catch {
          return {
            preview: withEstimate(sniffCsv(text), text, fileSize, "csv"),
            format: "csv",
          };
        }
      }
    }
  }
}

/**
 * Pre-fill the mapping form from the sniffed columns + filename.
 * Heuristics:
 *   - Filename `users.csv` -> label `User`; `relationships.csv` ->
 *     switch to relationship kind.
 *   - Sample row with `_label` (from `:LABEL` schema marker) -> use
 *     that as the label.
 *   - Sample row with `_type` -> switch to relationship kind, use
 *     it as the rel type.
 *   - Column named `id` or `*_id` -> identity / start-id / end-id.
 *   - Column matching `start|source|from` -> start_column;
 *     `end|target|dest|to` -> end_column.
 */
export function applySmartDefaults(
  fileName: string,
  preview: PreviewState,
  setters: {
    setLabel: (s: string) => void;
    setMappingKind: (k: MappingKind) => void;
    setRelType: (s: string) => void;
    setIdColumn: (s: string | null) => void;
    setStartColumn: (s: string | null) => void;
    setEndColumn: (s: string | null) => void;
    setStartLabel: (s: string) => void;
    setEndLabel: (s: string) => void;
    setPropertyColumns: (s: string[]) => void;
    setColumnTypes: (s: Record<string, string>) => void;
  },
) {
  if (preview.columns.length === 0) return;

  const base = fileName.replace(/\.[^.]+$/, "");
  const candidateLabel = singularize(toLabelCase(base));
  setters.setLabel(candidateLabel || "Imported");

  if (preview.sample[0]?._label) {
    setters.setLabel(String(preview.sample[0]._label));
  }
  if (preview.sample[0]?._type) {
    setters.setMappingKind("relationship");
    setters.setRelType(String(preview.sample[0]._type));
  } else if (/(rel(ationship)?s?|edges?|links?|connections?)$/i.test(base)) {
    setters.setMappingKind("relationship");
  }

  const ids = preview.columns.filter(
    (c) => c === "_id" || c === "id" || c.toLowerCase().endsWith("_id"),
  );
  const startGuess = preview.columns.find(
    (c) => c === "_start_id" || /start|source|from/i.test(c.replace(/^_/, "")),
  );
  const endGuess = preview.columns.find(
    (c) =>
      c === "_end_id" || /end|target|dest|to(?:$|_)/i.test(c.replace(/^_/, "")),
  );
  setters.setIdColumn(ids[0] ?? null);
  setters.setStartColumn(startGuess ?? null);
  setters.setEndColumn(endGuess ?? null);
  setters.setPropertyColumns(
    preview.columns.filter(
      (c) =>
        !c.startsWith("_") &&
        c !== ids[0] &&
        c !== startGuess &&
        c !== endGuess,
    ),
  );
  const types: Record<string, string> = {};
  for (const c of preview.columns) {
    if (!c.startsWith("_")) types[c] = "auto";
  }
  setters.setColumnTypes(types);
}

export function effectiveBatchSize(input: number | ""): number {
  return typeof input === "number" && input > 0 ? input : 1_000;
}

export function wrapStream(
  file: File,
  format: RowFormat,
  columnTypes: Record<string, string>,
): ReadableStream<Uint8Array> {
  if (format !== "csv" || !hasCsvOverrides(columnTypes)) {
    return file.stream();
  }
  return rewriteCsvHeaderStream(file.stream(), (h) =>
    applyCsvTypeOverrides(h, columnTypes),
  );
}

export function renderMappingTemplate(mapping: RowMapping): string {
  return mapping.kind === "node"
    ? renderNodeTemplate(mapping)
    : renderRelationshipTemplate(mapping);
}

/**
 * Cypher accepts unquoted identifiers only when they are alphanumeric +
 * underscore (and start with a letter or underscore). Anything else —
 * spaces in CSV headers like `User Id`, dashes, leading digits — has to
 * be wrapped in backticks to parse. Mirrors `quote_ident` in
 * `lora_io::mapping` so the preview matches the Cypher the engine
 * actually runs.
 */
const SIMPLE_IDENT_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;
function quoteIdent(name: string): string {
  return SIMPLE_IDENT_RE.test(name) ? name : `\`${name}\``;
}
function quoteParam(name: string): string {
  return `r.${quoteIdent(name)}`;
}

function renderNodeTemplate(mapping: RowMapping & { kind: "node" }): string {
  const labelPart = mapping.label
    .split(":")
    .filter((s) => s.length > 0)
    .map(quoteIdent)
    .join(":");
  const props: string[] = [];
  if (mapping.id_column) {
    props.push(
      `${quoteIdent(mapping.id_property ?? "id")}: ${quoteParam(mapping.id_column)}`,
    );
  }
  for (const spec of mapping.properties) {
    if (spec.source === mapping.id_column) continue;
    props.push(`${quoteIdent(spec.property)}: ${quoteParam(spec.source)}`);
  }
  const body = props.length === 0 ? "" : ` {${props.join(", ")}}`;
  return `UNWIND $rows AS r CREATE (:${labelPart}${body})`;
}

function renderRelationshipTemplate(
  mapping: RowMapping & { kind: "relationship" },
): string {
  const propPairs = mapping.properties
    .filter(
      (s) =>
        s.source !== mapping.start_column && s.source !== mapping.end_column,
    )
    .map((s) => `${quoteIdent(s.property)}: ${quoteParam(s.source)}`);
  const body = propPairs.length === 0 ? "" : ` {${propPairs.join(", ")}}`;
  const startLabel = quoteIdent(mapping.start_label);
  const endLabel = quoteIdent(mapping.end_label);
  const startMatch = quoteIdent(mapping.start_match_property);
  const endMatch = quoteIdent(mapping.end_match_property);
  return (
    `UNWIND $rows AS r\n` +
    `MATCH (a:${startLabel} {${startMatch}: ${quoteParam(mapping.start_column)}}),\n` +
    `      (b:${endLabel} {${endMatch}: ${quoteParam(mapping.end_column)}})\n` +
    `CREATE (a)-[:${quoteIdent(mapping.rel_type)}${body}]->(b)`
  );
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(2)} MB`;
}

export function renderSampleCell(value: unknown): string {
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
}

export function truncate(s: string, n: number): string {
  if (s.length <= n) return s;
  return `${s.slice(0, n - 1)}…`;
}

function hasCsvOverrides(overrides: Record<string, string>): boolean {
  for (const v of Object.values(overrides)) {
    if (v && v !== "auto") return true;
  }
  return false;
}

function applyCsvTypeOverrides(
  headerLine: string,
  overrides: Record<string, string>,
): string {
  // splitCsvLine already consumes the surrounding quotes, so we
  // re-encode every cell on the way out — that picks the right
  // RFC-4180 quoting for the *new* content (which may differ from
  // the original, e.g. a header name with a comma whose type just
  // got appended).
  const cells = splitCsvLine(stripBom(headerLine));
  const rewritten = cells.map((cell) => {
    const trimmed = cell.trim();
    if (trimmed.startsWith(":")) return encodeCsvCell(cell);
    const colonIdx = trimmed.indexOf(":");
    const name = (
      colonIdx === -1 ? trimmed : trimmed.slice(0, colonIdx)
    ).trim();
    const override = overrides[name];
    if (!override || override === "auto") return encodeCsvCell(cell);
    return encodeCsvCell(`${name}:${override}`);
  });
  return rewritten.join(",");
}

function encodeCsvCell(value: string): string {
  if (/[",\r\n]/.test(value)) {
    return `"${value.replace(/"/g, '""')}"`;
  }
  return value;
}

function rewriteCsvHeaderStream(
  source: ReadableStream<Uint8Array>,
  rewrite: (header: string) => string,
): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  let buffer: Uint8Array = new Uint8Array(0);
  let handled = false;
  return source.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      transform(chunk, controller) {
        if (handled) {
          controller.enqueue(chunk);
          return;
        }
        const merged = new Uint8Array(buffer.length + chunk.length);
        merged.set(buffer);
        merged.set(chunk, buffer.length);
        buffer = merged;
        const nl = buffer.indexOf(0x0a);
        if (nl < 0) return;
        const hadCrlf = nl > 0 && buffer[nl - 1] === 0x0d;
        const endIdx = hadCrlf ? nl - 1 : nl;
        const lineEnding = hadCrlf ? "\r\n" : "\n";
        const headerStr = decoder.decode(buffer.subarray(0, endIdx));
        const rewritten = rewrite(headerStr);
        controller.enqueue(encoder.encode(`${rewritten}${lineEnding}`));
        const tail = buffer.subarray(nl + 1);
        if (tail.length > 0) controller.enqueue(tail);
        buffer = new Uint8Array(0);
        handled = true;
      },
      flush(controller) {
        if (!handled && buffer.length > 0) {
          const headerStr = decoder.decode(buffer);
          controller.enqueue(encoder.encode(rewrite(headerStr)));
        }
      },
    }),
  );
}

function withEstimate(
  preview: { columns: string[]; sample: Array<Record<string, unknown>> },
  text: string,
  fileSize: number,
  format: RowFormat,
): PreviewState {
  const parsedSampleRows = preview.sample.length;
  if (format === "json") {
    return { ...preview, estimatedRows: null, parsedSampleRows };
  }
  let newlines = 0;
  for (let i = 0; i < text.length; i += 1) {
    if (text.charCodeAt(i) === 0x0a) newlines += 1;
  }
  const rowsInSample = format === "csv" ? Math.max(0, newlines - 1) : newlines;
  if (rowsInSample === 0) {
    return { ...preview, estimatedRows: null, parsedSampleRows };
  }
  const sampleBytes = byteLength(text);
  if (sampleBytes >= fileSize) {
    return { ...preview, estimatedRows: rowsInSample, parsedSampleRows };
  }
  const avgBytesPerRow = sampleBytes / rowsInSample;
  return {
    ...preview,
    estimatedRows: Math.round(fileSize / avgBytesPerRow),
    parsedSampleRows,
  };
}

function byteLength(s: string): number {
  return new TextEncoder().encode(s).length;
}

function sniffJsonl(text: string): {
  columns: string[];
  sample: Array<Record<string, unknown>>;
} {
  const lines = text.split(/\r?\n/).filter((l) => l.trim().length > 0);
  const sample: Array<Record<string, unknown>> = [];
  const columns = new Set<string>();
  for (let i = 0; i < Math.min(lines.length, 10); i += 1) {
    const parsed = JSON.parse(lines[i]!) as unknown;
    if (!isObjectRow(parsed)) {
      throw new Error(`expected JSON object on line ${i + 1}`);
    }
    const obj = parsed;
    sample.push(obj);
    for (const k of Object.keys(obj)) columns.add(k);
  }
  return { columns: [...columns], sample };
}

function sniffJsonArray(text: string): {
  columns: string[];
  sample: Array<Record<string, unknown>>;
} {
  let body = text.trim();
  if (!body.endsWith("]")) {
    const lastComma = body.lastIndexOf(",");
    body = lastComma > 0 ? `${body.slice(0, lastComma)}]` : `${body}]`;
  }
  const arr = JSON.parse(body) as unknown;
  if (!Array.isArray(arr)) throw new Error("expected a JSON array");
  const sample: Array<Record<string, unknown>> = [];
  const columns = new Set<string>();
  for (let i = 0; i < Math.min(arr.length, 10); i += 1) {
    const v = arr[i];
    if (!isObjectRow(v)) {
      throw new Error(`expected JSON object at array index ${i}`);
    }
    const obj = v;
    sample.push(obj);
    for (const k of Object.keys(obj)) columns.add(k);
  }
  return { columns: [...columns], sample };
}

function isObjectRow(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function sniffCsv(text: string): {
  columns: string[];
  sample: Array<Record<string, unknown>>;
} {
  // BOM is what Excel / Google Sheets write on "Save as CSV (UTF-8)"
  // — without the strip, the first column name carries `﻿` and
  // every smart-default + downstream `r.name` reference misses.
  const cleaned = stripBom(text);
  const records = parseCsvRecords(cleaned, 11);
  if (records.length === 0) return { columns: [], sample: [] };
  const columns = records[0]!.map(normalizeCsvHeader);
  const sample: Array<Record<string, unknown>> = [];
  for (let i = 1; i < records.length; i += 1) {
    const cells = records[i]!;
    if (cells.length === 1 && cells[0]!.length === 0) continue;
    const obj: Record<string, unknown> = {};
    columns.forEach((name, idx) => {
      obj[name] = cells[idx] ?? "";
    });
    sample.push(obj);
  }
  return { columns, sample };
}

const BOM = "﻿";
function stripBom(s: string): string {
  return s.startsWith(BOM) ? s.slice(1) : s;
}

/**
 * Parse up to `maxRecords` CSV records from `text`. Walks the input as a
 * single state machine so quoted cells with embedded newlines (RFC 4180)
 * stay attached to their record instead of being shattered by a naive
 * `split(/\r?\n/)`. The previous line-then-split sniffer broke the
 * preview's column count for any file whose first 10 rows contained a
 * multiline quoted cell — the Rust decoder handles those correctly, so
 * the import then "worked" but the wizard's preview was nonsense.
 */
function parseCsvRecords(text: string, maxRecords: number): string[][] {
  const records: string[][] = [];
  let current: string[] = [];
  let cell = "";
  let inQuotes = false;
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i]!;
    if (inQuotes) {
      if (ch === '"') {
        if (text[i + 1] === '"') {
          cell += '"';
          i += 1;
        } else {
          inQuotes = false;
        }
      } else {
        cell += ch;
      }
    } else if (ch === '"') {
      inQuotes = true;
    } else if (ch === ",") {
      current.push(cell);
      cell = "";
    } else if (ch === "\n" || ch === "\r") {
      current.push(cell);
      cell = "";
      records.push(current);
      current = [];
      if (ch === "\r" && text[i + 1] === "\n") i += 1;
      if (records.length >= maxRecords) return records;
    } else {
      cell += ch;
    }
  }
  if (cell.length > 0 || current.length > 0) {
    current.push(cell);
    records.push(current);
  }
  return records;
}

function splitCsvLine(line: string): string[] {
  const out: string[] = [];
  let cur = "";
  let inQuotes = false;
  for (let i = 0; i < line.length; i += 1) {
    const ch = line[i]!;
    if (inQuotes) {
      if (ch === '"') {
        if (line[i + 1] === '"') {
          cur += '"';
          i += 1;
        } else {
          inQuotes = false;
        }
      } else {
        cur += ch;
      }
    } else if (ch === '"') {
      inQuotes = true;
    } else if (ch === ",") {
      out.push(cur);
      cur = "";
    } else {
      cur += ch;
    }
  }
  out.push(cur);
  return out;
}

function normalizeCsvHeader(raw: string): string {
  const trimmed = raw.trim();
  if (trimmed.startsWith(":")) {
    const upper = trimmed.slice(1).toUpperCase();
    if (upper === "ID") return "_id";
    if (upper === "LABEL") return "_label";
    if (upper === "START_ID") return "_start_id";
    if (upper === "END_ID") return "_end_id";
    if (upper === "TYPE") return "_type";
    return `_${upper.toLowerCase()}`;
  }
  return trimmed.split(":")[0]?.trim() ?? trimmed;
}

function toLabelCase(s: string): string {
  return s
    .split(/[^A-Za-z0-9]+/)
    .filter((p) => p.length > 0)
    .map((p) => p[0]!.toUpperCase() + p.slice(1))
    .join("");
}

function singularize(label: string): string {
  if (label.length < 4) return label;
  if (/ies$/.test(label)) return label.slice(0, -3) + "y";
  if (/(s|x|z|ch|sh)es$/.test(label)) return label.slice(0, -2);
  if (/s$/.test(label) && !/ss$/.test(label)) return label.slice(0, -1);
  return label;
}
