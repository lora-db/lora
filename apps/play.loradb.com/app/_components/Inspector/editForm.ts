/**
 * Pure helpers behind the inspector's edit mode. Splitting these out
 * lets the UI stay declarative and lets the rules be unit-tested
 * without spinning React.
 *
 * The model is row-based: one `EditRow` per property the user is
 * editing. Rows carry the *raw* string the user typed so a half-typed
 * `12.3e` survives a re-render — we only parse on save (and again on
 * `validateRows` for live error feedback).
 */

import type { PropertyKind } from "./propertyValue";
import { detectPropertyKind } from "./propertyValue";

export type EditableKind =
  | "string"
  | "integer"
  | "float"
  | "boolean"
  | "json"
  | "null";

export interface EditRow {
  /** Stable id so React can key rows across reorders/deletes. */
  uid: string;
  key: string;
  kind: EditableKind;
  /** Raw user-facing text for string/number/json kinds. */
  text: string;
  /** Boolean toggle state, when `kind === "boolean"`. */
  bool: boolean;
  /** True when the original property carried a value that we can't
   *  edit safely (e.g. point / duration / bigint) — surfaced as a
   *  raw JSON textarea so the user can still tweak it. */
  rawHint?: PropertyKind;
}

export interface RowError {
  uid: string;
  message: string;
}

/**
 * Map the kind detected on the value to an editable kind. Specialised
 * kinds (url, email, datetime) all fall through to a plain string
 * input — we don't want to fight the user over an "is this an email?"
 * regex while they're editing. Engine-specific shapes (point /
 * duration / bigint) collapse to `json` so they can still be tweaked.
 */
export function editableKindFor(kind: PropertyKind): EditableKind {
  switch (kind) {
    case "null":
      return "null";
    case "boolean":
      return "boolean";
    case "integer":
      return "integer";
    case "float":
      return "float";
    case "string":
    case "url":
    case "email":
    case "datetime":
      return "string";
    case "array":
    case "object":
    case "point":
    case "duration":
    case "bigint":
      return "json";
  }
}

let uidCounter = 0;
function nextUid(): string {
  uidCounter += 1;
  return `r${uidCounter}`;
}

/**
 * Convert a `{ key: value }` property map into an ordered list of
 * editable rows. Object key order is preserved (V8 honours insertion
 * order for string keys), which keeps the visual layout stable across
 * a re-open of the popup.
 */
export function rowsFromProperties(
  properties: Record<string, unknown>,
): EditRow[] {
  const out: EditRow[] = [];
  for (const key of Object.keys(properties)) {
    out.push(rowFromValue(key, properties[key]));
  }
  return out;
}

export function rowFromValue(key: string, value: unknown): EditRow {
  const kind = detectPropertyKind(value);
  const editable = editableKindFor(kind);
  const uid = nextUid();
  if (editable === "boolean") {
    return { uid, key, kind: "boolean", text: "", bool: value === true };
  }
  if (editable === "null") {
    return { uid, key, kind: "null", text: "", bool: false };
  }
  if (editable === "json") {
    return {
      uid,
      key,
      kind: "json",
      text: JSON.stringify(value, null, 2),
      bool: false,
      rawHint: kind,
    };
  }
  // string / integer / float — render with the canonical text form
  // (no localisation; users don't expect "1,000" in a property cell).
  return {
    uid,
    key,
    kind: editable,
    text: stringifyScalar(value),
    bool: false,
  };
}

