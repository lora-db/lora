"use client";

/**
 * Human-readable rendering of the shortcut strings used by
 * `@mantine/hooks#useHotkeys`. The map in {@link bindings.ts} stores
 * the raw form (e.g. `"mod+shift+E"`); this module formats it for
 * tooltips, the spotlight palette, and the `?` help dialog.
 *
 * Each entry carries a description so the help dialog can show a
 * meaningful label next to the chord without us having to maintain a
 * second mapping.
 */

/** Action keys — anything in the app that has a discoverable chord. */
export type HotkeyId =
  | "run"
  | "newTab"
  | "closeTab"
  | "toggleSidebar"
  | "resultGraph"
  | "resultTable"
  | "resultJson"
  | "formatQuery"
  | "activityQueries"
  | "activitySchema"
  | "activitySnapshots"
  | "activityHistory"
  | "activitySettings"
  | "prevTab"
  | "nextTab"
  | "moveTabLeft"
  | "moveTabRight"
  | "focusEditor"
  | "toggleColorScheme"
  | "spotlight"
  | "help"
  | "splitRight"
  | "splitDown"
  | "closePane"
  | "focusNextPane"
  | "focusPrevPane"
  | "cycleViewInPane"
  | "toggleOrientation";

export interface HotkeyMeta {
  /** Raw shortcut as recognised by Mantine (e.g. "mod+shift+E"). */
  chord: string;
  /** Short label for the help dialog row. */
  description: string;
  /**
   * Bucket the action belongs to in the help dialog. Lets us group
   * the table by intent (Run, Tabs, Navigation, View, …) without
   * sprinkling section markers across the binding code.
   */
  group:
    | "Run"
    | "Tabs"
    | "Navigation"
    | "View"
    | "Editor"
    | "Panes"
    | "App";
}

export const HOTKEYS: Record<HotkeyId, HotkeyMeta> = {
  run: { chord: "mod+Enter", description: "Run query", group: "Run" },
  newTab: { chord: "mod+N", description: "New query tab", group: "Tabs" },
  closeTab: { chord: "mod+W", description: "Close active tab", group: "Tabs" },
  toggleSidebar: { chord: "mod+B", description: "Toggle sidebar", group: "View" },
  resultGraph: { chord: "mod+1", description: "Show graph result", group: "View" },
  resultTable: { chord: "mod+2", description: "Show table result", group: "View" },
  resultJson: { chord: "mod+3", description: "Show JSON result", group: "View" },
  formatQuery: { chord: "shift+alt+F", description: "Format query", group: "Editor" },
  activityQueries: { chord: "mod+shift+E", description: "Saved queries panel", group: "Navigation" },
  activitySchema: { chord: "mod+shift+S", description: "Schema panel", group: "Navigation" },
  activitySnapshots: { chord: "mod+shift+N", description: "Snapshots panel", group: "Navigation" },
  activityHistory: { chord: "mod+shift+H", description: "History panel", group: "Navigation" },
  activitySettings: { chord: "mod+comma", description: "Settings panel", group: "Navigation" },
  prevTab: { chord: "alt+ArrowLeft", description: "Previous tab", group: "Tabs" },
  nextTab: { chord: "alt+ArrowRight", description: "Next tab", group: "Tabs" },
  moveTabLeft: { chord: "mod+shift+alt+ArrowLeft", description: "Move tab left", group: "Tabs" },
  moveTabRight: { chord: "mod+shift+alt+ArrowRight", description: "Move tab right", group: "Tabs" },
  focusEditor: { chord: "mod+shift+P", description: "Focus editor", group: "Editor" },
  toggleColorScheme: { chord: "mod+shift+D", description: "Toggle color scheme", group: "App" },
  spotlight: { chord: "mod+K", description: "Open command palette", group: "App" },
  help: { chord: "shift+/", description: "Show keyboard shortcuts", group: "App" },
  splitRight: { chord: "mod+shift+ArrowRight", description: "Split pane right", group: "Panes" },
  splitDown: { chord: "mod+shift+ArrowDown", description: "Split pane down", group: "Panes" },
  closePane: { chord: "mod+alt+W", description: "Close active pane", group: "Panes" },
  focusNextPane: { chord: "mod+alt+ArrowRight", description: "Focus next pane", group: "Panes" },
  focusPrevPane: { chord: "mod+alt+ArrowLeft", description: "Focus previous pane", group: "Panes" },
  cycleViewInPane: { chord: "mod+`", description: "Cycle views in current pane", group: "Panes" },
  toggleOrientation: { chord: "mod+alt+O", description: "Toggle root split orientation", group: "Panes" },
};

const ARROW_SYMBOLS: Record<string, string> = {
  arrowleft: "←",
  arrowright: "→",
  arrowup: "↑",
  arrowdown: "↓",
};

/**
 * Decide whether we're running on macOS. SSR-safe — falls back to
 * "false" on the server so the first paint is consistent and the
 * hotkey label hydrates to the correct platform on mount.
 */
function isMac(): boolean {
  if (typeof navigator === "undefined") return false;
  const platform =
    (navigator as Navigator & { userAgentData?: { platform?: string } })
      .userAgentData?.platform ?? navigator.platform;
  return /Mac|iPad|iPhone|iPod/i.test(platform ?? "");
}

function formatPart(part: string, mac: boolean): string {
  const p = part.trim().toLowerCase();
  if (p.length === 0) return "";
  switch (p) {
    case "mod":
      return mac ? "⌘" : "Ctrl";
    case "ctrl":
    case "control":
      return mac ? "⌃" : "Ctrl";
    case "alt":
    case "option":
      return mac ? "⌥" : "Alt";
    case "shift":
      return mac ? "⇧" : "Shift";
    case "meta":
    case "cmd":
    case "command":
      return mac ? "⌘" : "Win";
    case "enter":
    case "return":
      return mac ? "↵" : "Enter";
    case "escape":
    case "esc":
      return "Esc";
    case "space":
      return "Space";
    case "comma":
      return ",";
    case "period":
    case "dot":
      return ".";
    case "slash":
      return "/";
    case "tab":
      return mac ? "⇥" : "Tab";
    case "backspace":
      return mac ? "⌫" : "Backspace";
    case "delete":
      return mac ? "⌦" : "Del";
    default:
      if (ARROW_SYMBOLS[p]) return ARROW_SYMBOLS[p];
      // Single character keys render upper-cased; multi-char keys
      // fall through unchanged (e.g. "F5", "PageUp").
      if (p.length === 1) return p.toUpperCase();
      return part.replace(/^[a-z]/, (c) => c.toUpperCase());
  }
}

/**
 * Format a Mantine shortcut string for display. The "+" separator is
 * elided on Mac (chords run together like `⌘⇧E`) and preserved on
 * other platforms (`Ctrl+Shift+E`) so each side feels native.
 */
export function formatChord(chord: string): string {
  const mac = isMac();
  const parts = chord.split("+").map((p) => formatPart(p, mac));
  return mac ? parts.join("") : parts.join("+");
}

/** Convenience — formatted chord by id. */
export function chordFor(id: HotkeyId): string {
  return formatChord(HOTKEYS[id].chord);
}
