/**
 * Palette helpers — shared between `nodeAutoColorBy` (in
 * `useAccessorOverrides`) and the `GroupLegend` swatches so a given
 * group key always maps to the same colour in the canvas and in the
 * legend.
 *
 * The default ramp is `d3-scale-chromatic`'s `schemeTableau10`. Hosts
 * can override the palette via `LoraGraphTheme.nodePalette` and the
 * hash-to-index function picks colours deterministically — same string
 * → same colour across re-renders, sessions, and consumers.
 */

/** Default node palette (Tableau10). Kept in the same order as
 *  d3-scale-chromatic so any consumer that already uses Tableau10
 *  matches without configuration. */
export const DEFAULT_NODE_PALETTE: readonly string[] = [
  "#4e79a7",
  "#f28e2b",
  "#e15759",
  "#76b7b2",
  "#59a14f",
  "#edc948",
  "#b07aa1",
  "#ff9da7",
  "#9c755f",
  "#bab0ac",
];

/** Default relationship colour when nothing is selected or hovered.
 *  Stays in lock-step with `useAccessorOverrides`'s baseline so the
 *  in-engine fallback and the selection wrapper agree. */
export const DEFAULT_LINK_COLOR = "rgba(96, 102, 110, 0.55)";

/** Default colour for hovered relationships. Same alpha as
 *  `DEFAULT_LINK_COLOR` so a hover transition only mutates RGB and
 *  doesn't drag the link across Three.js's transparent/opaque sort
 *  groups (which would visibly reshuffle neighbouring lines). */
export const DEFAULT_LINK_HOVER_COLOR = "rgba(180, 188, 198, 0.55)";

/** Stable string → index hash. Same group string always yields the
 *  same palette slot regardless of insertion order or session. */
function hashStringToIndex(input: string, mod: number): number {
  if (mod <= 0) return 0;
  let h = 0;
  for (let i = 0; i < input.length; i++) {
    h = (h * 31 + input.charCodeAt(i)) | 0;
  }
  return Math.abs(h) % mod;
}

/** Pick the palette colour for a group key. */
export function colorForGroup(
  group: string,
  palette: readonly string[] = DEFAULT_NODE_PALETTE,
): string {
  if (palette.length === 0) return DEFAULT_NODE_PALETTE[0]!;
  return palette[hashStringToIndex(group, palette.length)]!;
}
