import {
  Facet,
  type EditorState,
} from "@codemirror/state";
import { linter, type Diagnostic } from "@codemirror/lint";

/**
 * Host-supplied constraints on the **top-level** object keys.
 *
 *  - `allowedKeys`: when set, any top-level key not in this list
 *    is flagged as a lint error. Use it to lock down a payload to
 *    the parameters a sibling Cypher query actually accepts.
 *
 *  - `requiredKeys`: when set, any key in this list that is *not*
 *    present is flagged as a lint warning (anchored at the closing
 *    `}` so the marker stays visible).
 *
 * Both constraints only apply when the buffer's top-level value is
 * an object literal. Arrays / scalars / invalid JSON are passed
 * through (the JSON parse linter still flags those).
 */
export interface KeyConstraints {
  allowedKeys?: readonly string[];
  requiredKeys?: readonly string[];
}

const EMPTY: KeyConstraints = {};

/**
 * Facet carrying the active constraints through the editor. The
 * facet is intentionally per-state so reconfigures don't replace
 * the whole linter — only its config.
 */
export const keyConstraintsFacet = Facet.define<KeyConstraints, KeyConstraints>(
  {
    combine(values) {
      return values.length === 0 ? EMPTY : (values[0] ?? EMPTY);
    },
  },
);

export function getKeyConstraints(state: EditorState): KeyConstraints {
  return state.facet(keyConstraintsFacet);
}

/**
 * Walk the top level of a JSON object literal and yield each key
 * with its `from`/`to` offsets (covering the surrounding quotes).
 * Returns `null` when the buffer does not start with `{`. The walk
 * is tolerant of partial input — a missing `}` is fine.
 */
function* topLevelKeys(
  src: string,
): Generator<{ key: string; from: number; to: number }> {
  let i = 0;
  while (i < src.length && /\s/.test(src[i] ?? "")) i++;
  if (src[i] !== "{") return;
  i++;
  let depth = 0;
  while (i < src.length) {
    const ch = src[i];
    if (ch === undefined) break;
    if (/\s/.test(ch) || ch === ",") {
      i++;
      continue;
    }
    if (ch === "{" || ch === "[") {
      depth = 1;
      i++;
      while (i < src.length && depth > 0) {
        const c = src[i];
        if (c === '"') {
          // Skip string atomically.
          i++;
          while (i < src.length) {
            const cc = src[i];
            if (cc === "\\") {
              i += 2;
              continue;
            }
            if (cc === '"') {
              i++;
              break;
            }
            i++;
          }
          continue;
        }
        if (c === "{" || c === "[") depth++;
        else if (c === "}" || c === "]") depth--;
        i++;
      }
      continue;
    }
    if (ch === "}") return;
    if (ch === '"') {
      const from = i;
      i++;
      while (i < src.length) {
        const c = src[i];
        if (c === "\\") {
          i += 2;
          continue;
        }
        if (c === '"') {
          i++;
          break;
        }
        i++;
      }
      const to = i;
      const raw = src.slice(from, to);
      let key: string;
      try {
        key = JSON.parse(raw) as string;
      } catch {
        key = raw.replace(/^"|"$/g, "");
      }
      yield { key, from, to };
      // Skip whitespace + `:` + the value (string / number /
      // object / array / literal) so the next iteration lands on
      // either `,` or `}`.
      while (i < src.length && /\s/.test(src[i] ?? "")) i++;
      if (src[i] === ":") i++;
      while (i < src.length && /\s/.test(src[i] ?? "")) i++;
      // Skip the value.
      const vc = src[i];
      if (vc === '"') {
        i++;
        while (i < src.length) {
          const c = src[i];
          if (c === "\\") {
            i += 2;
            continue;
          }
          if (c === '"') {
            i++;
            break;
          }
          i++;
        }
      } else if (vc === "{" || vc === "[") {
        depth = 1;
        i++;
        while (i < src.length && depth > 0) {
          const c = src[i];
          if (c === '"') {
            i++;
            while (i < src.length) {
              const cc = src[i];
              if (cc === "\\") {
                i += 2;
                continue;
              }
              if (cc === '"') {
                i++;
                break;
              }
              i++;
            }
            continue;
          }
          if (c === "{" || c === "[") depth++;
          else if (c === "}" || c === "]") depth--;
          i++;
        }
      } else {
        while (
          i < src.length &&
          src[i] !== "," &&
          src[i] !== "}" &&
          src[i] !== "\n"
        ) {
          i++;
        }
      }
      continue;
    }
    // Unrecognised at top level — bail to avoid infinite loops.
    i++;
  }
}

/** Find the offset of the matching closing `}` of the top-level object. */
function findTopLevelClose(src: string): number {
  let i = 0;
  while (i < src.length && /\s/.test(src[i] ?? "")) i++;
  if (src[i] !== "{") return -1;
  let depth = 0;
  let inString = false;
  for (let j = i; j < src.length; j++) {
    const c = src[j];
    if (inString) {
      if (c === "\\") {
        j++;
        continue;
      }
      if (c === '"') inString = false;
      continue;
    }
    if (c === '"') {
      inString = true;
      continue;
    }
    if (c === "{") depth++;
    else if (c === "}") {
      depth--;
      if (depth === 0) return j;
    }
  }
  return src.length;
}

/**
 * Linter that surfaces `allowedKeys` / `requiredKeys` violations.
 * Falls silent when neither constraint is set (no work, no noise).
 */
export const keyConstraintsLinter = linter((view) => {
  const cfg = getKeyConstraints(view.state);
  const allowed = cfg.allowedKeys;
  const required = cfg.requiredKeys;
  if (!allowed?.length && !required?.length) return [];

  const src = view.state.doc.toString();
  if (!src.trim()) return [];

  const diags: Diagnostic[] = [];
  const seen = new Set<string>();
  const allowedSet = allowed ? new Set(allowed) : null;
  let sawObject = false;

  for (const { key, from, to } of topLevelKeys(src)) {
    sawObject = true;
    seen.add(key);
    if (allowedSet && !allowedSet.has(key)) {
      diags.push({
        from,
        to,
        severity: "error",
        message: `Key "${key}" is not allowed here.${
          allowed && allowed.length > 0
            ? ` Allowed keys: ${allowed.map((k) => `"${k}"`).join(", ")}.`
            : ""
        }`,
      });
    }
  }

  if (sawObject && required && required.length > 0) {
    const missing = required.filter((k) => !seen.has(k));
    if (missing.length > 0) {
      const close = findTopLevelClose(src);
      const from = close >= 0 ? close : Math.max(src.length - 1, 0);
      diags.push({
        from,
        to: Math.min(from + 1, src.length),
        severity: "warning",
        message:
          missing.length === 1
            ? `Missing required key "${missing[0]}".`
            : `Missing required keys: ${missing.map((k) => `"${k}"`).join(", ")}.`,
      });
    }
  }

  return diags;
});
