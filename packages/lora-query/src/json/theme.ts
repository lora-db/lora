import { EditorView } from "@codemirror/view";
import type { Extension } from "@codemirror/state";
import { latte, typography } from "../palettes";

/**
 * Subset of the React {@link LoraJsonTheme} keys that need to land
 * inside the CodeMirror-managed style namespace (autocomplete popup,
 * hover tooltip, lint message). The editor surface itself uses plain
 * CSS variables on `.lora-json`; only the popups need this indirection,
 * because CodeMirror renders them into `document.body` by default and
 * thus can't inherit our container's variables.
 *
 * Mirrors `cypher/theme.ts::PopupThemeValues` so the two editors share
 * tooltip styling rules without duplicating the source-of-truth list.
 */
export interface JsonPopupThemeValues {
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

const DEFAULTS: Required<JsonPopupThemeValues> = {
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
 * CodeMirror theme extension that styles autocomplete / hover / lint
 * popups using values from the React `theme` prop. The styles attach
 * via a generated class on the editor, so popups rendered into
 * `document.body` still pick them up.
 */
export function jsonPopupTheme(values: JsonPopupThemeValues): Extension {
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
    ".cm-diagnostic": {
      fontSize: v.popupFontSize,
      lineHeight: "1.45",
      maxWidth: "460px",
    },
    ".cm-diagnostic-error": {
      borderLeft: `3px solid ${v.errorAccent}`,
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
