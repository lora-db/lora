/**
 * `LoraQueryEditor` theme deriver. Builds a `Palette` from our
 * design tokens and runs it through the package's `createTheme`
 * helper so the editor surface, syntax tokens, popups, and diagnostic
 * accents all snap to the playground palette.
 *
 * Mirrors the dark/light preset construction in
 * `@loradb/lora-query/themes.ts`: `createTheme(palette, overrides)`
 * spreads typography defaults from `palettes.ts` and flattens the
 * popup / diagnostic groups onto the flat `LoraQueryTheme` shape.
 */

import {
  createTheme,
  type LoraQueryTheme,
  type Palette,
} from "@loradb/lora-query";

import type { Tokens } from "./tokens";
import { hexA } from "./util";

/** Derive a `LoraQueryTheme` from our token set. */
export function deriveEditorTheme(tokens: Tokens): LoraQueryTheme {
  const palette: Palette = {
    surface: {
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
    },
    tokens: {
      keyword: tokens.syntax.keyword,
      variable: tokens.syntax.identifier,
      parameter: tokens.syntax.identifier,
      label: tokens.syntax.type,
      relType: tokens.syntax.type,
      property: tokens.syntax.identifier,
      functionName: tokens.syntax.type,
      namespace: tokens.syntax.type,
      string: tokens.syntax.string,
      number: tokens.syntax.number,
      bool: tokens.syntax.keyword,
      null: tokens.syntax.keyword,
      operator: tokens.syntax.operator,
      comment: tokens.syntax.comment,
    },
    popup: {
      background: tokens.bg.panel,
      foreground: tokens.fg.primary,
      border: tokens.border.subtle,
      selectedBackground: tokens.accent.primary,
      selectedForeground: tokens.fg.inverse,
      shadow: `0 6px 16px ${hexA("#000000", 0.35)}`,
    },
    diagnostic: {
      error: tokens.accent.danger,
      warning: tokens.accent.warning,
      info: tokens.accent.info,
    },
    scrollbar: {
      track: tokens.bg.editor,
      thumb: tokens.border.strong,
      thumbHover: tokens.fg.subtle,
      width: "auto",
      size: "10px",
    },
  };

  return createTheme(palette, {
    fontFamily: tokens.font.ui,
    monoFontFamily: tokens.font.mono,
    fontSize: "13px",
    popupFontSize: "12px",
  });
}
