import type { KeyBinding } from "@codemirror/view";
import type { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { startCompletion } from "@codemirror/autocomplete";

/**
 * Smart `Enter` behaviour for the JSON editor. Three cases:
 *
 *  1. Cursor sits between `{` and `}` (or `[` and `]`) — split the
 *     braces onto separate lines and put the caret on a new
 *     properly-indented middle line, mirroring what IDEs do for
 *     JS object literals.
 *
 *  2. Cursor sits right after a `,` inside an object — start the
 *     next key by inserting a newline + `"` and triggering the
 *     autocomplete popup (which surfaces `knownKeys` if the host
 *     supplied them).
 *
 *  3. Cursor sits right after a `,` inside an array — newline at
 *     the array's indent so the next element starts cleanly.
 *
 * Everything else falls through to CodeMirror's default Enter so
 * the editor still behaves predictably inside string literals,
 * line comments (n/a in strict JSON), etc.
 */

/** Detect the kind of bracket the caret is enclosed by. */
function findEnclosingOpener(
  doc: string,
  pos: number,
): { ch: "{" | "[" | null; indent: string } {
  let depth = 0;
  let inString: '"' | null = null;
  for (let i = pos - 1; i >= 0; i--) {
    const ch = doc[i];
    if (inString) {
      if (ch === inString && doc[i - 1] !== "\\") inString = null;
      continue;
    }
    if (ch === '"') {
      inString = ch;
      continue;
    }
    if (ch === "}" || ch === "]") {
      depth++;
      continue;
    }
    if (ch === "{" || ch === "[") {
      if (depth === 0) {
        // Read this opener's own indent (everything on its line up
        // to but not including the opener).
        let lineStart = i;
        while (lineStart > 0 && doc[lineStart - 1] !== "\n") lineStart--;
        const indent = doc.slice(lineStart, i).replace(/[^\s].*$/, "");
        return { ch: ch as "{" | "[", indent };
      }
      depth--;
    }
  }
  return { ch: null, indent: "" };
}

/**
 * Best-effort indent step probe. Reads the first non-empty line
 * after `pos` for its leading whitespace and falls back to two
 * spaces. Two spaces is what the prettifier emits by default, so
 * mixed buffers still feel right.
 */
function detectIndentStep(state: EditorState): string {
  const doc = state.doc.toString();
  // Walk lines looking for one that starts deeper than its
  // predecessor — that delta is the indent step.
  let prevIndent = -1;
  for (let i = 1; i <= state.doc.lines && i <= 200; i++) {
    const text = state.doc.line(i).text;
    if (text.trim() === "") continue;
    const m = text.match(/^[ \t]*/);
    const len = m ? m[0].length : 0;
    if (prevIndent >= 0 && len > prevIndent) {
      return " ".repeat(len - prevIndent);
    }
    prevIndent = len;
  }
  void doc;
  return "  ";
}

function smartEnter(view: EditorView): boolean {
  const { state } = view;
  const sel = state.selection.main;
  if (!sel.empty) return false;
  const pos = sel.from;
  const doc = state.doc.toString();

  // Walk just past whitespace either side of the caret to find the
  // nearest non-whitespace characters. That's what decides which
  // "smart" case we're in.
  let prev = pos - 1;
  while (prev >= 0 && (doc[prev] === " " || doc[prev] === "\t")) prev--;
  let next = pos;
  while (next < doc.length && (doc[next] === " " || doc[next] === "\t")) {
    next++;
  }
  const prevCh = prev >= 0 ? doc[prev] : "";
  const nextCh = next < doc.length ? doc[next] : "";

  // Bail if the caret sits inside a string literal — count
  // unescaped quotes on the current line up to the caret. Odd
  // means we're inside a string, hand off to default Enter.
  const line = state.doc.lineAt(pos);
  const lineBefore = doc.slice(line.from, pos);
  let quoteCount = 0;
  for (let i = 0; i < lineBefore.length; i++) {
    if (lineBefore[i] === '"' && lineBefore[i - 1] !== "\\") quoteCount++;
  }
  if (quoteCount % 2 === 1) return false;

  const step = detectIndentStep(state);
  const baseIndent = (line.text.match(/^[ \t]*/)?.[0] ?? "");

  // Case 1: between `{` and `}` or `[` and `]`. Split onto three
  // lines with the inner line indented one step deeper.
  if (
    (prevCh === "{" && nextCh === "}") ||
    (prevCh === "[" && nextCh === "]")
  ) {
    const inner = baseIndent + step;
    const insert = `\n${inner}\n${baseIndent}`;
    view.dispatch({
      changes: { from: pos, to: pos, insert },
      selection: { anchor: pos + 1 + inner.length },
      scrollIntoView: true,
      userEvent: "input.smartEnter",
    });
    return true;
  }

  // Case 2/3: immediately after a `,`. Enclosing opener decides
  // whether to seed a quote for the next key (object) or just
  // produce a clean newline (array).
  if (prevCh === ",") {
    const opener = findEnclosingOpener(doc, prev);
    if (opener.ch === "{") {
      const insert = `\n${baseIndent}"`;
      view.dispatch({
        changes: { from: pos, to: pos, insert },
        selection: { anchor: pos + insert.length },
        scrollIntoView: true,
        userEvent: "input.smartEnter",
      });
      // Fire the autocomplete popup so the host's `knownKeys` (if
      // any) surface immediately.
      window.setTimeout(() => {
        try {
          startCompletion(view);
        } catch {
          /* extension not active — fine, the input still lands. */
        }
      }, 0);
      return true;
    }
    if (opener.ch === "[") {
      const insert = `\n${baseIndent}`;
      view.dispatch({
        changes: { from: pos, to: pos, insert },
        selection: { anchor: pos + insert.length },
        scrollIntoView: true,
        userEvent: "input.smartEnter",
      });
      return true;
    }
  }

  // Default Enter for every other case.
  return false;
}

/**
 * Binding for the default keymap. Attach at `Prec.high` so it
 * wins over the standard Enter that lives in `defaultKeymap`.
 */
export const jsonSmartEnter: KeyBinding = {
  key: "Enter",
  run: smartEnter,
};
