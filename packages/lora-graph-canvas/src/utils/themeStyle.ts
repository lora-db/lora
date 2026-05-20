import type { CSSProperties } from "react";
import type { LoraGraphTheme } from "../types";

// Only the chrome-facing theme keys map to CSS variables; the engine-
// facing keys (`nodePalette`, `linkDefault`, `linkHover`) are consumed
// in JS by the accessor wrappers and don't need a `--lgc-*` slot.
const THEME_TO_VAR: Partial<Record<keyof LoraGraphTheme, string>> = {
  background: "--lgc-bg",
  foreground: "--lgc-fg",
  border: "--lgc-border",
  accent: "--lgc-accent",
  toolbarBackground: "--lgc-toolbar-bg",
  toolbarForeground: "--lgc-toolbar-fg",
  toolbarBorder: "--lgc-toolbar-border",
  toolActiveBackground: "--lgc-tool-active-bg",
  toolHoverBackground: "--lgc-tool-hover-bg",
  tooltipBackground: "--lgc-tooltip-bg",
  tooltipForeground: "--lgc-tooltip-fg",
  menuBackground: "--lgc-menu-bg",
  menuForeground: "--lgc-menu-fg",
  menuHoverBackground: "--lgc-menu-hover-bg",
  fontFamily: "--lgc-font",
  fontSize: "--lgc-font-size",
};

/** Translate a partial `LoraGraphTheme` into a style object that sets
 *  the matching CSS custom properties on the host element. */
export function themeToStyle(theme?: Partial<LoraGraphTheme>): CSSProperties {
  if (!theme) return {};
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(theme)) {
    if (value === undefined) continue;
    const cssVar = THEME_TO_VAR[key as keyof LoraGraphTheme];
    if (cssVar) out[cssVar] = String(value);
  }
  return out as CSSProperties;
}
