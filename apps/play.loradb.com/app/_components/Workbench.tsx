"use client";

/**
 * The main playground shell. Mantine `AppShell` with header + main +
 * footer; the `main` region is a horizontal row containing the
 * Activity Bar, the optional Sidebar, and a vertical split with the
 * editor on top and the result pane below.
 *
 * TODO(phase-3): replace the CSS-grid split with a real docking layer
 * (e.g. dockview-react) so we can offer the multi-panel dockable layout
 * the design calls for. The dep is not installed yet — add it back in
 * the PR that introduces the layout.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { AppShell, Button, Code, Group, Modal, Stack, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { IconAlertTriangle, IconRefresh } from "@tabler/icons-react";

import {
  bootAutoRestore,
  startAutoSaveLoop,
} from "@/lib/actions/autoRestoreActions";
import { hydrateFromIDB, useStore } from "@/lib/state/store";
import { useDbStatus } from "@/lib/hooks/useDbStatus";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

import { ActivityBar } from "./ActivityBar";
import { DropZone } from "./DropZone";
import { EditorPane } from "./Editor/EditorPane";
import { EditorTabs } from "./Editor/EditorTabs";
import { HotkeyHost } from "./HotkeyHost";
import { ResultPane } from "./Result/ResultPane";
import { SidebarRoot } from "./Sidebar/SidebarRoot";
import { SpotlightHost } from "./SpotlightHost";
import { StatusBar } from "./StatusBar";
import { TopBar } from "./TopBar";

const HEADER_H = 44;
const FOOTER_H = 24;
const DIVIDER_H = 6;
const MIN_PANE = 120;
// Key used inside `layout.panelSizes` for the editor/result vertical split.
// Persisted as a 0..1 fraction of the work-area height.
const EDITOR_SPLIT_KEY = "editorSplit";
const EDITOR_SPLIT_DEFAULT = 0.5;

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
  const persistedEditorSplit = useStore(
    (s) => s.panelSizes[EDITOR_SPLIT_KEY] ?? EDITOR_SPLIT_DEFAULT,
  );
  const setPanelSize = useStore((s) => s.setPanelSize);

  // Hydrate persisted state once on mount, then ensure there's at
  // least one tab to land in.
  const hydratedRef = useRef(false);
  useEffect(() => {
    if (hydratedRef.current) return;
    hydratedRef.current = true;
    (async () => {
      await hydrateFromIDB();
      const st = useStore.getState();
      if (st.tabs.length === 0) {
        st.openTab({
          name: "Query 1",
          body: "MATCH (n)\nOPTIONAL MATCH (n)-[r]->(m)\nRETURN n, r, m",
        });
      }
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
      const st = useStore.getState();
      if (st.tabs.length === 0) {
        st.openTab({
          name: "Query 1",
          body: "MATCH (n)\nOPTIONAL MATCH (n)-[r]->(m)\nRETURN n, r, m",
        });
      }
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

  // Resizable split: top fraction of the work area, persisted as a
  // ratio (0..1) inside `layout.panelSizes`. The active drag drives a
  // local state for 60fps feedback; we only write to the store on
  // pointer-up to keep the persistence subscription quiet during drags.
  const splitRef = useRef<HTMLDivElement | null>(null);
  const [topFrac, setTopFrac] = useState(persistedEditorSplit);
  const dragging = useRef(false);

  // Re-sync local state when the persisted value lands from IDB. Without
  // this the first render uses `EDITOR_SPLIT_DEFAULT` and the user's
  // restored layout gets stomped a tick later.
  useEffect(() => {
    if (!dragging.current) {
      setTopFrac(persistedEditorSplit);
    }
  }, [persistedEditorSplit]);

  const onPointerDown = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    dragging.current = true;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  }, []);
  const onPointerUp = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      dragging.current = false;
      try {
        (e.target as HTMLElement).releasePointerCapture(e.pointerId);
      } catch {
        /* ignore — pointer may already be released */
      }
      // Commit the final position to the store on release. Writing on
      // every pointermove would queue dozens of IDB writes during a drag
      // even with the 500ms debounce.
      setPanelSize(EDITOR_SPLIT_KEY, topFrac);
    },
    [setPanelSize, topFrac],
  );
  const onPointerMove = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (!dragging.current) return;
    const el = splitRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const y = e.clientY - rect.top;
    const usable = Math.max(1, rect.height - DIVIDER_H);
    const minFrac = MIN_PANE / usable;
    const maxFrac = 1 - minFrac;
    const next = Math.min(maxFrac, Math.max(minFrac, y / rect.height));
    setTopFrac(next);
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
            display: "flex",
            flexDirection: "column",
            overflow: "hidden",
            background: tokens.bg.app,
          }}
        >
          <div
            ref={splitRef}
            style={{
              flex: 1,
              minHeight: 0,
              display: "grid",
              gridTemplateRows: `minmax(${MIN_PANE}px, ${topFrac}fr) ${DIVIDER_H}px minmax(${MIN_PANE}px, ${1 - topFrac}fr)`,
              background: tokens.bg.app,
            }}
          >
            <div
              style={{
                minHeight: 0,
                display: "flex",
                flexDirection: "column",
                overflow: "hidden",
                background: tokens.bg.editor,
              }}
            >
              <EditorTabs />
              <EditorPane />
            </div>

            <div
              role="separator"
              aria-orientation="horizontal"
              aria-label="Resize editor / results"
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerUp={onPointerUp}
              style={{
                cursor: "row-resize",
                background: tokens.border.subtle,
                userSelect: "none",
                touchAction: "none",
              }}
            />

            <div
              style={{
                minHeight: 0,
                display: "flex",
                flexDirection: "column",
                overflow: "hidden",
                background: tokens.bg.editor,
              }}
            >
              <ResultPane />
            </div>
          </div>
        </div>
      </AppShell.Main>

      <AppShell.Footer style={{ background: tokens.bg.panel }}>
        <StatusBar />
      </AppShell.Footer>

      <HotkeyHost />
      <SpotlightHost />
      <DropZone />

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
