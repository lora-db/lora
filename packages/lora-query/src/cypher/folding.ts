import { StateEffect, StateField, type Extension } from "@codemirror/state";
import { EditorView, ViewPlugin, type ViewUpdate } from "@codemirror/view";
import { foldGutter, foldService } from "@codemirror/language";
import { analyse, type FoldRange } from "../parser";

/**
 * A top-level statement carved out of a multi-statement script.
 *
 * `start` and `end` are byte offsets into the original source; `text`
 * is the slice (with leading / trailing whitespace trimmed) so callers
 * can hand it straight to the WASM parser.
 */
export interface StatementSlice {
  start: number;
  end: number;
  text: string;
}

// One-entry memo. `validateAll`, `analyseAll`, and `detectQueryFolds`
// all run this on the same source within a parse cycle — caching keeps
// us to a single O(N) pass instead of three.
let _splitCache: { source: string; result: StatementSlice[] } | null = null;

/**
 * Split the doc into top-level statements at every `;` outside
 * strings, comments, and balanced delimiters. The trailing remainder
 * (everything past the last `;`) is included as its own slice when
 * non-blank, so single-statement queries without a trailing `;`
 * round-trip as a single slice.
 */
export function splitTopLevelStatements(source: string): StatementSlice[] {
  if (_splitCache && _splitCache.source === source) return _splitCache.result;
  const out: StatementSlice[] = [];
  const bytes = source.length;
  if (bytes === 0) return out;

  let state: "normal" | "single" | "double" | "back" | "line" | "block" =
    "normal";
  let depth = 0;
  let segStart = 0;
  const emit = (from: number, to: number) => {
    const s = trimStart(source, from, to);
    const e = trimEnd(source, from, to);
    if (e > s) out.push({ start: s, end: e, text: source.slice(s, e) });
  };
  for (let i = 0; i < bytes; i++) {
    const c = source[i];
    if (state === "normal") {
      if (c === "'") {
        state = "single";
        continue;
      }
      if (c === '"') {
        state = "double";
        continue;
      }
      if (c === "`") {
        state = "back";
        continue;
      }
      if (c === "/" && source[i + 1] === "/") {
        state = "line";
        i++;
        continue;
      }
      if (c === "/" && source[i + 1] === "*") {
        state = "block";
        i++;
        continue;
      }
      if (c === "(" || c === "[" || c === "{") {
        depth++;
        continue;
      }
      if (c === ")" || c === "]" || c === "}") {
        depth = Math.max(0, depth - 1);
        continue;
      }
      if (c === ";" && depth === 0) {
        emit(segStart, i);
        segStart = i + 1;
      }
      continue;
    }
    if (state === "single") {
      if (c === "\\") {
        i++;
        continue;
      }
      if (c === "'") state = "normal";
      continue;
    }
    if (state === "double") {
      if (c === "\\") {
        i++;
        continue;
      }
      if (c === '"') state = "normal";
      continue;
    }
    if (state === "back") {
      if (c === "`") state = "normal";
      continue;
    }
    if (state === "line") {
      if (c === "\n") state = "normal";
      continue;
    }
    if (state === "block") {
      if (c === "*" && source[i + 1] === "/") {
        state = "normal";
        i++;
      }
    }
  }
  // Always emit the trailing slice, even if we ended inside a string or
  // comment — the parser still needs the chance to point at the bad
  // token. Without this, an unterminated string would erase every
  // diagnostic past the opening quote.
  emit(segStart, bytes);
  _splitCache = { source, result: out };
  return out;
}

/**
 * Per-query fold ranges — every statement gets one `kind: "query"`
 * range so the gutter can collapse / expand it. Implementation is now
 * a thin wrapper over {@link splitTopLevelStatements}.
 */
export function detectQueryFolds(source: string): FoldRange[] {
  return splitTopLevelStatements(source).map((s) => ({
    start: s.start,
    end: s.end,
    kind: "query",
  }));
}

