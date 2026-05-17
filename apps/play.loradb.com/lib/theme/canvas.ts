/**
 * `LoraGraphCanvas` theme deriver. Maps our design tokens onto the
 * package's CSS-variable theme shape so the canvas chrome (toolbar,
 * tooltips, context menu) sits flush with the rest of the playground.
 */

import type { LoraGraphTheme } from "@loradb/lora-graph-canvas";

import type { Tokens } from "./tokens";
import { hexA } from "./util";

/** Build a `LoraGraphTheme` from a token set. */
export function deriveCanvasTheme(tokens: Tokens): LoraGraphTheme {
  return {
    background: tokens.bg.editor,
    foreground: tokens.fg.primary,
    border: tokens.border.subtle,
    accent: tokens.accent.primary,
    toolbarBackground: hexA(tokens.bg.panel, 0.92),
    toolbarForeground: tokens.fg.primary,
    toolbarBorder: tokens.border.subtle,
    toolActiveBackground: hexA(tokens.accent.primary, 0.25),
    toolHoverBackground: hexA(tokens.fg.primary, 0.08),
    tooltipBackground: hexA(tokens.fg.primary, 0.92),
    tooltipForeground: tokens.bg.editor,
    menuBackground: tokens.bg.panel,
    menuForeground: tokens.fg.primary,
    menuHoverBackground: hexA(tokens.fg.primary, 0.08),
    fontFamily: tokens.font.ui,
  };
}
