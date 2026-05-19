import type { EditorView } from "@codemirror/view";

/**
 * Custom fold placeholder for the JSON editor. Replaces the default
 * `…` with `{ N items }` / `[ N items ]` so a collapsed object or
 * array tells you at a glance how many entries it holds.
 *
 * The count is approximate: we walk the folded slice and count
 * commas at the matching depth, then add one for the first entry.
 * This is fast (no full parse needed) and gives the right answer
 * for well-formed JSON.
 */

export function jsonFoldPlaceholderDOM(
  _view: EditorView,
  onclick: (event: Event) => void,
  prepared: string,
): HTMLElement {
  const el = document.createElement("span");
  el.className = "cm-lora-json-fold-placeholder";
  el.setAttribute("role", "button");
  el.setAttribute("aria-label", "unfold");
  el.title = "unfold";
  el.textContent = prepared;
  el.onclick = onclick;
  return el;
}

/**
 * Pre-compute the placeholder text from the slice that's about to
 * be folded. Returning a string here means it's serialisable
 * across the runtime boundary — CodeMirror caches it.
 */
export function jsonFoldPreparePlaceholder(
  state: { doc: { sliceString: (from: number, to: number) => string } },
  range: { from: number; to: number },
): string {
  const slice = state.doc.sliceString(range.from, range.to);
  if (slice.length === 0) return "…";

  // Look at the first non-whitespace char to decide what shape the
  // fold contains. The CodeMirror fold ranges for json open after
  // `{` / `[`, so `slice` typically starts with whitespace or the
  // first key/element.
  const opener = guessOpener(slice);
  const count = countTopLevelEntries(slice);
  if (opener === "object") {
    return `{ ${count} ${count === 1 ? "key" : "keys"} }`;
  }
  if (opener === "array") {
    return `[ ${count} ${count === 1 ? "item" : "items"} ]`;
  }
  return `… ${count} ${count === 1 ? "entry" : "entries"}`;
}

function guessOpener(slice: string): "object" | "array" | null {
  // CodeMirror's JSON fold service folds the *body* of `{}` and
  // `[]`. We inspect the first non-whitespace token: a quote
  // (object key) or any of `{[` or a literal value (array entry).
  for (let i = 0; i < slice.length; i++) {
    const c = slice[i];
    if (c === " " || c === "\t" || c === "\n" || c === "\r") continue;
    if (c === '"') return "object";
    if (c === "}" || c === "]") return null;
    // Numbers, true/false/null, `{`, `[` — those are array entries
    // when sitting at the slice's top level.
    return "array";
  }
  return null;
}

function countTopLevelEntries(slice: string): number {
  let count = 0;
  let depth = 0;
  let inString = false;
  let sawContent = false;
  for (let i = 0; i < slice.length; i++) {
    const c = slice[i];
    if (inString) {
      if (c === "\\") {
        i++;
        continue;
      }
      if (c === '"') inString = false;
      sawContent = true;
      continue;
    }
    if (c === '"') {
      inString = true;
      sawContent = true;
      continue;
    }
    if (c === "{" || c === "[") {
      depth++;
      sawContent = true;
      continue;
    }
    if (c === "}" || c === "]") {
      depth = Math.max(0, depth - 1);
      continue;
    }
    if (c === ",") {
      if (depth === 0) count++;
      continue;
    }
    if (c === " " || c === "\t" || c === "\n" || c === "\r") continue;
    sawContent = true;
  }
  // Each top-level comma separates two entries → entries = commas+1
  // when any content is present.
  return sawContent ? count + 1 : 0;
}
