"use client";

/**
 * The main playground shell. Mantine `AppShell` with header + main +
 * footer; the `main` region is a horizontal row containing the
 * Activity Bar, the optional Sidebar, and a recursive split workspace
 * (`PanelHost`) driven by `react-resizable-panels`.
 */

import { useEffect, useRef } from "react";
import { AppShell, Button, Code, Group, Modal, Stack, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { IconAlertTriangle, IconRefresh } from "@tabler/icons-react";

import {
  bootAutoRestore,
  startAutoSaveLoop,
} from "@/lib/actions/autoRestoreActions";
import { hydrateFromIDB, useStore } from "@/lib/state/store";
import { healOrphanedTabs } from "@/lib/state/workspace/default";
import { validateWorkspace } from "@/lib/state/workspace/validate";
import { useDbStatus } from "@/lib/hooks/useDbStatus";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

import { ActivityBar } from "./ActivityBar";
import { DropZone } from "./DropZone";
import { HotkeyHost } from "./HotkeyHost";
import { InspectorDrawer } from "./Inspector/InspectorDrawer";
import { PanelHost } from "./Layout/PanelHost";
import { SidebarRoot } from "./Sidebar/SidebarRoot";
import { SpotlightHost } from "./SpotlightHost";
import { StatusBar } from "./StatusBar";
import { TopBar } from "./TopBar";

const HEADER_H = 44;
const FOOTER_H = 24;

export function Workbench() {
  const { tokens } = usePlaygroundTheme();
  // Boot the DB lazily; the StatusBar consumes the same hook
  // independently, but mounting it here ensures the WASM kick-off
  // happens regardless of which surfaces are visible. We also surface
  // a retry modal when the boot times out or errors so users aren't
  // stuck staring at a half-functional workbench.
  const dbStatus = useDbStatus();
  const bootErrored = dbStatus.state === "error";

  const sidebarOpen = useStore((s) => s.sidebarOpen);

  // Hydrate persisted state once on mount, then ensure there's at least
  // one tab to land in and that the first editor view's strip lists every
  // open tab (legacy sessions migrate without per-view tab order).
  const hydratedRef = useRef(false);
  useEffect(() => {
    if (hydratedRef.current) return;
    hydratedRef.current = true;
    const ensureFirstTab = () => {
      const st = useStore.getState();
      if (st.tabs.length === 0) {
        st.openTab({
          name: "Query 1",
          body: "MATCH (n)\nOPTIONAL MATCH (n)-[r]->(m)\nRETURN n, r, m",
        });
      }
    };
    const ensureEditorStrip = () => {
      const st = useStore.getState();
      const tabIds = st.tabs.map((t) => t.id);
      const healed = healOrphanedTabs(st.workspace, tabIds);
      if (healed !== st.workspace) {
        st.replaceWorkspace(healed);
      }
    };
    const failsafeIfBroken = () => {
      const st = useStore.getState();
      const tabIds = new Set(st.tabs.map((t) => t.id));
      const reason = validateWorkspace(st.workspace, {
        activePaneId: st.activePaneId,
        tabIds,
      });
      if (reason !== null) {
        console.warn("workspace validation failed, resetting layout:", reason);
        notifications.show({
          color: "yellow",
          title: "Layout reset",
          message:
            "Your saved layout looked off, so we restored the default editor / result split.",
          autoClose: 6000,
        });
        st.resetLayout();
      }
    };
    (async () => {
      await hydrateFromIDB();
      ensureFirstTab();
      ensureEditorStrip();
      failsafeIfBroken();
    })().catch((err) => {
      // Hydration failure is non-fatal — fall back to a fresh tab and
      // tell the user so they aren't surprised when saved tabs are gone.
      console.warn("hydrateFromIDB failed", err);
      notifications.show({
        color: "yellow",
        title: "Couldn't load saved tabs",
        message: "Starting with a fresh editor.",
        autoClose: 6000,
      });
      ensureFirstTab();
      ensureEditorStrip();
      failsafeIfBroken();
    });
  }, []);

  // Auto-restore: rehydrate the WASM DB from the last auto-saved
  // snapshot (if any), then start a debounced save loop that mirrors
  // mutations back to localStorage. Mounted once thanks to StrictMode
  // double-fire being guarded inside `bootAutoRestore`.
  useEffect(() => {
    let detach: (() => void) | null = null;
    (async () => {
      await bootAutoRestore();
      detach = startAutoSaveLoop();
    })().catch((err) => {
      // bootAutoRestore handles its own user-facing notification on a
      // snapshot load failure; this catch is reached only if the wiring
      // itself throws (e.g. WASM init rejected). Log so support has a
      // breadcrumb without spamming a second toast.
      console.warn("auto-restore wiring failed", err);
    });
    return () => {
      detach?.();
    };
  }, []);

  return (
    <AppShell
      header={{ height: HEADER_H }}
      footer={{ height: FOOTER_H }}
      padding={0}
      styles={{
        main: {
          background: tokens.bg.app,
        },
      }}
    >
      <AppShell.Header style={{ background: tokens.bg.panel }}>
        <TopBar />
      </AppShell.Header>

      {/*
        `AppShell.Main` already gets `padding-top: var(--app-shell-header-offset)`
        and `padding-bottom: var(--app-shell-footer-offset)` from Mantine. With
        `box-sizing: border-box` (set globally), pinning the border-box to
        `100dvh` makes the content area exactly `100dvh − header − footer`.

        We use `100dvh` (not `100vh`) so the work area shrinks to match the
        visible viewport on mobile when the URL bar collapses, instead of
        forcing page-level scroll.
      */}
      <AppShell.Main
        style={{
          height: "100dvh",
          minHeight: 0,
          display: "flex",
          flexDirection: "row",
          overflow: "hidden",
        }}
      >
        <ActivityBar />
        {sidebarOpen && <SidebarRoot />}

        <div
          style={{
            flex: 1,
            minWidth: 0,
            minHeight: 0,
            display: "flex",
            flexDirection: "column",
            overflow: "hidden",
            background: tokens.bg.app,
          }}
        >
          <PanelHost />
        </div>
      </AppShell.Main>

      <AppShell.Footer style={{ background: tokens.bg.panel }}>
        <StatusBar />
      </AppShell.Footer>

      <HotkeyHost />
      <SpotlightHost />
      <DropZone />
      <InspectorDrawer />

      <Modal
        opened={bootErrored}
        onClose={() => {
          // Intentionally a no-op — the modal can only be dismissed by
          // a successful retry or a hard reload. Letting it close while
          // the DB is dead leaves the workbench in a misleading state.
        }}
        withCloseButton={false}
        centered
        title={
          <Group gap={8} align="center">
            <IconAlertTriangle size={16} color={tokens.accent.warning} />
            <Text fw={600}>Database failed to start</Text>
          </Group>
        }
      >
        <Stack gap="sm">
          <Text size="sm" c={tokens.fg.muted}>
            The LoraDB WebAssembly module didn&rsquo;t finish booting. This is
            usually caused by a locked-down browser (private mode, blocked
            WASM origins) or a temporary network blip during the module
            fetch.
          </Text>
          {dbStatus.error ? (
            <Code
              block
              style={{
                whiteSpace: "pre-wrap",
                fontSize: 11,
                color: tokens.fg.muted,
                background: tokens.bg.panel,
              }}
            >
              {dbStatus.error}
            </Code>
          ) : null}
          <Group justify="flex-end" gap="xs">
            <Button
              variant="default"
              size="xs"
              onClick={() => {
                window.location.reload();
              }}
            >
              Reload page
            </Button>
            <Button
              size="xs"
              color="blue"
              leftSection={<IconRefresh size={14} />}
              onClick={() => {
                void dbStatus.retry();
              }}
            >
              Retry
            </Button>
          </Group>
        </Stack>
      </Modal>
    </AppShell>
  );
}
