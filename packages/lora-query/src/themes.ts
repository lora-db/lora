import type { LoraQueryTheme } from "./LoraQueryEditor";
import {
  githubDark,
  latte,
  typography,
  type Palette,
} from "./palettes";

/**
 * Flatten a {@link Palette} into the flat-key shape the editor's
 * CSS-variable bridge expects. Typography fields fall through from
 * the shared {@link typography} defaults.
 */
function fromPalette(palette: Palette): LoraQueryTheme {
  const { surface, tokens, popup, diagnostic } = palette;
  return {
    ...surface,
    ...typography,
    ...tokens,
    popupBackground: popup.background,
    popupForeground: popup.foreground,
    popupBorder: popup.border,
    popupSelectedBackground: popup.selectedBackground,
    popupSelectedForeground: popup.selectedForeground,
    popupShadow: popup.shadow,
    errorAccent: diagnostic.error,
    warningAccent: diagnostic.warning,
    infoAccent: diagnostic.info,
  };
}

/**
 * Build a {@link LoraQueryTheme} from a base palette plus optional
 * overrides. Use this when you want most of a preset but with a
 * couple of fields swapped — it's strictly equivalent to
 * `{ ...lightTheme, ...overrides }` but typed against the palette.
 *
 * ```tsx
 * const myTheme = createTheme(githubDark, { accent: "#ff6b6b" });
 * <LoraQueryEditor theme={myTheme} ... />
 * ```
 */
export function createTheme(
  palette: Palette,
  overrides: LoraQueryTheme = {},
): LoraQueryTheme {
  return { ...fromPalette(palette), ...overrides };
}

/**
 * Default light theme — Catppuccin Latte. Tracks the CSS defaults in
 * `editor.css` exactly. Override individual fields by spreading:
 *
 * ```tsx
 * <LoraQueryEditor theme={{ ...lightTheme, accent: "#ff6b6b" }} ... />
 * ```
 */
export const lightTheme: LoraQueryTheme = fromPalette(latte);

/**
 * Default dark theme — GitHub Dark token hues on a VS-Code-style
 * `#1e1e1e` surface. Tracks the `prefers-color-scheme: dark` block
 * in `editor.css` exactly.
 *
 * ```tsx
 * <LoraQueryEditor theme={darkTheme} ... />
 * ```
 */
export const darkTheme: LoraQueryTheme = fromPalette(githubDark);
