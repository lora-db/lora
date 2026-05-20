"use client";

/**
 * Sidebar shell — routes between the five section panels based on the
 * layout slice's `activitySection`. The right edge is a drag handle so
 * the user can resize the sidebar; the width is persisted via the
 * layout slice (`sidebarWidth`, clamped to MIN_SIDEBAR_WIDTH /
 * MAX_SIDEBAR_WIDTH).
 */

import { useCallback, useRef, useState } from "react";

import { useStore } from "@/lib/state/store";
import type { ActivitySection } from "@/lib/state/slices/layout";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import {
  MAX_SIDEBAR_WIDTH,
  MIN_SIDEBAR_WIDTH,
} from "@/lib/state/workspace/default";

import { HistoryPanel } from "./HistoryPanel";
import { SavedQueriesPanel } from "./SavedQueriesPanel";
import { SchemaBrowserPanel } from "./SchemaBrowserPanel";
import { SchemaDesignPanel } from "./SchemaDesignPanel";
import { SettingsPanel } from "./SettingsPanel";
import { SnapshotsPanel } from "./SnapshotsPanel";

const SECTION_LABEL: Record<ActivitySection, string> = {
  queries: "Saved queries",
  schema: "Schema",
  schemaDesign: "Schema design",
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
    case "schemaDesign":
      return <SchemaDesignPanel />;
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
  const width = useStore((s) => s.sidebarWidth);
  const setSidebarWidth = useStore((s) => s.setSidebarWidth);
  const asideRef = useRef<HTMLElement | null>(null);
  const [drafting, setDrafting] = useState<number | null>(null);

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      e.preventDefault();
      (e.target as HTMLElement).setPointerCapture(e.pointerId);
      setDrafting(width);
    },
    [width],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      if (drafting === null) return;
      const el = asideRef.current;
      if (!el) return;
      const rect = el.getBoundingClientRect();
      const next = Math.min(
        MAX_SIDEBAR_WIDTH,
        Math.max(MIN_SIDEBAR_WIDTH, e.clientX - rect.left),
      );
      setDrafting(Math.round(next));
    },
    [drafting],
  );

  const onPointerUp = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      try {
        (e.target as HTMLElement).releasePointerCapture(e.pointerId);
      } catch {
        /* ignore — pointer may already be released */
      }
      if (drafting !== null) {
        setSidebarWidth(drafting);
      }
      setDrafting(null);
    },
    [drafting, setSidebarWidth],
  );

  const renderedWidth = drafting ?? width;

  return (
    <aside
      ref={asideRef}
      aria-label={`${SECTION_LABEL[section]} panel`}
      style={{
        width: renderedWidth,
        flexShrink: 0,
        background: tokens.bg.panel,
        borderRight: `1px solid ${tokens.border.subtle}`,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        position: "relative",
      }}
    >
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
      <div
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize sidebar"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        style={{
          position: "absolute",
          top: 0,
          right: -2,
          bottom: 0,
          width: 5,
          cursor: "col-resize",
          touchAction: "none",
          userSelect: "none",
          zIndex: 2,
        }}
      />
    </aside>
  );
}

/**
 * Default starting width used by ActivityBar / Workbench layout helpers
 * that historically imported `SIDEBAR_WIDTH` from this module.
 */
export const SIDEBAR_WIDTH = 280;
