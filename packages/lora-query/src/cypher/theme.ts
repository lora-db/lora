import { EditorView } from "@codemirror/view";
import type { Extension } from "@codemirror/state";
import { latte, typography } from "../palettes";

/**
 * Subset of the React {@link LoraQueryTheme} keys that need to land
 * inside the CodeMirror-managed style namespace (tooltips, completion
 * popup, lint message). The editor surface itself uses plain CSS
 * variables on `.lora-query`; only the popups need this indirection,
 * because CodeMirror renders them into `document.body` by default and
 * thus can't inherit our container's variables.
 */
export interface PopupThemeValues {
  fontFamily?: string;
  monoFontFamily?: string;
  popupFontSize?: string;
  popupBackground?: string;
  popupForeground?: string;
  popupBorder?: string;
  popupSelectedBackground?: string;
  popupSelectedForeground?: string;
  popupShadow?: string;
  muted?: string;
  errorAccent?: string;
}

// Defaults are derived from the same palette source `editor.css` and
// `themes.lightTheme` mirror, so the three layers cannot drift.
const DEFAULTS: Required<PopupThemeValues> = {
  fontFamily: typography.fontFamily,
  monoFontFamily: typography.monoFontFamily,
  popupFontSize: typography.popupFontSize,
  popupBackground: latte.popup.background,
  popupForeground: latte.popup.foreground,
  popupBorder: latte.popup.border,
  popupSelectedBackground: latte.popup.selectedBackground,
  popupSelectedForeground: latte.popup.selectedForeground,
  popupShadow: latte.popup.shadow,
  muted: latte.surface.muted,
  errorAccent: latte.diagnostic.error,
};

/**
 * Build a CodeMirror {@link EditorView.theme} extension that styles
 * autocomplete / hover / lint popups using values from the React
 * `theme` prop. The styles apply via a generated class on the editor,
 * so popups rendered into `document.body` still pick them up.
 */
export function popupTheme(values: PopupThemeValues): Extension {
  const v = { ...DEFAULTS, ...stripUndefined(values) };
  return EditorView.theme({
    ".cm-tooltip": {
      backgroundColor: v.popupBackground,
      color: v.popupForeground,
      border: `1px solid ${v.popupBorder}`,
      borderRadius: "6px",
      boxShadow: v.popupShadow,
      fontFamily: v.fontFamily,
      fontSize: v.popupFontSize,
    },
    ".cm-tooltip-autocomplete > ul": {
      fontFamily: v.fontFamily,
      fontSize: v.popupFontSize,
    },
    ".cm-tooltip-autocomplete > ul > li": {
      padding: "3px 8px",
    },
    ".cm-tooltip-autocomplete > ul > li[aria-selected='true']": {
      backgroundColor: v.popupSelectedBackground,
      color: v.popupSelectedForeground,
    },
    ".cm-completionDetail": {
      color: v.muted,
      fontStyle: "italic",
    },
    ".cm-completionInfo": {
      backgroundColor: v.popupBackground,
      color: v.popupForeground,
      border: `1px solid ${v.popupBorder}`,
      borderRadius: "6px",
      boxShadow: v.popupShadow,
      padding: "8px 10px",
      maxWidth: "360px",
      fontFamily: v.fontFamily,
      fontSize: v.popupFontSize,
    },
    ".cm-tooltip-lora-query": {
      padding: "8px 10px",
      maxWidth: "360px",
      lineHeight: "1.4",
    },
    ".cm-tooltip-lora-query__title code": {
      fontFamily: v.monoFontFamily,
      background: "rgba(110, 119, 129, 0.12)",
      padding: "1px 4px",
      borderRadius: "3px",
    },
    ".cm-tooltip-lora-query__body": {
      color: v.muted,
    },
    ".cm-lora-diagnostic": {
      fontSize: v.popupFontSize,
      lineHeight: "1.45",
      maxWidth: "460px",
    },
    ".cm-lora-diagnostic__details": {
      marginTop: "6px",
      padding: "6px 8px",
      backgroundColor: tintWithAlpha(v.errorAccent, 0.06),
      borderLeft: `3px solid ${v.errorAccent}`,
      fontFamily: v.monoFontFamily,
      fontSize: "11px",
      whiteSpace: "pre",
      overflowX: "auto",
    },
    ".cm-lora-diagnostic__examples code": {
      fontFamily: v.monoFontFamily,
      background: "rgba(110, 119, 129, 0.12)",
      padding: "1px 4px",
      borderRadius: "3px",
    },
    ".cm-lora-diagnostic__hint": {
      marginTop: "6px",
      color: v.muted,
    },
  });
}

function stripUndefined<T extends object>(obj: T): Partial<T> {
  const out: Partial<T> = {};
  for (const k of Object.keys(obj) as (keyof T)[]) {
    if (obj[k] !== undefined) out[k] = obj[k];
  }
  return out;
}

/** Add an alpha channel to a hex colour. Falls back to the colour itself. */
function tintWithAlpha(color: string, alpha: number): string {
  if (/^#([0-9a-f]{6})$/i.test(color)) {
    const r = parseInt(color.slice(1, 3), 16);
    const g = parseInt(color.slice(3, 5), 16);
    const b = parseInt(color.slice(5, 7), 16);
    return `rgba(${r}, ${g}, ${b}, ${alpha})`;
  }
  return color;
}
