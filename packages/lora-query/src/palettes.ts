/**
 * Palette constants — single source of truth for every default
 * colour the editor ships with. Both the React theme presets
 * (`themes.ts`) and the popup-theme defaults (`cypher/theme.ts`)
 * read from here, and `editor.css` mirrors the same values so the
 * three layers cannot drift.
 *
 * A palette is a flat record of CSS-ready strings. Adding a palette
 * is intentionally a one-line affair: define the surface + token
 * hues, then call `createTheme({ palette })` in `themes.ts` to wrap
 * it in a full `LoraQueryTheme`.
 */

export interface SurfaceColors {
  background: string;
  foreground: string;
  border: string;
  muted: string;
  accent: string;
  activeLine: string;
  gutterBackground: string;
  gutterForeground: string;
  cursor: string;
  /** rgba() — must already include alpha for the selection highlight. */
  selectionBackground: string;
}

export interface TokenColors {
  keyword: string;
  variable: string;
  parameter: string;
  label: string;
  relType: string;
  property: string;
  functionName: string;
  namespace: string;
  string: string;
  number: string;
  bool: string;
  null: string;
  operator: string;
  comment: string;
}

export interface PopupColors {
  background: string;
  foreground: string;
  border: string;
  selectedBackground: string;
  selectedForeground: string;
  shadow: string;
}

export interface DiagnosticColors {
  error: string;
  warning: string;
  info: string;
}

export interface ScrollbarColors {
  /** Background of the scrollbar gutter (track). */
  track: string;
  /** Resting colour of the draggable thumb. */
  thumb: string;
  /** Thumb colour on hover. */
  thumbHover: string;
  /**
   * CSS `scrollbar-width` value — `"auto"`, `"thin"`, or `"none"`.
   * Honoured by Firefox + modern WebKit; native browsers fall back to
   * their default sizing.
   */
  width: "auto" | "thin" | "none";
  /**
   * Pixel size used by the `::-webkit-scrollbar` rules — gives the
   * thumb/track a consistent thickness on Chrome and Safari where
   * `scrollbar-width: thin` is partially honoured.
   */
  size: string;
}

export interface Palette {
  surface: SurfaceColors;
  tokens: TokenColors;
  popup: PopupColors;
  diagnostic: DiagnosticColors;
  scrollbar: ScrollbarColors;
}

/** Catppuccin Latte — the default light palette. */
export const latte: Palette = {
  surface: {
    background: "#eff1f5",
    foreground: "#4c4f69",
    border: "#ccd0da",
    muted: "#6c6f85",
    accent: "#1e66f5",
    activeLine: "#e6e9ef",
    gutterBackground: "#e6e9ef",
    gutterForeground: "#8c8fa1",
    cursor: "#4c4f69",
    selectionBackground: "rgba(30, 102, 245, 0.18)",
  },
  tokens: {
    keyword: "#d20f39",
    variable: "#1e66f5",
    parameter: "#8839ef",
    label: "#40a02b",
    relType: "#df8e1d",
    property: "#ea76cb",
    functionName: "#fe640b",
    namespace: "#fe640b",
    string: "#179299",
    number: "#8839ef",
    bool: "#d20f39",
    null: "#d20f39",
    operator: "#6c6f85",
    comment: "#8c8fa1",
  },
  popup: {
    background: "#eff1f5",
    foreground: "#4c4f69",
    border: "#ccd0da",
    selectedBackground: "#1e66f5",
    selectedForeground: "#eff1f5",
    shadow: "0 4px 12px rgba(76, 79, 105, 0.15)",
  },
  diagnostic: {
    error: "#d20f39",
    warning: "#df8e1d",
    info: "#1e66f5",
  },
  scrollbar: {
    track: "#e6e9ef",
    thumb: "#ccd0da",
    thumbHover: "#9ca0b0",
    width: "auto",
    size: "10px",
  },
};

/**
 * "GitHub Dark on VS-Code surface" — the default dark palette. The
 * surface follows VS Code's `#1e1e1e` neutral so the editor sits
 * comfortably next to other VS-Code-ish panels; token hues follow
 * GitHub Dark for the recognisable coral/sky/lilac/mint mapping.
 */
export const githubDark: Palette = {
  surface: {
    background: "#1e1e1e",
    foreground: "#e6e6e6",
    border: "#3a3a3a",
    muted: "#9d9d9d",
    accent: "#1f6feb",
    activeLine: "#2a2a2a",
    gutterBackground: "#1e1e1e",
    gutterForeground: "#6e7681",
    cursor: "#e6e6e6",
    selectionBackground: "rgba(31, 111, 235, 0.28)",
  },
  tokens: {
    keyword: "#ff7b72",
    variable: "#79c0ff",
    parameter: "#d2a8ff",
    label: "#7ee787",
    relType: "#ffa657",
    property: "#ffa657",
    functionName: "#d2a8ff",
    namespace: "#ffa657",
    string: "#a5d6ff",
    number: "#79c0ff",
    bool: "#ff7b72",
    null: "#ff7b72",
    operator: "#9d9d9d",
    comment: "#7d8590",
  },
  popup: {
    background: "#161b22",
    foreground: "#e6e6e6",
    border: "#30363d",
    selectedBackground: "#1f6feb",
    selectedForeground: "#ffffff",
    shadow: "0 6px 16px rgba(0, 0, 0, 0.5)",
  },
  diagnostic: {
    error: "#f85149",
    warning: "#d29922",
    info: "#58a6ff",
  },
  scrollbar: {
    track: "#1e1e1e",
    thumb: "#3a3a3a",
    thumbHover: "#5a5a5a",
    width: "auto",
    size: "10px",
  },
};

/** Typography defaults — shared across every palette. */
export const typography = {
  fontFamily:
    'ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif',
  monoFontFamily:
    'ui-monospace, SFMono-Regular, "JetBrains Mono", Menlo, monospace',
  fontSize: "13px",
  popupFontSize: "12px",
} as const;
