"use client";

/**
 * Sidebar shell — routes between the five section panels based on the
 * layout slice's `activitySection`. Width is fixed for now; Phase 3
 * will add a resizer.
 */

import { useStore } from "@/lib/state/store";
import type { ActivitySection } from "@/lib/state/slices/layout";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

import { HistoryPanel } from "./HistoryPanel";
import { SavedQueriesPanel } from "./SavedQueriesPanel";
import { SchemaBrowserPanel } from "./SchemaBrowserPanel";
import { SettingsPanel } from "./SettingsPanel";
import { SnapshotsPanel } from "./SnapshotsPanel";

export const SIDEBAR_WIDTH = 280;

const SECTION_LABEL: Record<ActivitySection, string> = {
  queries: "Saved queries",
  schema: "Schema",
  snapshots: "Snapshots",
  history: "History",
  settings: "Settings",
};

function renderPanel(section: ActivitySection) {
  switch (section) {
    case "queries":
      return <SavedQueriesPanel />;
    case "schema":
      return <SchemaBrowserPanel />;
    case "snapshots":
      return <SnapshotsPanel />;
    case "history":
      return <HistoryPanel />;
    case "settings":
      return <SettingsPanel />;
  }
}

export function SidebarRoot() {
  const { tokens } = usePlaygroundTheme();
  const section = useStore((s) => s.activitySection);

  return (
    <aside
      aria-label={`${SECTION_LABEL[section]} panel`}
      style={{
        width: SIDEBAR_WIDTH,
        flexShrink: 0,
        background: tokens.bg.panel,
        borderRight: `1px solid ${tokens.border.subtle}`,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
      }}
    >
      {/*
        Each panel renders its own header bar (label + toolbar) and
        manages its own scroll via an internal Mantine `ScrollArea`.
        This wrapper is just a flex column that propagates the
        available height — no overflow handling, no native scrollbar.
      */}
      <div
        style={{
          flex: 1,
          minHeight: 0,
          display: "flex",
          flexDirection: "column",
        }}
      >
        {renderPanel(section)}
      </div>
    </aside>
  );
}
