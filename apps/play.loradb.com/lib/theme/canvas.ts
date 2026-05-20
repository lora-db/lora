/**
 * `LoraGraphCanvas` theme deriver. Maps our design tokens onto the
 * package's CSS-variable theme shape so the canvas chrome (toolbar,
 * tooltips, context menu) plus its node / relationship coloring sit
 * flush with the rest of the playground.
 *
 * The node palette is split across two theme tuples (light / dark) so
 * label colours stay readable against either editor background.
 * `colorForGroup` in the package hashes a node's `group` key against
 * this array, so the same `:Label` always lands on the same swatch.
 */

import type { LoraGraphTheme } from "@loradb/lora-graph-canvas";

import type { Tokens } from "./tokens";
import { hexA } from "./util";

/** Hand-tuned playground node palette. Mirrors the editor's category
 *  hues (variable, label, relType, parameter) so a `:Person` node on
 *  the canvas reads the same hue as the `:Person` chip in the schema
 *  browser, then fans out into supporting swatches that still read on
 *  the editor background. */
const LIGHT_NODE_PALETTE: readonly string[] = [
  "#0969da", // var / node primary blue
  "#40a02b", // label green
  "#df8e1d", // rel-type amber
  "#8839ef", // parameter violet
  "#1f883d", // success green
  "#cf222e", // danger red
  "#0a7ea4", // cyan
  "#bf8700", // warning ochre
  "#9333ea", // magenta-violet
  "#475569", // slate
];

const DARK_NODE_PALETTE: readonly string[] = [
  "#6aa3ff", // var / node primary blue
  "#7ee787", // label green
  "#ffa657", // rel-type amber
  "#d2a8ff", // parameter violet
  "#4ec9b0", // success teal
  "#f48771", // danger coral
  "#79c0ff", // cyan
  "#dcdcaa", // warning ochre
  "#c586c0", // magenta-violet
  "#9ca3af", // slate
];

function pickNodePalette(tokens: Tokens): readonly string[] {
  return tokens.bg.app.toLowerCase() === "#ffffff"
    ? LIGHT_NODE_PALETTE
    : DARK_NODE_PALETTE;
}

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
    // ── Node / relationship palette ──────────────────────────────
    // Nodes are coloured by `:Label` (the adapter sets `group =
    // primaryLabel(n)`) via the package's `nodeAutoColorBy="group"`,
    // hashed against this palette. Link colours use the playground's
    // own graph tokens with matched alpha — alpha equality is load-
    // bearing for stable three.js material sort order; see the
    // comment in `useAccessorOverrides.ts`.
    nodePalette: pickNodePalette(tokens),
    linkDefault: hexA(tokens.graph.link, 0.55),
    // `linkHover` deliberately falls back to the package default
    // (a brighter neutral grey). The playground's `graph.linkHighlight`
    // token is an accent blue, which would conflate hover with selection
    // in the canvas's three-tier emphasis model — hover stays neutral,
    // selection is the only thing that uses the accent.
  };
}
