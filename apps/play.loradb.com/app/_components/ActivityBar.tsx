"use client";

/**
 * VS-Code-style activity bar — a 48px vertical rail with one icon
 * button per section. Clicking a button selects the section and
 * forces the sidebar open if it was collapsed.
 */

import type { ComponentType } from "react";
import { Stack, Tooltip, UnstyledButton } from "@mantine/core";
import {
  IconCamera,
  IconFileText,
  IconHistory,
  IconSchema,
  IconSettings,
  type IconProps,
} from "@tabler/icons-react";

import { useStore } from "@/lib/state/store";
import type { ActivitySection } from "@/lib/state/slices/layout";
import { hexA } from "@/lib/theme/util";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

interface ActivityItem {
  section: ActivitySection;
  label: string;
  Icon: ComponentType<IconProps>;
}

const ITEMS: ReadonlyArray<ActivityItem> = [
  {
    section: "queries",
    label: "Saved queries",
    Icon: IconFileText,
  },
  {
    section: "schema",
    label: "Schema browser",
    Icon: IconSchema,
  },
  {
    section: "snapshots",
    label: "Snapshots",
    Icon: IconCamera,
  },
  {
    section: "history",
    label: "History",
    Icon: IconHistory,
  },
  {
    section: "settings",
    label: "Settings",
    Icon: IconSettings,
  },
];

export const ACTIVITY_BAR_WIDTH = 48;

export function ActivityBar() {
  const { tokens } = usePlaygroundTheme();
  const activeSection = useStore((s) => s.activitySection);
  const sidebarOpen = useStore((s) => s.sidebarOpen);
  const setActivitySection = useStore((s) => s.setActivitySection);
  const toggleSidebar = useStore((s) => s.toggleSidebar);

  return (
    <Stack
      gap={4}
      align="center"
      py={8}
      style={{
        width: ACTIVITY_BAR_WIDTH,
        flexShrink: 0,
        background: tokens.bg.sidebar,
        borderRight: `1px solid ${tokens.border.subtle}`,
      }}
      role="tablist"
      aria-label="Activity sections"
    >
      {ITEMS.map(({ section, label, Icon }) => {
        const active = section === activeSection && sidebarOpen;
        return (
          <Tooltip
            key={section}
            label={label}
            position="right"
            withArrow
          >
            <UnstyledButton
              role="tab"
              aria-selected={active}
              aria-label={label}
              onClick={() => {
                if (section === activeSection && sidebarOpen) {
                  // Re-clicking the active icon collapses the sidebar.
                  toggleSidebar();
                  return;
                }
                setActivitySection(section);
                if (!sidebarOpen) toggleSidebar();
              }}
              style={{
                width: 36,
                height: 36,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                borderRadius: tokens.radius.sm,
                color: active ? tokens.fg.primary : tokens.fg.muted,
                background: active
                  ? hexA(tokens.accent.primary, 0.25)
                  : "transparent",
                transition: "background 120ms ease, color 120ms ease",
                cursor: "pointer",
              }}
              onMouseEnter={(e) => {
                if (!active) {
                  (e.currentTarget as HTMLElement).style.background = hexA(
                    tokens.fg.primary,
                    0.06,
                  );
                }
              }}
              onMouseLeave={(e) => {
                if (!active) {
                  (e.currentTarget as HTMLElement).style.background =
                    "transparent";
                }
              }}
            >
              <Icon size={20} />
            </UnstyledButton>
          </Tooltip>
        );
      })}
    </Stack>
  );
}
