import {
  RangeSetBuilder,
  StateField,
  type EditorState,
  type Extension,
} from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView } from "@codemirror/view";
import { getHighlightSpans } from "./decoration";

/**
 * Pulse-style highlighting of variable references. When the caret
 * lands on (or selects) a variable identifier, every other reference
 * to the same variable in the document gets a subtle background
 * tint — matching what IDEs do for `let x` jump-around.
 *
 * The implementation reads the AST-driven highlight spans stored on
 * the editor state (see `decoration.ts`), so it inherits the parser's
 * notion of "variable": labels (`:Person`), property keys
 * (`{name: ...}`) and rel-types (`-[:KNOWS]-`) are excluded by
 * construction. While the WASM parse hasn't caught up yet — fresh
 * doc, mid-keystroke — the highlight spans are empty and this plugin
 * is a no-op.
 */

const referencesField = StateField.define<DecorationSet>({
  create: (state) => computeReferences(state),
  update(value, tr) {
    // Recompute when the caret moves, the doc changes, or the
    // highlight-spans field has been refreshed by the AST watcher.
    // For everything else, just map the existing decorations through
    // the change set so they ride along with edits.
    if (
      tr.docChanged ||
      tr.selection ||
      tr.effects.length > 0
    ) {
      return computeReferences(tr.state);
    }
    return value.map(tr.changes);
  },
  provide: (f) => EditorView.decorations.from(f),
});

const referenceMark = Decoration.mark({ class: "cm-lora-variable-active" });

/** Compute the variable-reference decorations for the current state. */
function computeReferences(state: EditorState): DecorationSet {
  const sel = state.selection.main;
  const spans = getHighlightSpans(state);
  if (!spans.length) return Decoration.none;
  const doc = state.doc;
  const docLen = doc.length;

  // Find the variable span the caret is currently inside. Use the
  // selection head; if the user has a range selection it must sit
  // wholly within a single variable for the highlight to fire (mirrors
  // IDE conventions — partial selections suppress the pulse).
  const pos = sel.head;
  const cursor = spans.find(
    (s) =>
      s.kind === "variable" &&
      pos >= s.start &&
      pos <= s.end &&
      s.end <= docLen,
  );
  if (!cursor) return Decoration.none;
  if (!sel.empty) {
    // Selection range must fit inside the same variable span.
    if (sel.from < cursor.start || sel.to > cursor.end) return Decoration.none;
  }

  const name = doc.sliceString(cursor.start, cursor.end);
  if (!name) return Decoration.none;

  // Collect every other variable span carrying the same identifier
  // and emit a single mark per span.
  const matches: Array<{ from: number; to: number }> = [];
  for (const s of spans) {
    if (s.kind !== "variable") continue;
    const from = Math.max(0, Math.min(s.start, docLen));
    const to = Math.max(from, Math.min(s.end, docLen));
    if (from === to) continue;
    if (doc.sliceString(from, to) !== name) continue;
    matches.push({ from, to });
  }
  if (matches.length < 2) return Decoration.none;

  matches.sort((a, b) => a.from - b.from || a.to - b.to);
  const builder = new RangeSetBuilder<Decoration>();
  for (const m of matches) builder.add(m.from, m.to, referenceMark);
  return builder.finish();
}

export const cypherVariableReferences: Extension = [referencesField];
