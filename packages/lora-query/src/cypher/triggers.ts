import { startCompletion } from "@codemirror/autocomplete";
import { EditorView, type ViewUpdate } from "@codemirror/view";

/** Single-char triggers that fire completion unconditionally. */
const ALWAYS_TRIGGERS = new Set([":", ".", "$", "{", "(", ")", "]"]);

/**
 * Patterns matched against the text immediately preceding the cursor.
 * If one matches after a space is typed, we open completion. This is
 * how we surface NULL/NOT NULL after `IS `, RETURN auto-fill, ORDER BY
 * smarts, and so on — without spamming the popup on every space.
 */
const SPACE_TRIGGER_PATTERNS: RegExp[] = [
  /\bIS\s$/i,
  /\bRETURN\s$/i,
  /\bWITH\s$/i,
  /\bORDER\s+BY\s$/i,
  /\bWHERE\s$/i,
  /\bSET\s$/i,
  /\bUNWIND\s$/i,
  /\bMATCH\s$/i,
  /\bCREATE\s$/i,
  /\bMERGE\s$/i,
  /\bYIELD\s$/i,
  /\bWHEN\s$/i,
  /\bTHEN\s$/i,
  /\bELSE\s$/i,
  /\bON\s+(?:CREATE|MATCH)\s+SET\s$/i,
  /\bON\s+(?:CREATE|MATCH)\s$/i,
  // After a value / closing delimiter / operator: comparison-op or
  // RHS-of-operator completions.
  /[A-Za-z_][\w]*(?:\.[A-Za-z_][\w]*)*\s$/,
  /[)\]]\s$/,
  /(?:=|<>|<|<=|>|>=|=~)\s$/,
  /\b(?:IN|STARTS WITH|ENDS WITH|CONTAINS)\s$/i,
];

/** Two-character compound triggers detected on the trailing edit. */
const COMPOUND_TRIGGERS: RegExp[] = [
  /(?:-->|<--|->|<-|--)$/, // pattern arrows — node pattern expected next
];

export const autoCompletionTriggers = EditorView.updateListener.of(
  (update: ViewUpdate) => {
    if (!update.docChanged) return;
    let triggerKind: "always" | "space" | "maybe-compound" | null = null;
    for (const tr of update.transactions) {
      tr.changes.iterChanges((_fa, _ta, _fb, _tb, inserted) => {
        if (triggerKind === "always") return;
        const text = inserted.toString();
        if (text.length !== 1) return;
        if (ALWAYS_TRIGGERS.has(text)) {
          triggerKind = "always";
        } else if (text === " ") {
          triggerKind = triggerKind ?? "space";
        } else if (text === ">" || text === "-") {
          triggerKind = triggerKind ?? "maybe-compound";
        }
      });
    }
    if (!triggerKind) return;
    if (triggerKind === "always") {
      queueMicrotask(() => startCompletion(update.view));
      return;
    }
    queueMicrotask(() => {
      const view = update.view;
      const head = view.state.selection.main.head;
      const lookback = view.state.doc.sliceString(Math.max(0, head - 40), head);
      if (triggerKind === "space") {
        if (SPACE_TRIGGER_PATTERNS.some((re) => re.test(lookback))) {
          startCompletion(view);
        }
        return;
      }
      // "maybe-compound" — the user typed `>` or `-`; check whether
      // the surrounding text now ends with a pattern arrow.
      if (COMPOUND_TRIGGERS.some((re) => re.test(lookback))) {
        startCompletion(view);
      }
    });
  },
);