function trimStart(source: string, from: number, to: number): number {
  let i = from;
  while (i < to && /\s/.test(source[i]!)) i++;
  return i;
}
function trimEnd(source: string, from: number, to: number): number {
  let i = to;
  while (i > from && /\s/.test(source[i - 1]!)) i--;
  return i;
}

const setFoldRanges = StateEffect.define<readonly FoldRange[]>();

const foldRangeField = StateField.define<readonly FoldRange[]>({
  create: () => [],
  update(value, tr) {
    for (const e of tr.effects) {
      if (e.is(setFoldRanges)) return e.value;
    }
    return value;
  },
});

const foldWatcher = ViewPlugin.fromClass(
  class {
    private pending: ReturnType<typeof setTimeout> | null = null;
    private generation = 0;

    constructor(view: EditorView) {
      this.schedule(view, 0);
    }
    update(update: ViewUpdate) {
      if (update.docChanged) this.schedule(update.view, 250);
    }
    private schedule(view: EditorView, delay: number) {
      if (this.pending) clearTimeout(this.pending);
      const gen = ++this.generation;
      this.pending = setTimeout(() => {
        this.pending = null;
        const source = view.state.doc.toString();
        if (!source) {
          view.dispatch({ effects: setFoldRanges.of([]) });
          return;
        }
        // Seed with the per-query ranges immediately — they're cheap
        // and work even when the source doesn't parse.
        const seed = detectQueryFolds(source);
        view.dispatch({ effects: setFoldRanges.of(seed) });
        analyse(source)
          .then((a) => {
            if (gen !== this.generation) return;
            const merged: FoldRange[] = [...seed];
            for (const r of a.foldRanges) {
              // Skip duplicates of the per-query ranges.
              if (
                !merged.some(
                  (m) =>
                    m.start === r.start && m.end === r.end && m.kind === r.kind,
                )
              ) {
                merged.push(r);
              }
            }
            merged.sort((a, b) => a.start - b.start);
            view.dispatch({ effects: setFoldRanges.of(merged) });
          })
          .catch(() => {});
      }, delay);
    }
    destroy() {
      if (this.pending) clearTimeout(this.pending);
    }
  },
);

/**
 * Pick the chevron range for a line. Exported so unit tests can hit it
 * without spinning up an EditorState — the foldService below is a thin
 * wrapper that just feeds this with the current `foldRangeField` value.
 *
 * Picks the *largest* range that starts on this line and extends past
 * it. Largest wins so the chevron on the first line of a multi-clause
 * query collapses the whole query (rather than just its opening
 * `MATCH` clause, which can also start at the same offset). Inner
 * clause folds remain reachable via the chevrons on their own starting
 * lines (`WHERE`, `WITH`, …).
 */
export function pickFoldForLine(
  ranges: readonly FoldRange[],
  lineStart: number,
  lineEnd: number,
): { from: number; to: number } | null {
  let best: { from: number; to: number } | null = null;
  for (const r of ranges) {
    if (r.start >= lineStart && r.start <= lineEnd && r.end > lineEnd) {
      const candidate = { from: lineEnd, to: r.end };
      if (!best || candidate.to - candidate.from > best.to - best.from) {
        best = candidate;
      }
    }
  }
  return best;
}

const cypherFoldService = foldService.of((state, lineStart, lineEnd) =>
  pickFoldForLine(state.field(foldRangeField, false) ?? [], lineStart, lineEnd),
);

export const cypherFolding: Extension = [
  foldRangeField,
  foldWatcher,
  cypherFoldService,
  foldGutter({
    // CodeMirror's foldGutter only rebuilds gutter markers when its
    // built-in change predicates fire (doc, viewport, language facet,
    // syntax tree, fold state). Our async WASM-driven `foldRangeField`
    // is none of those, so without this opt-in the chevrons would
    // never appear: the gutter built its markers at construction time
    // when the field was still empty. `foldingChanged` lets us tell
    // CodeMirror "rebuild when our field changes too".
    foldingChanged: (update) =>
      update.startState.field(foldRangeField, false) !==
      update.state.field(foldRangeField, false),
  }),
];
