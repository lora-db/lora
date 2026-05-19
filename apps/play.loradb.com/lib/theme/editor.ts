/**
 * `LoraQueryEditor` theme deriver. Builds a flat `LoraQueryTheme`
 * directly from our design tokens.
 *
 * Why we don't use `createTheme` from `@loradb/lora-query`:
 * the package's built ESM bundle is wrapped by
 * `vite-plugin-top-level-await` (the parser entry needs WASM TLA),
 * so every export — `createTheme` included — is gated behind an
 * async `__tla` chain that Webpack does not auto-await. Calling
 * `createTheme` synchronously from a `useMemo` during the first
 * render therefore throws `U is not a function` until the WASM
 * promise resolves. The flatten itself is trivial, so we do it
 * inline and skip the TLA-gated import entirely.
 */

import type { LoraJsonTheme, LoraQueryTheme } from "@loradb/lora-query";

import type { Tokens } from "./tokens";
import { hexA } from "./util";

/**
 * Derive a `LoraJsonTheme` from our token set. Mirrors
 * {@link deriveEditorTheme} but uses the JSON-specific token slots
 * (`key`, `string`, `number`, `bool`, `null`, `punct`).
 */
export function deriveJsonEditorTheme(tokens: Tokens): LoraJsonTheme {
  const identifier = tokens.syntax.identifier;
  return {
    background: tokens.bg.editor,
    foreground: tokens.fg.primary,
    border: tokens.border.subtle,
    muted: tokens.fg.muted,
    accent: tokens.accent.primary,
    activeLine: hexA(tokens.fg.primary, 0.06),
    gutterBackground: tokens.bg.editor,
    gutterForeground: tokens.fg.subtle,
    cursor: tokens.fg.primary,
    selectionBackground: hexA(tokens.accent.primary, 0.28),

    fontFamily: tokens.font.ui,
    monoFontFamily: tokens.font.mono,
    fontSize: "13px",
    popupFontSize: "12px",

    key: identifier,
    string: tokens.syntax.string,
    number: tokens.syntax.number,
    bool: tokens.syntax.keyword,
    null: tokens.syntax.keyword,
    // `punct` left to fall through to the shared `--lq-color-punct`
    // default (inherited text colour with 0.7 opacity).

    popupBackground: tokens.bg.panel,
    popupForeground: tokens.fg.primary,
    popupBorder: tokens.border.subtle,
    popupSelectedBackground: tokens.accent.primary,
    popupSelectedForeground: tokens.fg.inverse,
    popupShadow: `0 6px 16px ${hexA("#000000", 0.35)}`,

    errorAccent: tokens.accent.danger,
    warningAccent: tokens.accent.warning,
    infoAccent: tokens.accent.info,

    scrollbarTrack: tokens.bg.editor,
    scrollbarThumb: tokens.border.strong,
    scrollbarThumbHover: tokens.fg.subtle,
    scrollbarWidth: "auto",
    scrollbarSize: "10px",
  };
}

/** Derive a `LoraQueryTheme` from our token set. */
export function deriveEditorTheme(tokens: Tokens): LoraQueryTheme {
  const keyword = tokens.syntax.keyword;
  const identifier = tokens.syntax.identifier;
  const type = tokens.syntax.type;

  return {
    background: tokens.bg.editor,
    foreground: tokens.fg.primary,
    border: tokens.border.subtle,
    muted: tokens.fg.muted,
    accent: tokens.accent.primary,
    activeLine: hexA(tokens.fg.primary, 0.06),
    gutterBackground: tokens.bg.editor,
    gutterForeground: tokens.fg.subtle,
    cursor: tokens.fg.primary,
    selectionBackground: hexA(tokens.accent.primary, 0.28),

    fontFamily: tokens.font.ui,
    monoFontFamily: tokens.font.mono,
    fontSize: "13px",
    popupFontSize: "12px",

    keyword,
    variable: tokens.category.variable,
    parameter: tokens.category.parameter,
    label: tokens.category.label,
    relType: tokens.category.relType,
    property: identifier,
    functionName: type,
    namespace: type,
    string: tokens.syntax.string,
    number: tokens.syntax.number,
    bool: keyword,
    null: keyword,

    popupBackground: tokens.bg.panel,
    popupForeground: tokens.fg.primary,
    popupBorder: tokens.border.subtle,
    popupSelectedBackground: tokens.accent.primary,
    popupSelectedForeground: tokens.fg.inverse,
    popupShadow: `0 6px 16px ${hexA("#000000", 0.35)}`,

    errorAccent: tokens.accent.danger,
    warningAccent: tokens.accent.warning,
    infoAccent: tokens.accent.info,

    scrollbarTrack: tokens.bg.editor,
    scrollbarThumb: tokens.border.strong,
    scrollbarThumbHover: tokens.fg.subtle,
    scrollbarWidth: "auto",
    scrollbarSize: "10px",
  };
}
