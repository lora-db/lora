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
import type {
  ActivitySection,
  ResultTab,
} from "@/lib/state/slices/layout";

export function setResultTab(tab: ResultTab): void {
  useStore.getState().setResultTab(tab);
}

export function setActivity(section: ActivitySection): void {
  const state = useStore.getState();
  state.setActivitySection(section);
  if (!state.sidebarOpen) state.toggleSidebar();
}

export function toggleSidebar(): void {
  useStore.getState().toggleSidebar();
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
