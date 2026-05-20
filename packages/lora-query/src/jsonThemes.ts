import { githubDark, latte, typography, type Palette } from "./palettes";

/**
 * CSS-variable theme for the JSON editor. Identical shape to
 * {@link import("./LoraQueryEditor").LoraQueryTheme} for the surface,
 * popup, scrollbar, and typography slots — the JSON editor reuses the
 * exact same `--lq-*` variables so a single CSS source of truth
 * styles both editors.
 *
 * The token names below name the JSON role each colour fills:
 *   - `key`     — object property keys
 *   - `string`  — string values
 *   - `number`  — numeric values
 *   - `bool`    — `true` / `false`
 *   - `null`    — `null`
 *   - `punct`   — braces, brackets, colons, commas
 */
export interface LoraJsonTheme {
  // Editor surface
  background?: string;
  foreground?: string;
  border?: string;
  accent?: string;
  muted?: string;
  activeLine?: string;
  gutterBackground?: string;
  gutterForeground?: string;
  cursor?: string;
  /** Width of the blinking caret (e.g. `"1px"`, `"2px"`). */
  cursorWidth?: string;
  selectionBackground?: string;

  // Typography
  fontFamily?: string;
  monoFontFamily?: string;
  fontSize?: string;
  popupFontSize?: string;

  // JSON tokens
  key?: string;
  string?: string;
  number?: string;
  bool?: string;
  null?: string;
  punct?: string;

  // Popups
  popupBackground?: string;
  popupForeground?: string;
  popupBorder?: string;
  popupSelectedBackground?: string;
  popupSelectedForeground?: string;
  popupShadow?: string;

  // Diagnostic accent (used by the parse-error lint message)
  errorAccent?: string;
  warningAccent?: string;
  infoAccent?: string;

  // Scrollbar
  scrollbarTrack?: string;
  scrollbarThumb?: string;
  scrollbarThumbHover?: string;
  scrollbarWidth?: "auto" | "thin" | "none";
  scrollbarSize?: string;
}

/**
 * Map every `LoraJsonTheme` field to the CSS variable it overrides.
 * The variables match the Cypher editor's namespace exactly — the
 * shared `editor.css` already drives `.cm-lora-property`,
 * `.cm-lora-string`, etc. from those vars.
 */
export const JSON_THEME_TO_VAR: Record<keyof LoraJsonTheme, string> = {
  background: "--lq-bg",
  foreground: "--lq-fg",
  border: "--lq-border",
  accent: "--lq-accent",
  muted: "--lq-muted",
  activeLine: "--lq-active-line",
  gutterBackground: "--lq-gutter-bg",
  gutterForeground: "--lq-gutter-fg",
  cursor: "--lq-cursor",
  cursorWidth: "--lq-cursor-width",
  selectionBackground: "--lq-selection-bg",

  fontFamily: "--lq-font",
  monoFontFamily: "--lq-mono-font",
  fontSize: "--lq-font-size",
  popupFontSize: "--lq-popup-font-size",

  key: "--lq-color-property",
  string: "--lq-color-string",
  number: "--lq-color-number",
  bool: "--lq-color-bool",
  null: "--lq-color-null",
  punct: "--lq-color-punct",

  popupBackground: "--lq-popup-bg",
  popupForeground: "--lq-popup-fg",
  popupBorder: "--lq-popup-border",
  popupSelectedBackground: "--lq-popup-selected-bg",
  popupSelectedForeground: "--lq-popup-selected-fg",
  popupShadow: "--lq-popup-shadow",

  errorAccent: "--lq-error",
  warningAccent: "--lq-warning",
  infoAccent: "--lq-info",

  scrollbarTrack: "--lq-scrollbar-track",
  scrollbarThumb: "--lq-scrollbar-thumb",
  scrollbarThumbHover: "--lq-scrollbar-thumb-hover",
  scrollbarWidth: "--lq-scrollbar-width",
  scrollbarSize: "--lq-scrollbar-size",
};

/**
 * Flatten a {@link Palette} into the flat-key shape the editor's
 * CSS-variable bridge expects. Typography fields fall through from
 * the shared {@link typography} defaults.
 */
function fromPalette(palette: Palette): LoraJsonTheme {
  const { surface, tokens, popup, diagnostic, scrollbar } = palette;
  return {
    ...surface,
    ...typography,
    key: tokens.property,
    string: tokens.string,
    number: tokens.number,
    bool: tokens.bool,
    null: tokens.null,
    // No dedicated punctuation hue in the palette — let CSS fall
    // through to the existing `--lq-color-punct` (inherits text
    // colour with `opacity: 0.7` for a quieter look).
    popupBackground: popup.background,
    popupForeground: popup.foreground,
    popupBorder: popup.border,
    popupSelectedBackground: popup.selectedBackground,
    popupSelectedForeground: popup.selectedForeground,
    popupShadow: popup.shadow,
    errorAccent: diagnostic.error,
    warningAccent: diagnostic.warning,
    infoAccent: diagnostic.info,
    scrollbarTrack: scrollbar.track,
    scrollbarThumb: scrollbar.thumb,
    scrollbarThumbHover: scrollbar.thumbHover,
    scrollbarWidth: scrollbar.width,
    scrollbarSize: scrollbar.size,
  };
}

/**
 * Build a {@link LoraJsonTheme} from a base palette plus optional
 * overrides. Mirrors `createTheme` from `themes.ts` for the Cypher
 * editor — both consume the same `Palette` shape.
 *
 * ```tsx
 * const myTheme = createJsonTheme(githubDark, { accent: "#ff6b6b" });
 * <LoraJsonEditor theme={myTheme} ... />
 * ```
 */
export function createJsonTheme(
  palette: Palette,
  overrides: LoraJsonTheme = {},
): LoraJsonTheme {
  return { ...fromPalette(palette), ...overrides };
}

/** Default light JSON theme — Catppuccin Latte. */
export const lightJsonTheme: LoraJsonTheme = fromPalette(latte);

/** Default dark JSON theme — GitHub Dark on VS-Code-style surface. */
export const darkJsonTheme: LoraJsonTheme = fromPalette(githubDark);
