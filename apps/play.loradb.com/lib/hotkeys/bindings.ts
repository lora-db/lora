"use client";

/**
 * Centralised hotkey map. The map is built from a factory so it can
 * close over the per-render Mantine color-scheme API. Each entry is a
 * tuple shaped for `@mantine/hooks#useHotkeys`.
 */

import type { MantineColorScheme } from "@mantine/core";

import { formatActiveTab } from "@/lib/actions/formatActions";
import { runActiveTab } from "@/lib/actions/runActiveTab";
import {
  closeActiveTab,
  focusEditor,
  moveActiveTabLeft,
  moveActiveTabRight,
  newTab,
  nextTab,
  prevTab,
} from "@/lib/actions/tabActions";
import {
  setActivity,
  setResultTab,
  toggleColorScheme,
  toggleSidebar,
} from "@/lib/actions/uiActions";
import {
  closeActivePane,
  cycleViewInActivePane,
  focusNextPane,
  focusPrevPane,
  splitActivePane,
  toggleRootOrientation,
} from "@/lib/actions/workspaceActions";
import { openHotkeyHelpDialog } from "@/app/_components/Dialogs/HotkeyHelpDialog";

import { HOTKEYS } from "./labels";

export interface HotkeyContext {
  setColorScheme: (scheme: MantineColorScheme) => void;
  currentColorScheme: MantineColorScheme;
  computedColorScheme: "light" | "dark";
}

export type HotkeyEntry = [
  shortcut: string,
  handler: (event: KeyboardEvent) => void,
  opts?: { preventDefault?: boolean },
];

export function buildHotkeys(ctx: HotkeyContext): HotkeyEntry[] {
  return [
    [HOTKEYS.run.chord, () => { void runActiveTab(); }],
    [HOTKEYS.newTab.chord, () => { newTab(); }],
    [HOTKEYS.closeTab.chord, () => { closeActiveTab(); }],
    [HOTKEYS.toggleSidebar.chord, () => { toggleSidebar(); }],
    // mod+K — Spotlight installs its own listener via the `shortcut` prop.
    [HOTKEYS.resultGraph.chord, () => { setResultTab("graph"); }],
    [HOTKEYS.resultTable.chord, () => { setResultTab("table"); }],
    [HOTKEYS.resultJson.chord, () => { setResultTab("json"); }],
    [HOTKEYS.formatQuery.chord, () => { void formatActiveTab(); }],
    [HOTKEYS.activityQueries.chord, () => { setActivity("queries"); }],
    [HOTKEYS.activitySchema.chord, () => { setActivity("schema"); }],
    [HOTKEYS.activitySnapshots.chord, () => { setActivity("snapshots"); }],
    [HOTKEYS.activityHistory.chord, () => { setActivity("history"); }],
    [HOTKEYS.activitySettings.chord, () => { setActivity("settings"); }],
    [HOTKEYS.prevTab.chord, () => { prevTab(); }],
    [HOTKEYS.nextTab.chord, () => { nextTab(); }],
    [HOTKEYS.moveTabLeft.chord, () => { moveActiveTabLeft(); }],
    [HOTKEYS.moveTabRight.chord, () => { moveActiveTabRight(); }],
    [HOTKEYS.focusEditor.chord, () => { focusEditor(); }],
    [HOTKEYS.help.chord, () => { openHotkeyHelpDialog(); }],
    [HOTKEYS.splitRight.chord, () => { splitActivePane("row", "after"); }],
    [HOTKEYS.splitDown.chord, () => { splitActivePane("column", "after"); }],
    [HOTKEYS.closePane.chord, () => { closeActivePane(); }],
    [HOTKEYS.focusNextPane.chord, () => { focusNextPane(); }],
    [HOTKEYS.focusPrevPane.chord, () => { focusPrevPane(); }],
    [HOTKEYS.cycleViewInPane.chord, () => { cycleViewInActivePane(); }],
    [HOTKEYS.toggleOrientation.chord, () => { toggleRootOrientation(); }],
    [
      HOTKEYS.toggleColorScheme.chord,
      () => {
        toggleColorScheme({
          setColorScheme: ctx.setColorScheme,
          current: ctx.currentColorScheme,
          computed: ctx.computedColorScheme,
        });
      },
    ],
  ];
}
