"use client";

/**
 * Settings panel — the only Phase-2 sidebar surface with real behavior.
 * Mantine controls live-wire to the prefs slice and the global color
 * scheme so changes here take effect immediately.
 */

import {
  Button,
  NumberInput,
  Radio,
  ScrollArea,
  SegmentedControl,
  Stack,
  Switch,
  Text,
  useMantineColorScheme,
  type MantineColorScheme,
} from "@mantine/core";
import { openConfirmModal } from "@mantine/modals";
import { notifications } from "@mantine/notifications";

import { reset as resetWasmDb } from "@/lib/db/client";
import { clearAuto } from "@/lib/persistence/autoSnapshot";
import { resetDB } from "@/lib/persistence/idb";
import { useStore } from "@/lib/state/store";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

export function SettingsPanel() {
  const { tokens } = usePlaygroundTheme();
  const { colorScheme, setColorScheme } = useMantineColorScheme();

  const graphMode = useStore((s) => s.graphMode);
  const autoRunOnSave = useStore((s) => s.autoRunOnSave);
  const autoRestore = useStore((s) => s.autoRestore);
  const nodeCap = useStore((s) => s.nodeCap);
  const resultRowCap = useStore((s) => s.resultRowCap);
  const focusOnNodeClick = useStore((s) => s.focusOnNodeClick);
  const alwaysShowLabels = useStore((s) => s.alwaysShowLabels);
  const fitOnSelect = useStore((s) => s.fitOnSelect);
  const setPref = useStore((s) => s.setPref);

  const onConfirmClear = () => {
    openConfirmModal({
      title: "Clear all local data?",
      children: (
        <Text size="sm" c={tokens.fg.muted}>
          This wipes the playground database (saved queries, history,
          snapshots, and the in-memory graph). The page will reload.
        </Text>
      ),
      labels: { confirm: "Clear", cancel: "Cancel" },
      confirmProps: { color: "red", "data-autofocus": "true" },
      onConfirm: () => {
        (async () => {
          try {
            // Order matters. The WASM in-memory graph must be wiped
            // BEFORE we leave this function: the `beforeunload` hook
            // installed by `startAutoSaveLoop` flushes a fresh
            // snapshot of the live DB into localStorage on its way
            // out, so if the graph is still populated when reload
            // fires, `bootAutoRestore` rehydrates the very data we
            // just promised to delete. Clearing the localStorage
            // slot afterwards isn't enough on its own for the same
            // reason — the flush would re-populate it.
            await resetWasmDb();
            clearAuto();
            await resetDB();
            notifications.show({
              color: "green",
              title: "Local data cleared",
              message: "Reloading the playground...",
            });
            if (typeof window !== "undefined") {
              window.location.reload();
            }
          } catch (err) {
            notifications.show({
              color: "red",
              title: "Failed to clear data",
              message: err instanceof Error ? err.message : String(err),
            });
          }
        })().catch(() => {
          // Swallowed; notifications above already inform the user.
        });
      },
    });
  };

  return (
    <Stack gap={0} style={{ flex: 1, minHeight: 0 }}>
      <div
        style={{
          padding: "10px 12px",
          borderBottom: `1px solid ${tokens.border.subtle}`,
          flexShrink: 0,
        }}
      >
        <Text
          size="xs"
          fw={600}
          c={tokens.fg.muted}
          style={{ letterSpacing: 1, textTransform: "uppercase" }}
        >
          Settings
        </Text>
      </div>
      <ScrollArea style={{ flex: 1, minHeight: 0 }}>
        <Stack gap="md" p={12}>
          <Stack gap={6}>
            <Text size="xs" fw={600} c={tokens.fg.muted}>
              Theme
            </Text>
            <Radio.Group
              value={colorScheme}
              onChange={(value) => {
                setColorScheme(value as MantineColorScheme);
              }}
            >
              <Stack gap={4}>
                <Radio value="dark" label="Dark" />
                <Radio value="light" label="Light" />
                <Radio value="auto" label="Auto" />
              </Stack>
            </Radio.Group>
          </Stack>

          <Stack gap={6}>
            <Text size="xs" fw={600} c={tokens.fg.muted}>
              Graph mode
            </Text>
            <SegmentedControl
              value={graphMode}
              onChange={(value) => {
                setPref("graphMode", value as "2d" | "3d");
              }}
              data={[
                { label: "2D", value: "2d" },
                { label: "3D", value: "3d" },
              ]}
              fullWidth
            />
          </Stack>

          <Stack gap={4}>
            <Switch
              size="sm"
              checked={focusOnNodeClick}
              onChange={(e) => {
                setPref("focusOnNodeClick", e.currentTarget.checked);
              }}
              label="Zoom on node click"
            />
            <Text size="xs" c={tokens.fg.muted}>
              Animates the camera toward a clicked node; click again to
              restore the prior view.
            </Text>
          </Stack>

          <Stack gap={4}>
            <Switch
              size="sm"
              checked={alwaysShowLabels}
              onChange={(e) => {
                setPref("alwaysShowLabels", e.currentTarget.checked);
              }}
              label="Always show labels"
            />
            <Text size="xs" c={tokens.fg.muted}>
              Render every node&apos;s label on the canvas (2D only).
            </Text>
          </Stack>

          <Stack gap={4}>
            <Switch
              size="sm"
              checked={fitOnSelect}
              onChange={(e) => {
                setPref("fitOnSelect", e.currentTarget.checked);
              }}
              label="Fit to selection"
            />
            <Text size="xs" c={tokens.fg.muted}>
              Animate the camera to frame the current selection whenever
              it changes.
            </Text>
          </Stack>

          <Switch
            size="sm"
            checked={autoRunOnSave}
            onChange={(e) => {
              setPref("autoRunOnSave", e.currentTarget.checked);
            }}
            label="Auto-run on save"
          />

          <NumberInput
            size="xs"
            label="Node cap"
            description="Maximum nodes rendered in the graph"
            value={nodeCap}
            onChange={(value) => {
              if (typeof value === "number" && Number.isFinite(value)) {
                setPref("nodeCap", value);
              }
            }}
            min={1000}
            max={50000}
            step={1000}
          />

          <NumberInput
            size="xs"
            label="Result row cap"
            description="Maximum rows kept per run"
            value={resultRowCap}
            onChange={(value) => {
              if (typeof value === "number" && Number.isFinite(value)) {
                setPref("resultRowCap", value);
              }
            }}
            min={1000}
            max={1000000}
            step={1000}
          />

          <Stack gap={4}>
            <Switch
              size="sm"
              checked={autoRestore}
              onChange={(e) => {
                setPref("autoRestore", e.currentTarget.checked);
              }}
              label="Auto-save DB to local storage"
            />
            <Text size="xs" c={tokens.fg.muted}>
              When on, the database is restored after a page reload.
            </Text>
          </Stack>

          <Stack gap={4}>
            <Text size="xs" fw={600} c={tokens.fg.muted}>
              Layout
            </Text>
            <Text size="xs" c={tokens.fg.muted}>
              Restore the default editor / result split. Open tabs and
              saved queries are kept; only the pane arrangement is reset.
            </Text>
            <Button
              size="xs"
              variant="default"
              onClick={() => {
                openConfirmModal({
                  title: "Reset workspace layout?",
                  children: (
                    <Text size="sm" c={tokens.fg.muted}>
                      This collapses any split panes back to a single
                      editor pane and a single result pane. Your open
                      tabs and their bodies remain intact.
                    </Text>
                  ),
                  labels: { confirm: "Reset", cancel: "Cancel" },
                  confirmProps: { color: "blue", "data-autofocus": "true" },
                  onConfirm: () => {
                    useStore.getState().resetLayout();
                    notifications.show({
                      color: "green",
                      title: "Layout reset",
                      message: "Workspace restored to the default split.",
                      autoClose: 3000,
                    });
                  },
                });
              }}
            >
              Reset workspace layout
            </Button>
          </Stack>

          <Button
            size="xs"
            variant="light"
            color="red"
            onClick={onConfirmClear}
          >
            Clear all local data
          </Button>
        </Stack>
      </ScrollArea>
    </Stack>
  );
}
