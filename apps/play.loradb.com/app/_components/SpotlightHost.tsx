"use client";

/**
 * Command palette host. Renders the Mantine 7 `<Spotlight>` modal with
 * the playground's action list. The `mod+K` opener lives in
 * `HotkeyHost` (it bridges the contentEditable gap that CodeMirror
 * creates), so the `<Spotlight>` element itself registers no shortcut.
 */

import { useMemo } from "react";
import {
  useComputedColorScheme,
  useMantineColorScheme,
} from "@mantine/core";
import { Spotlight, type SpotlightActionData } from "@mantine/spotlight";
import {
  IconArrowsLeftRight,
  IconBinaryTree,
  IconBraces,
  IconCamera,
  IconDatabase,
  IconFileText,
  IconHistory,
  IconKeyboard,
  IconLayoutColumns,
  IconLayoutRows,
  IconLayoutSidebar,
  IconMoon,
  IconPhoto,
  IconPlayerPlay,
  IconPlus,
  IconSchema,
  IconSettings,
  IconSun,
  IconTable,
  IconWand,
  IconX,
} from "@tabler/icons-react";

import { requestGraphPng } from "@/lib/actions/exportActions";
import { openHotkeyHelpDialog } from "./Dialogs/HotkeyHelpDialog";
import { formatActiveTab } from "@/lib/actions/formatActions";
import { runActiveTab } from "@/lib/actions/runActiveTab";
import { closeActiveTab, newTab } from "@/lib/actions/tabActions";
import {
  setActivity,
  setResultTab,
  toggleColorScheme,
  toggleSidebar,
} from "@/lib/actions/uiActions";
import {
  closeActivePane,
  focusNextPane,
  focusPrevPane,
  splitActivePane,
  toggleRootOrientation,
} from "@/lib/actions/workspaceActions";
import { useStore } from "@/lib/state/store";

