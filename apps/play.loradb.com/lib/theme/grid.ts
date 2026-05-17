/**
 * Glide Data Grid theme deriver. Returns a `Partial<GlideTheme>` so
 * the host can spread it onto whatever defaults `DataEditor` ships
 * with — we only override the keys we actually care about and let
 * Glide fill the rest.
 */

import type { Theme as GlideTheme } from "@glideapps/glide-data-grid";

import type { Tokens } from "./tokens";
import { hexA } from "./util";

/** Derive a Glide Data Grid theme override from a token set. */
export function deriveGridTheme(tokens: Tokens): Partial<GlideTheme> {
  return {
    accentColor: tokens.accent.primary,
    accentLight: hexA(tokens.accent.primary, 0.18),
    accentFg: tokens.fg.inverse,

    textDark: tokens.fg.primary,
    textMedium: tokens.fg.muted,
    textLight: tokens.fg.subtle,
    textBubble: tokens.fg.primary,
    textHeader: tokens.fg.primary,
    textHeaderSelected: tokens.fg.inverse,
    textGroupHeader: tokens.fg.muted,

    bgCell: tokens.bg.editor,
    bgCellMedium: tokens.bg.panel,
    bgHeader: tokens.bg.panel,
    bgHeaderHasFocus: tokens.bg.sidebar,
    bgHeaderHovered: tokens.bg.overlay,
    bgBubble: tokens.bg.panel,
    bgBubbleSelected: tokens.accent.primary,
    bgSearchResult: hexA(tokens.accent.warning, 0.35),

    borderColor: tokens.border.subtle,
    drilldownBorder: tokens.border.strong,
    horizontalBorderColor: tokens.border.subtle,

    cellHorizontalPadding: 8,
    cellVerticalPadding: 4,

    headerFontStyle: "600 13px",
    baseFontStyle: "13px",
    fontFamily: tokens.font.ui,
    editorFontSize: "13px",
    lineHeight: 1.4,
  };
}
