import { describe, expect, it } from "vitest";
import { EditorView } from "@codemirror/view";
import { EditorState, EditorSelection } from "@codemirror/state";
import { cypherVariableReferences } from "../src/cypher/references";
import {
  _setHighlightSpansEffect,
  astDecorations,
} from "../src/cypher/decoration";
import type { HighlightSpan } from "../src/parser";

/**
 * Build a minimal EditorState carrying the references extension and
 * seed it with synthetic highlight spans so we can assert decoration
 * behaviour without waiting for the WASM walker.
 */
function build(doc: string, spans: HighlightSpan[], cursor: number) {
  let state = EditorState.create({
    doc,
    selection: EditorSelection.cursor(cursor),
    extensions: [astDecorations, cypherVariableReferences],
  });
  state = state.update({ effects: _setHighlightSpansEffect.of(spans) }).state;
  return state;
}

/** Extract the (from, to) ranges where the active-variable mark is set. */
function activeRanges(state: EditorState): Array<[number, number]> {
  // The references field is private; round-trip through the view's
  // decoration provider by mounting a headless EditorView.
  const view = new EditorView({ state });
  const decos = view.state.facet(EditorView.decorations);
  const out: Array<[number, number]> = [];
  for (const d of decos) {
    const set = typeof d === "function" ? d(view) : d;
    set.between(0, view.state.doc.length, (from, to, value) => {
      const spec = value.spec as { class?: string } | undefined;
      if (spec?.class === "cm-lora-variable-active") {
        out.push([from, to]);
      }
    });
  }
  view.destroy();
  out.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  return out;
}

describe("cypherVariableReferences", () => {
  it("highlights every reference of the variable under the caret", () => {
    const doc = "MATCH (n:Person) WHERE n.age > 30 RETURN n";
    // Variable spans: the two `n` occurrences before `.age` and after
    // RETURN, plus the declaration in the node pattern.
    const spans: HighlightSpan[] = [
      { kind: "variable", start: 7, end: 8 }, // n in MATCH (n:Person)
      { kind: "variable", start: 23, end: 24 }, // n in WHERE n.age
      { kind: "variable", start: 41, end: 42 }, // n in RETURN n
    ];
    // Cursor inside the first `n`.
    const ranges = activeRanges(build(doc, spans, 7));
    expect(ranges).toEqual([
      [7, 8],
      [23, 24],
      [41, 42],
    ]);
  });

  it("does nothing when the caret is on a non-variable token", () => {
    const doc = "MATCH (n:Person) RETURN n";
    const spans: HighlightSpan[] = [
      { kind: "variable", start: 7, end: 8 },
      { kind: "label", start: 9, end: 15 }, // :Person
      { kind: "variable", start: 24, end: 25 },
    ];
    // Cursor on the `:Person` label.
    const ranges = activeRanges(build(doc, spans, 11));
    expect(ranges).toEqual([]);
  });

  it("does not match a same-name property key", () => {
    // `name` appears once as a property key (skipped) and once as a
    // variable. Caret on the variable must NOT highlight the key.
    const doc = "MATCH ({name: 'A'}) WITH name AS n RETURN n";
    const spans: HighlightSpan[] = [
      { kind: "propertyKey", start: 8, end: 12 }, // name: 'A'
      { kind: "variable", start: 25, end: 29 }, // WITH name
      { kind: "variable", start: 33, end: 34 }, // AS n
      { kind: "variable", start: 42, end: 43 }, // RETURN n
    ];
    // Caret on `name` variable in WITH.
    const ranges = activeRanges(build(doc, spans, 27));
    // Only the one variable occurrence of `name` exists → no other
    // refs to highlight, so the decoration set is empty.
    expect(ranges).toEqual([]);
  });

  it("only matches identical names — shadowed neighbours stay calm", () => {
    const doc = "MATCH (a)-[r]->(b) RETURN a, b";
    const spans: HighlightSpan[] = [
      { kind: "variable", start: 7, end: 8 }, // a
      { kind: "variable", start: 11, end: 12 }, // r
      { kind: "variable", start: 16, end: 17 }, // b
      { kind: "variable", start: 26, end: 27 }, // a
      { kind: "variable", start: 29, end: 30 }, // b
    ];
    // Caret on the first `a` — only the two `a` spans should fire.
    const ranges = activeRanges(build(doc, spans, 7));
    expect(ranges).toEqual([
      [7, 8],
      [26, 27],
    ]);
  });

  it("single occurrence yields no decorations", () => {
    const doc = "MATCH (n) RETURN x";
    const spans: HighlightSpan[] = [
      { kind: "variable", start: 7, end: 8 }, // n declared but never re-used
      { kind: "variable", start: 17, end: 18 }, // x — undeclared but still a span
    ];
    const ranges = activeRanges(build(doc, spans, 7));
    // Only one `n` occurrence → nothing to highlight.
    expect(ranges).toEqual([]);
  });
});
