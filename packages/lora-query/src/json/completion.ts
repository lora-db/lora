import { Facet, type EditorState } from "@codemirror/state";
import {
  type CompletionContext,
  type CompletionResult,
} from "@codemirror/autocomplete";

/**
 * Host-provided completion data for the JSON payload editor.
 *
 * - `knownKeys`: top-level keys to suggest. The typical wiring point
 *   is "the names of `$param` references in a sibling Cypher query"
 *   so the payload editor autocompletes the keys it needs to fill.
 */
export interface LoraJsonProviders {
  knownKeys: readonly string[];
}

const EMPTY: LoraJsonProviders = { knownKeys: [] };

/**
 * Facet that carries the host's providers through the editor. The
 * provider Facet is the only point of coupling between the React
 * component and the CodeMirror extension layer — every completer
 * pulls from `state.facet(loraJsonProviders)`.
 */
export const loraJsonProviders = Facet.define<
  LoraJsonProviders,
  LoraJsonProviders
>({
  combine(values) {
    return values.length === 0 ? EMPTY : (values[0] ?? EMPTY);
  },
});

export function getProviders(state: EditorState): LoraJsonProviders {
  return state.facet(loraJsonProviders);
}

const KEY_CHAR_RE = /[A-Za-z0-9_\-$]/;

/**
 * Find the position of the most recent unmatched `{` or `[` before
 * `pos`. Used to decide whether the cursor is at a key position
 * (inside an object) — we only surface key completions when the
 * enclosing opener is `{`.
 *
 * Returns `null` when nothing relevant was found (cursor at the top
 * level, or inside an array).
 */
function findEnclosingOpener(doc: string, pos: number): "{" | "[" | null {
  let depth = 0;
  let inString: '"' | "'" | null = null;
  for (let i = pos - 1; i >= 0; i--) {
    const ch = doc[i];
    if (inString) {
      // Skip the entire string from its closing quote backwards.
      if (ch === inString && doc[i - 1] !== "\\") inString = null;
      continue;
    }
    if (ch === '"' || ch === "'") {
      inString = ch;
      continue;
    }
    if (ch === "}" || ch === "]") {
      depth++;
      continue;
    }
    if (ch === "{" || ch === "[") {
      if (depth === 0) return ch;
      depth--;
    }
  }
  return null;
}

/**
 * Completion source for top-level object keys. Fires when:
 *   1. The cursor sits inside an object literal (the nearest
 *      unmatched opener is `{`), AND
 *   2. The cursor is at the start of a key — i.e. the preceding
 *      non-whitespace character is `{`, `,`, or a `"` that begins a
 *      key being typed.
 *
 * Suggestions are rendered as `"key": ` so the user lands at the
 * value position with a single accept.
 */
export function jsonCompletions(
  ctx: CompletionContext,
): CompletionResult | null {
  const { state, pos, explicit } = ctx;
  const providers = getProviders(state);
  if (providers.knownKeys.length === 0) return null;

  const doc = state.doc.toString();

  // Token under the caret. CodeMirror's default word regex doesn't
  // include `"`, so we widen it: a partial token may be either a
  // bare identifier or a leading `"` plus identifier chars.
  let from = pos;
  while (from > 0) {
    const ch = doc[from - 1];
    if (ch && (KEY_CHAR_RE.test(ch) || ch === '"')) {
      from--;
    } else {
      break;
    }
  }
  const tokenText = doc.slice(from, pos);
  // The user is either typing nothing yet (require `explicit` to fire),
  // or typing characters that could begin/extend a JSON key.
  if (!explicit && tokenText.length === 0) return null;

  // Verify the cursor is at a *key* position. Walk backwards through
  // whitespace; the next non-whitespace char must be `{` (first key)
  // or `,` (subsequent key) or `"` (we're inside a partially-typed
  // quoted key).
  let probe = from - 1;
  while (probe >= 0 && /\s/.test(doc[probe] ?? "")) probe--;
  const prev = probe >= 0 ? doc[probe] : "";
  const atKeyPos =
    prev === "{" ||
    prev === "," ||
    // If we already consumed a leading `"`, the prev char (before the
    // `"`) is the relevant one.
    (tokenText.startsWith('"') &&
      (prev === "{" || prev === "," || prev === ""));

  if (!atKeyPos) {
    // Special-case: if the token text itself starts with `"` and the
    // character before *that* is `{` or `,`, accept it.
    if (tokenText.startsWith('"')) {
      const before = from - 1;
      let p = before;
      while (p >= 0 && /\s/.test(doc[p] ?? "")) p--;
      const c = p >= 0 ? doc[p] : "";
      if (c !== "{" && c !== "," && c !== "") return null;
    } else {
      return null;
    }
  }

  // Confirm the enclosing opener is an object, not an array.
  const opener = findEnclosingOpener(doc, from);
  if (opener !== "{") return null;

  // Detect whether the line already has a `:` after the cursor — if
  // so, we don't append `": "` since the structure is in place.
  const lineEnd = doc.indexOf("\n", pos);
  const tail = doc.slice(pos, lineEnd === -1 ? doc.length : lineEnd);
  const colonAlreadyThere = /^\s*"?\s*:/.test(tail);

  // Skip keys that are already present in the enclosing object — we
  // only walk the current line + nearby lines, so this is a best-
  // effort dedupe rather than a true scope check.
  const used = collectSiblingKeys(doc, pos);

  return {
    from,
    to: pos,
    filter: true,
    options: providers.knownKeys
      .filter((k) => !used.has(k))
      .map((key) => ({
        label: key,
        apply: colonAlreadyThere ? `"${key}"` : `"${key}": `,
        type: "property",
      })),
  };
}

/**
 * Walk backwards + forwards within the current object literal and
 * collect already-used keys. Best-effort — a simple regex over the
 * lines that obviously belong to the same object scope, which is
 * enough to avoid suggesting a key the user has already typed.
 */
function collectSiblingKeys(doc: string, pos: number): Set<string> {
  const used = new Set<string>();
  // Bound: walk backwards to the enclosing `{` and forwards to the
  // matching `}`. Bail on mismatched braces — we'd rather under-
  // dedupe than crash.
  let depth = 0;
  let start = -1;
  for (let i = pos - 1; i >= 0; i--) {
    const ch = doc[i];
    if (ch === "}" || ch === "]") depth++;
    else if (ch === "{" || ch === "[") {
      if (depth === 0) {
        start = i;
        break;
      }
      depth--;
    }
  }
  let end = doc.length;
  depth = 0;
  for (let i = pos; i < doc.length; i++) {
    const ch = doc[i];
    if (ch === "{" || ch === "[") depth++;
    else if (ch === "}" || ch === "]") {
      if (depth === 0) {
        end = i;
        break;
      }
      depth--;
    }
  }
  if (start === -1) return used;
  const slice = doc.slice(start + 1, end);
  const re = /"([A-Za-z0-9_\-$]+)"\s*:/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(slice)) !== null) {
    if (m[1]) used.add(m[1]);
  }
  return used;
}
