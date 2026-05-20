"use client";

/**
 * Imperative UI actions — layout/section toggles called from hotkeys
 * and the Spotlight palette. These deliberately read the store via
 * `getState()` so they're cheap to invoke from outside React.
 *
 * `toggleColorScheme` is a special case: Mantine's colour scheme is
 * owned by `useMantineColorScheme`, which is a React hook and can't
 * be called from non-component code. Callers that need to flip the
 * scheme pass in the hook-derived API explicitly.
 */

import type { MantineColorScheme } from "@mantine/core";

import { useStore } from "@/lib/state/store";
import type { ActivitySection, ResultTab } from "@/lib/state/slices/layout";
import { setActiveResultTab } from "@/lib/actions/workspaceActions";
import { findLeaf, resolveActiveViewId } from "@/lib/state/workspace/tree";

/**
 * Switch the result inner-tab (Graph/Table/JSON/Plan). Acts on the
 * active pane if it shows a result view; otherwise the first result
 * view anywhere in the workspace.
 */
export function setResultTab(tab: ResultTab): void {
  setActiveResultTab(tab);
}

export function setActivity(section: ActivitySection): void {
  const state = useStore.getState();
  state.setActivitySection(section);
  if (!state.sidebarOpen) state.toggleSidebar();
}

export function toggleSidebar(): void {
  useStore.getState().toggleSidebar();
}

/**
 * Resolve the editor view the user is currently looking at — the
 * active leaf's active view, falling back to any view anywhere.
 * Returns `null` only when the tree has no editor view at all.
 */
function getActiveEditorViewId(): string | null {
  const state = useStore.getState();
  return resolveActiveViewId(state.workspace, state.activePaneId);
}

/** True iff the active editor view has the Params panel open. */
export function activeParamsPanelOpen(): boolean {
  const state = useStore.getState();
  const viewId = resolveActiveViewId(state.workspace, state.activePaneId);
  if (!viewId) return false;
  const leaf = findLeaf(state.workspace, state.activePaneId);
  const view = leaf?.views.find((v) => v.id === viewId);
  return view?.paramsPanelOpen ?? false;
}

/**
 * Toggle the Params panel on the active editor view. Shared by the
 * hotkey, the Spotlight command, and the toolbar/status-bar
 * indicators so a single source of truth governs which view flips.
 */
export function toggleParamsPanel(): void {
  const viewId = getActiveEditorViewId();
  if (!viewId) return;
  useStore
    .getState()
    .setParamsPanelOpenForView(viewId, !activeParamsPanelOpen());
}

export function setActiveParamsPanelOpen(open: boolean): void {
  const viewId = getActiveEditorViewId();
  if (!viewId) return;
  useStore.getState().setParamsPanelOpenForView(viewId, open);
}

export interface ColorSchemeApi {
  setColorScheme: (scheme: MantineColorScheme) => void;
  current: MantineColorScheme;
  computed: "light" | "dark";
}

export function toggleColorScheme(api: ColorSchemeApi): void {
  const current = api.current === "auto" ? api.computed : api.current;
  api.setColorScheme(current === "dark" ? "light" : "dark");
}