export function SpotlightHost() {
  const { colorScheme, setColorScheme } = useMantineColorScheme();
  const computed = useComputedColorScheme("dark", {
    getInitialValueInEffect: false,
  });
  const effectiveScheme = colorScheme === "auto" ? computed : colorScheme;
  const isDark = effectiveScheme === "dark";
  const autoRestore = useStore((s) => s.autoRestore);

  const actions = useMemo<SpotlightActionData[]>(
    () => [
      {
        id: "run",
        label: "Run query",
        description: "Execute the active editor's query",
        keywords: ["run", "execute", "play"],
        onClick: () => {
          void runActiveTab();
        },
        leftSection: <IconPlayerPlay size={16} />,
      },
      {
        id: "format",
        label: "Format query",
        description: "Reformat the active tab's Cypher",
        keywords: ["format", "pretty", "indent"],
        onClick: () => {
          void formatActiveTab();
        },
        leftSection: <IconWand size={16} />,
      },
      {
        id: "new-tab",
        label: "New query tab",
        keywords: ["new", "tab", "open"],
        onClick: () => {
          newTab();
        },
        leftSection: <IconPlus size={16} />,
      },
      {
        id: "close-tab",
        label: "Close current tab",
        keywords: ["close", "tab"],
        onClick: () => {
          closeActiveTab();
        },
        leftSection: <IconX size={16} />,
      },
      {
        id: "toggle-sidebar",
        label: "Toggle sidebar",
        keywords: ["sidebar", "panel"],
        onClick: () => {
          toggleSidebar();
        },
        leftSection: <IconLayoutSidebar size={16} />,
      },
      {
        id: "view-graph",
        label: "View: Graph",
        keywords: ["graph", "view", "result"],
        onClick: () => {
          setResultTab("graph");
        },
        leftSection: <IconBinaryTree size={16} />,
      },
      {
        id: "view-table",
        label: "View: Table",
        keywords: ["table", "view", "result"],
        onClick: () => {
          setResultTab("table");
        },
        leftSection: <IconTable size={16} />,
      },
      {
        id: "view-json",
        label: "View: JSON",
        keywords: ["json", "view", "result"],
        onClick: () => {
          setResultTab("json");
        },
        leftSection: <IconBraces size={16} />,
      },
      {
        id: "section-queries",
        label: "Go to: Saved queries",
        keywords: ["queries", "saved"],
        onClick: () => {
          setActivity("queries");
        },
        leftSection: <IconFileText size={16} />,
      },
      {
        id: "section-schema",
        label: "Go to: Schema browser",
        keywords: ["schema", "labels", "browse"],
        onClick: () => {
          setActivity("schema");
        },
        leftSection: <IconSchema size={16} />,
      },
      {
        id: "section-snapshots",
        label: "Go to: Snapshots",
        keywords: ["snapshot", "save"],
        onClick: () => {
          setActivity("snapshots");
        },
        leftSection: <IconCamera size={16} />,
      },
      {
        id: "section-history",
        label: "Go to: History",
        keywords: ["history", "recent"],
        onClick: () => {
          setActivity("history");
        },
        leftSection: <IconHistory size={16} />,
      },
      {
        id: "section-settings",
        label: "Go to: Settings",
        keywords: ["settings", "preferences", "config"],
        onClick: () => {
          setActivity("settings");
        },
        leftSection: <IconSettings size={16} />,
      },
      {
        id: "toggle-theme",
        label: isDark ? "Switch to light theme" : "Switch to dark theme",
        keywords: ["theme", "dark", "light", "color"],
        onClick: () => {
          toggleColorScheme({
            setColorScheme,
            current: colorScheme,
            computed,
          });
        },
        leftSection: isDark ? <IconSun size={16} /> : <IconMoon size={16} />,
      },
      {
        id: "split-right",
        label: "Split pane right",
        description: "Split the active pane horizontally",
        keywords: ["split", "right", "pane", "window"],
        onClick: () => {
          splitActivePane("row", "after");
        },
        leftSection: <IconLayoutColumns size={16} />,
      },
      {
        id: "split-down",
        label: "Split pane down",
        description: "Split the active pane vertically",
        keywords: ["split", "down", "pane", "window"],
        onClick: () => {
          splitActivePane("column", "after");
        },
        leftSection: <IconLayoutRows size={16} />,
      },
      {
        id: "close-pane",
        label: "Close active pane",
        keywords: ["close", "pane", "window"],
        onClick: () => {
          closeActivePane();
        },
        leftSection: <IconX size={16} />,
      },
      {
        id: "toggle-orientation",
        label: "Toggle root split orientation",
        description: "Flip the root split between left/right and top/bottom",
        keywords: ["orientation", "flip", "rotate", "vertical", "horizontal"],
        onClick: () => {
          toggleRootOrientation();
        },
        leftSection: <IconArrowsLeftRight size={16} />,
      },
      {
        id: "focus-next-pane",
        label: "Focus next pane",
        keywords: ["focus", "next", "pane"],
        onClick: () => {
          focusNextPane();
        },
        leftSection: <IconLayoutColumns size={16} />,
      },
      {
        id: "focus-prev-pane",
        label: "Focus previous pane",
        keywords: ["focus", "previous", "pane"],
        onClick: () => {
          focusPrevPane();
        },
        leftSection: <IconLayoutColumns size={16} />,
      },
      {
        id: "keyboard-shortcuts",
        label: "Keyboard shortcuts",
        description: "Show every chord the workbench listens for",
        keywords: ["help", "hotkey", "shortcut", "cheat", "?"],
        onClick: () => {
          openHotkeyHelpDialog();
        },
        leftSection: <IconKeyboard size={16} />,
      },
      {
        id: "export-png",
        label: "Export graph as PNG",
        description: "Download a screenshot of the current graph view",
        keywords: ["export", "png", "screenshot", "image", "graph"],
        onClick: () => {
          requestGraphPng();
        },
        leftSection: <IconPhoto size={16} />,
      },
      {
        id: "toggle-auto-restore",
        label: autoRestore ? "Disable auto-restore" : "Enable auto-restore",
        description: "Auto-save the database to local storage between reloads",
        keywords: ["auto", "restore", "save", "persistence", "local"],
        onClick: () => {
          const s = useStore.getState();
          s.setPref("autoRestore", !s.autoRestore);
        },
        leftSection: <IconDatabase size={16} />,
      },
    ],
    [isDark, setColorScheme, colorScheme, computed, autoRestore],
  );

  return (
    <Spotlight
      actions={actions}
      nothingFound="No matching commands"
      shortcut={null}
      searchProps={{ placeholder: "Search commands..." }}
    />
  );
}
