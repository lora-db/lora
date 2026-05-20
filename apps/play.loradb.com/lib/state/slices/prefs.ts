/**
 * Prefs slice — user-tunable knobs the workbench should remember across
 * sessions.
 *
 * Color scheme is intentionally NOT here: Mantine owns that one through its
 * own `useMantineColorScheme` persistence layer.
 */

import type { StateCreator } from "zustand";

export interface SerializedPrefs {
  graphMode: "2d" | "3d";
  autoRunOnSave: boolean;
  autoFormatOnRun: boolean;
  nodeCap: number;
  resultRowCap: number;
  autoRestore: boolean;
  focusOnNodeClick: boolean;
  alwaysShowLabels: boolean;
  fitOnSelect: boolean;
}

export interface PrefsSlice extends SerializedPrefs {
  setPref<K extends keyof SerializedPrefs>(
    key: K,
    value: SerializedPrefs[K],
  ): void;
  hydratePrefs(prefs: SerializedPrefs): void;
}

export const DEFAULT_PREFS: SerializedPrefs = {
  graphMode: "2d",
  autoRunOnSave: false,
  autoFormatOnRun: false,
  nodeCap: 500,
  resultRowCap: 100000,
  autoRestore: true,
  focusOnNodeClick: false,
  alwaysShowLabels: false,
  fitOnSelect: false,
};

export const createPrefsSlice: StateCreator<
  PrefsSlice,
  [["zustand/immer", never]],
  [],
  PrefsSlice
> = (set) => ({
  ...DEFAULT_PREFS,

  setPref(key, value) {
    set((state) => {
      // Immer's WritableDraft is fine here — the union is enforced by the
      // generic signature so this assignment is sound at the type level.
      (state as SerializedPrefs)[key] = value;
    });
  },

  hydratePrefs(prefs) {
    set((state) => {
      state.graphMode = prefs.graphMode;
      state.autoRunOnSave = prefs.autoRunOnSave;
      state.autoFormatOnRun =
        prefs.autoFormatOnRun ?? DEFAULT_PREFS.autoFormatOnRun;
      state.nodeCap = prefs.nodeCap;
      state.resultRowCap = prefs.resultRowCap;
      // `autoRestore` was added in Phase 4b; treat a missing value as the
      // default (on) so existing sessions opt in transparently.
      state.autoRestore = prefs.autoRestore ?? DEFAULT_PREFS.autoRestore;
      state.focusOnNodeClick =
        prefs.focusOnNodeClick ?? DEFAULT_PREFS.focusOnNodeClick;
      state.alwaysShowLabels =
        prefs.alwaysShowLabels ?? DEFAULT_PREFS.alwaysShowLabels;
      state.fitOnSelect = prefs.fitOnSelect ?? DEFAULT_PREFS.fitOnSelect;
    });
  },
});
