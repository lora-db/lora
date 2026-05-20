import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
} from "@codemirror/view";
import {
  RangeSetBuilder,
  StateEffect,
  StateField,
  type EditorState,
  type Extension,
} from "@codemirror/state";
import { highlight, type HighlightSpan } from "../parser";

const KIND_TO_CLASS: Record<HighlightSpan["kind"], string> = {
  variable: "cm-lora-variable",
  parameter: "cm-lora-parameter",
  label: "cm-lora-label",
  relType: "cm-lora-rel-type",
  propertyKey: "cm-lora-property-key",
  functionName: "cm-lora-function",
  namespace: "cm-lora-namespace",
  stringLiteral: "cm-lora-string",
  numberLiteral: "cm-lora-number",
  boolLiteral: "cm-lora-bool",
  nullLiteral: "cm-lora-null",
  keyword: "cm-lora-keyword",
};

/** Effect that swaps in a freshly-computed decoration set. */
const setDecorations = StateEffect.define<DecorationSet>();

/**
 * Effect that swaps in a fresh batch of highlight spans. Exported as
 * `_setHighlightSpansEffect` (underscored) so tests can seed the
 * state directly without mounting an editor and waiting for the
 * debounced WASM walker.
 */
export const _setHighlightSpansEffect = StateEffect.define<HighlightSpan[]>();
const setSpans = _setHighlightSpansEffect;

/**
 * StateField holding the AST-driven decorations. Driven by the
 * `astDecorationWatcher` plugin below — dispatched updates carry the
 * new set as a `StateEffect`, which keeps text selection intact (an
 * empty `view.dispatch({})` would force a re-render that can drop
 * mid-drag selections).
 */
const astDecorationField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    let next = value.map(tr.changes);
    for (const effect of tr.effects) {
      if (effect.is(setDecorations)) next = effect.value;
    }
    return next;
  },
  provide: (f) => EditorView.decorations.from(f),
});

/**
 * StateField holding the latest raw AST highlight spans. Consumed by
 * cross-cutting features (the variable-reference highlighter) that
 * need to classify a position without re-parsing the document.
 *
 * Spans are kept in the order produced by the WASM walker. Positions
 * are *not* mapped through subsequent document changes — features
 * that read this field should consider the spans authoritative only
 * until the next debounced refresh fires (~150 ms after a keystroke).
 */
const highlightSpansField = StateField.define<HighlightSpan[]>({
  create: () => [],
  update(value, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setSpans)) return effect.value;
    }
    return tr.docChanged ? [] : value;
  },
});

export function getHighlightSpans(state: EditorState): HighlightSpan[] {
  return state.field(highlightSpansField, false) ?? [];
}

const astDecorationWatcher = ViewPlugin.fromClass(
  class {
    private pending: ReturnType<typeof setTimeout> | null = null;
    private generation = 0;

    constructor(view: EditorView) {
      this.schedule(view, 0);
    }

    update(update: ViewUpdate) {
      if (update.docChanged) this.schedule(update.view, 150);
    }

    private schedule(view: EditorView, delay: number) {
      if (this.pending) clearTimeout(this.pending);
      const gen = ++this.generation;
      this.pending = setTimeout(() => {
        this.pending = null;
        const source = view.state.doc.toString();
        if (!source) {
          view.dispatch({ effects: setDecorations.of(Decoration.none) });
          return;
        }
        highlight(source)
          .then((spans) => {
            if (gen !== this.generation) return;
            if (!spans.length) return;
            const next = build(spans, view.state.doc.length);
            view.dispatch({
              effects: [setDecorations.of(next), setSpans.of(spans)],
            });
          })
          .catch(() => {});
      }, delay);
    }

    destroy() {
      if (this.pending) clearTimeout(this.pending);
    }
  },
);

function build(spans: HighlightSpan[], docLen: number): DecorationSet {
  // RangeSetBuilder requires monotonic positions. The Rust side emits
  // spans in AST traversal order, which is *mostly* sorted by start —
  // but nested constructs (e.g. property keys inside a node pattern)
  // can produce a later span whose start sits before its parent's end.
  // The previous implementation silently dropped them; we instead
  // clamp+sort and emit every legitimate span in document order so
  // nested decorations actually render.
  const clamped: Array<{ from: number; to: number; cls: string }> = [];
  for (const span of spans) {
    const cls = KIND_TO_CLASS[span.kind];
    if (!cls) continue;
    const from = Math.max(0, Math.min(span.start, docLen));
    const to = Math.max(from, Math.min(span.end, docLen));
    if (from === to) continue;
    clamped.push({ from, to, cls });
  }
  clamped.sort((a, b) => a.from - b.from || a.to - b.to);
  const builder = new RangeSetBuilder<Decoration>();
  for (const r of clamped) {
    builder.add(r.from, r.to, Decoration.mark({ class: r.cls }));
  }
  return builder.finish();
}

export const astDecorations: Extension = [
  astDecorationField,
  highlightSpansField,
  astDecorationWatcher,
];
