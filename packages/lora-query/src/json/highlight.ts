import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import type { Extension } from "@codemirror/state";

/**
 * Lezer-tag → `.cm-lora-*` class mapping for the JSON grammar shipped
 * by `@codemirror/lang-json`. The class names match the ones the
 * Cypher editor already uses, so a single `editor.css` palette colours
 * both editors with no extra rules.
 *
 * Tag coverage for JSON:
 *   - `propertyName` → object keys
 *   - `string`       → string values
 *   - `number`       → numeric values
 *   - `bool`         → `true` / `false`
 *   - `null`         → `null`
 *   - `punctuation`  → braces, brackets, colons, commas
 *   - `separator`    → commas (Lezer-JSON emits this tag for `,`)
 */
const jsonHighlightStyle = HighlightStyle.define([
  { tag: t.propertyName, class: "cm-lora-property" },
  { tag: t.string, class: "cm-lora-string" },
  { tag: t.number, class: "cm-lora-number" },
  { tag: t.bool, class: "cm-lora-bool" },
  { tag: t.null, class: "cm-lora-null" },
  { tag: [t.punctuation, t.separator, t.brace, t.bracket], class: "cm-lora-punct" },
  { tag: t.invalid, class: "cm-lora-invalid" },
]);

/**
 * CodeMirror extension that hooks the JSON highlight style into the
 * syntax-highlighting service. Apply once when bundling the editor's
 * extensions.
 */
export const jsonHighlight: Extension = syntaxHighlighting(jsonHighlightStyle);
