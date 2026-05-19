import { foldAll, unfoldAll } from "@codemirror/language";
import type { EditorView } from "@codemirror/view";

/**
 * Imperative commands exposed on the `LoraJsonEditorHandle`. Each
 * works against the current buffer and mutates it via a single
 * full-doc replace transaction, so undo treats them as a single
 * step.
 */

/**
 * Recursively sort object keys alphabetically. Arrays preserve
 * their order. On parse failure the buffer is left untouched.
 * Returns the new source (also dispatched to the view).
 */
export function sortKeysCmd(view: EditorView, indent = 2): string {
  const current = view.state.doc.toString();
  let parsed: unknown;
  try {
    parsed = JSON.parse(current);
  } catch {
    return current;
  }
  const sorted = sortValue(parsed);
  const next = JSON.stringify(sorted, null, indent);
  if (next === current) return current;
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: next },
    userEvent: "input.sortKeys",
  });
  return next;
}

/**
 * Convert single-quoted strings to double-quoted (a strict-JSON
 * repair for hand-written payloads). Only flips quotes outside an
 * existing string literal — it does not modify legitimate `'`
 * characters inside `"…"` values.
 */
export function toggleQuotesCmd(view: EditorView): string {
  const src = view.state.doc.toString();
  let inDouble = false;
  let out = "";
  for (let i = 0; i < src.length; i++) {
    const ch = src[i];
    if (inDouble) {
      out += ch;
      if (ch === "\\") {
        // Copy the escape pair atomically.
        const next = src[i + 1];
        if (next !== undefined) {
          out += next;
          i++;
        }
        continue;
      }
      if (ch === '"') inDouble = false;
      continue;
    }
    if (ch === '"') {
      inDouble = true;
      out += ch;
      continue;
    }
    if (ch === "'") {
      // Read until the matching `'`, copy as a double-quoted JSON
      // string with any embedded `"` properly escaped.
      let body = "";
      let j = i + 1;
      while (j < src.length) {
        const c = src[j];
        if (c === "\\" && src[j + 1] !== undefined) {
          body += c + src[j + 1];
          j += 2;
          continue;
        }
        if (c === "'") break;
        body += c;
        j++;
      }
      out += JSON.stringify(body);
      i = j;
      continue;
    }
    out += ch;
  }
  if (out === src) return src;
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: out },
    userEvent: "input.toggleQuotes",
  });
  return out;
}

/** Collapse every foldable range in the buffer. */
export function foldAllCmd(view: EditorView): void {
  foldAll(view);
}

/** Expand every folded range in the buffer. */
export function unfoldAllCmd(view: EditorView): void {
  unfoldAll(view);
}

function sortValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortValue);
  if (value !== null && typeof value === "object") {
    const obj = value as Record<string, unknown>;
    const out: Record<string, unknown> = {};
    for (const key of Object.keys(obj).sort()) {
      out[key] = sortValue(obj[key]);
    }
    return out;
  }
  return value;
}