function stringifyScalar(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number") return String(value);
  if (typeof value === "boolean") return value ? "true" : "false";
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

export function emptyRow(kind: EditableKind = "string"): EditRow {
  return { uid: nextUid(), key: "", kind, text: "", bool: false };
}

/**
 * Parse a single row to its concrete value. Returns either `{ ok: true,
 * value }` or `{ ok: false, message }`. Whitespace-only `null` rows
 * resolve to `null` so the user can explicitly clear a field.
 */
export function parseRow(
  row: EditRow,
): { ok: true; value: unknown } | { ok: false; message: string } {
  switch (row.kind) {
    case "null":
      return { ok: true, value: null };
    case "boolean":
      return { ok: true, value: row.bool };
    case "string":
      return { ok: true, value: row.text };
    case "integer": {
      const trimmed = row.text.trim();
      if (trimmed === "") return { ok: true, value: null };
      if (!/^-?\d+$/.test(trimmed)) {
        return { ok: false, message: "Not a valid integer." };
      }
      const n = Number(trimmed);
      if (!Number.isFinite(n)) {
        return { ok: false, message: "Integer out of range." };
      }
      return { ok: true, value: n };
    }
    case "float": {
      const trimmed = row.text.trim();
      if (trimmed === "") return { ok: true, value: null };
      const n = Number(trimmed);
      if (!Number.isFinite(n)) {
        return { ok: false, message: "Not a valid number." };
      }
      return { ok: true, value: n };
    }
    case "json": {
      const trimmed = row.text.trim();
      if (trimmed === "") return { ok: true, value: null };
      try {
        const parsed = JSON.parse(trimmed) as unknown;
        return { ok: true, value: parsed };
      } catch (err) {
        return {
          ok: false,
          message: err instanceof Error ? err.message : "Invalid JSON.",
        };
      }
    }
  }
}

/**
 * Validate a row's key. Cypher accepts a wide range of identifiers but
 * the wider Lora driver and the result-pane renderers assume plain
 * names (no leading dot, no surrounding whitespace, no embedded `.`).
 * We keep the check permissive so the user isn't fenced in.
 */
export function validateKey(key: string): string | null {
  if (key.trim().length === 0) return "Key is required.";
  if (key !== key.trim()) return "Key cannot have leading/trailing spaces.";
  if (key.includes(".")) return "Key cannot contain '.'.";
  return null;
}

/**
 * Cross-row validation. Returns an array of `RowError`s (one per
 * offending row) plus a `globalError` for issues that span rows (e.g.
 * a missing required key).
 */
export function validateRows(
  rows: EditRow[],
  options: {
    /** Keys covered by NODE_KEY / UNIQUE / NOT_NULL constraints. They
     *  must be present and non-null on save. */
    requiredKeys?: ReadonlySet<string>;
  } = {},
): { rowErrors: RowError[]; globalError: string | null } {
  const rowErrors: RowError[] = [];
  const seen = new Map<string, string>(); // key -> first uid

  for (const row of rows) {
    const keyErr = validateKey(row.key);
    if (keyErr) {
      rowErrors.push({ uid: row.uid, message: keyErr });
      continue;
    }
    const prior = seen.get(row.key);
    if (prior !== undefined) {
      rowErrors.push({
        uid: row.uid,
        message: `Duplicate key "${row.key}".`,
      });
      continue;
    }
    seen.set(row.key, row.uid);

    const parsed = parseRow(row);
    if (!parsed.ok) {
      rowErrors.push({ uid: row.uid, message: parsed.message });
      continue;
    }
    if (
      options.requiredKeys?.has(row.key) === true &&
      (parsed.value === null || parsed.value === undefined)
    ) {
      rowErrors.push({
        uid: row.uid,
        message: "Required by a constraint — cannot be empty.",
      });
    }
  }

  // Missing required keys → global error (no row to attach to).
  let globalError: string | null = null;
  if (options.requiredKeys && options.requiredKeys.size > 0) {
    const present = new Set(rows.map((r) => r.key));
    const missing = [...options.requiredKeys].filter((k) => !present.has(k));
    if (missing.length > 0) {
      globalError = `Missing required ${
        missing.length === 1 ? "property" : "properties"
      }: ${missing.join(", ")}`;
    }
  }

  return { rowErrors, globalError };
}

/**
 * Build the final `{ key: value }` map from a set of rows. Returns
 * `null` if any row fails validation — callers should call
 * `validateRows` first to surface row-level errors to the user.
 */
export function buildPropertiesPayload(
  rows: EditRow[],
): Record<string, unknown> | null {
  const out: Record<string, unknown> = {};
  for (const row of rows) {
    if (validateKey(row.key) !== null) return null;
    if (row.key in out) return null;
    const parsed = parseRow(row);
    if (!parsed.ok) return null;
    out[row.key] = parsed.value;
  }
  return out;
}

/**
 * Return true when the row-set is materially different from the
 * original property map — used to gate the "discard unsaved changes?"
 * confirm and to disable Save while the form is clean.
 */
export function rowsDirty(
  rows: EditRow[],
  original: Record<string, unknown>,
): boolean {
  const payload = buildPropertiesPayload(rows);
  if (payload === null) {
    // Invalid form — treat as dirty so the user can't accidentally
    // dismiss a broken edit.
    return true;
  }
  const originalKeys = Object.keys(original);
  const payloadKeys = Object.keys(payload);
  if (originalKeys.length !== payloadKeys.length) return true;
  for (const k of originalKeys) {
    if (!(k in payload)) return true;
    if (!valuesShallowEqual(original[k], payload[k])) return true;
  }
  return false;
}

function valuesShallowEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (a === null || b === null) return false;
  if (typeof a !== typeof b) return false;
  if (typeof a === "object") {
    try {
      return JSON.stringify(a) === JSON.stringify(b);
    } catch {
      return false;
    }
  }
  return false;
}
