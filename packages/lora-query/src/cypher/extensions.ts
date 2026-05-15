import {
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  completionKeymap,
} from "@codemirror/autocomplete";
import {
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
} from "@codemirror/commands";
import {
  HighlightStyle,
  bracketMatching,
  indentOnInput,
  syntaxHighlighting,
} from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { lintGutter, lintKeymap } from "@codemirror/lint";
import { search, searchKeymap } from "@codemirror/search";
import { EditorState, type Extension } from "@codemirror/state";
import {
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  placeholder as placeholderExt,
} from "@codemirror/view";
import { loraQueryLanguage } from "../highlight";
import { astDecorations } from "./decoration";
import { cypherCompletions } from "./completion";
import { cypherFolding } from "./folding";
import { cypherHover } from "./hover";
import { cypherLinter } from "./linter";
import { cypherNavigation } from "./navigation";
import { cypherVariableReferences } from "./references";
import { outlineExtension } from "./scope";
import { signatureHint } from "./signatureHint";
import { autoCompletionTriggers } from "./triggers";

/**
 * Map StreamLanguage tag classes onto our `cm-lora-*` CSS classes so
 * keywords / strings / numbers / comments produced by the synchronous
 * tokenizer pick up the same colour variables the AST decorations use.
 *
 * The StreamLanguage tokenizer is the only thing that runs on every
 * keystroke; the WASM parse + decoration roundtrip is debounced and
 * may not have caught up yet. Without this style the keystroke-to-
 * colour latency would have keywords like `SET` / `LIMIT` / `AS`
 * staying black until the parse settles — or forever, for slices
 * that don't parse.
 */
const cypherHighlightStyle = HighlightStyle.define([
  { tag: [t.keyword, t.controlKeyword, t.modifier, t.definitionKeyword], class: "cm-lora-keyword" },
  { tag: [t.atom, t.bool, t.null], class: "cm-lora-bool" },
  { tag: t.string, class: "cm-lora-string" },
  { tag: t.number, class: "cm-lora-number" },
  { tag: t.comment, class: "cm-lora-comment" },
  { tag: t.operator, class: "cm-lora-operator" },
  { tag: t.punctuation, class: "cm-lora-punct" },
  // `function` is what the StreamLanguage emits for an identifier
  // immediately followed by `(`. Routing it to cm-lora-function gives
  // every call site a distinct colour (currently green) without
  // waiting for the WASM AST to resolve.
  { tag: [t.function(t.variableName), t.function(t.propertyName)], class: "cm-lora-function" },
]);

export interface CypherExtensionsOptions {
  readOnly?: boolean;
  /** Toggle the line-number gutter. Defaults to `true`. */
  showLineNumbers?: boolean;
  /** Placeholder text shown when the buffer is empty. */
  placeholder?: string;
}

/**
 * Curated bundle of editor extensions for the Cypher dialect:
 *   - keyword highlighter + AST-driven semantic decorations,
 *   - autocomplete popup (clause-gated, scope-aware variables,
 *     `namespace.member` + `var.property` completion, snippets),
 *   - signature hints inside function calls,
 *   - hover tooltips (keywords + live variables),
 *   - syntax linter + semantic warnings (undeclared variable, schema
 *     mismatch, unused binding),
 *   - code folding for big clauses / patterns / projections,
 *   - jump-to-declaration via F12 / Cmd-D / ⌘-click,
 *   - bracket matching, auto-close, indent-on-input,
 *   - find / replace, line numbers, active-line, history,
 *   - the standard editing keymap plus indent-with-tab.
 */
export function cypherExtensions(
  options: CypherExtensionsOptions = {},
): Extension {
  const { readOnly = false, showLineNumbers = true, placeholder } = options;
  return [
    loraQueryLanguage,
    syntaxHighlighting(cypherHighlightStyle),
    astDecorations,
    outlineExtension,
    cypherVariableReferences,
    autocompletion({
      override: [cypherCompletions],
      activateOnTyping: true,
      defaultKeymap: true,
    }),
    autoCompletionTriggers,
    signatureHint,
    closeBrackets(),
    bracketMatching(),
    indentOnInput(),
    cypherHover,
    cypherLinter,
    cypherFolding,
    cypherNavigation,
    lintGutter(),
    ...(showLineNumbers ? [lineNumbers()] : []),
    highlightActiveLine(),
    highlightActiveLineGutter(),
    history(),
    search({ top: true }),
    ...(placeholder ? [placeholderExt(placeholder)] : []),
    EditorState.readOnly.of(readOnly),
    keymap.of([
      ...closeBracketsKeymap,
      ...defaultKeymap,
      ...historyKeymap,
      ...completionKeymap,
      ...lintKeymap,
      ...searchKeymap,
      indentWithTab,
    ]),
  ];
}
