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
import { json, jsonParseLinter } from "@codemirror/lang-json";
import {
  bracketMatching,
  codeFolding,
  foldGutter,
  foldKeymap,
  indentOnInput,
} from "@codemirror/language";
import { lintGutter, lintKeymap, linter } from "@codemirror/lint";
import { search, searchKeymap } from "@codemirror/search";
import { EditorState, Prec, type Extension } from "@codemirror/state";
import {
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  placeholder as placeholderExt,
} from "@codemirror/view";
import { jsonHighlight } from "./highlight";
import { jsonCompletions } from "./completion";
import { jsonSmartEnter } from "./smartEnter";
import { keyConstraintsLinter } from "./keyConstraints";
import {
  jsonFoldPlaceholderDOM,
  jsonFoldPreparePlaceholder,
} from "./foldPlaceholder";

export interface JsonExtensionsOptions {
  readOnly?: boolean;
  showLineNumbers?: boolean;
  placeholder?: string;
  enableLinter?: boolean;
  /**
   * Enable the smart-Enter keymap (newline-on-comma, split-on-brace).
   * Defaults to `true` when the editor is editable, `false` when
   * read-only (Enter on a read-only buffer is a no-op anyway).
   */
  smartEnter?: boolean;
}

/**
 * Curated extension bundle for the JSON editor:
 *   - `@codemirror/lang-json` + our `cm-lora-*` highlight style,
 *   - configurable JSON parse linter + custom key-constraint linter
 *     (driven by the `keyConstraintsFacet`),
 *   - schema-driven key autocomplete (via `loraJsonProviders`),
 *   - code folding with rich placeholders showing item counts,
 *   - smart Enter keymap (split on brace, key-skeleton on comma),
 *   - bracket matching, auto-close, indent-on-input,
 *   - find/replace, line numbers, active-line highlighting, history.
 *
 * Mirrors `cypher/extensions.ts` so both editors share the same
 * structural UX.
 */
export function jsonExtensions(
  options: JsonExtensionsOptions = {},
): Extension {
  const {
    readOnly = false,
    showLineNumbers = true,
    placeholder,
    enableLinter = !readOnly,
    smartEnter = !readOnly,
  } = options;

  return [
    json(),
    jsonHighlight,
    codeFolding({
      placeholderDOM: jsonFoldPlaceholderDOM,
      preparePlaceholder: jsonFoldPreparePlaceholder,
    }),
    autocompletion({
      override: [jsonCompletions],
      activateOnTyping: true,
      defaultKeymap: true,
    }),
    closeBrackets(),
    bracketMatching(),
    indentOnInput(),
    foldGutter(),
    ...(enableLinter
      ? [linter(jsonParseLinter()), keyConstraintsLinter, lintGutter()]
      : []),
    ...(showLineNumbers ? [lineNumbers()] : []),
    highlightActiveLine(),
    highlightActiveLineGutter(),
    history(),
    search({ top: true }),
    ...(placeholder ? [placeholderExt(placeholder)] : []),
    EditorState.readOnly.of(readOnly),
    ...(smartEnter ? [Prec.high(keymap.of([jsonSmartEnter]))] : []),
    keymap.of([
      ...closeBracketsKeymap,
      ...defaultKeymap,
      ...historyKeymap,
      ...completionKeymap,
      ...lintKeymap,
      ...searchKeymap,
      ...foldKeymap,
      indentWithTab,
    ]),
  ];
}
