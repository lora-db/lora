/**
 * Results slice — per-tab run state.
 *
 * Each tab's result is either `undefined` (no run yet), a transient `running`
 * marker (so the UI can show a spinner without losing the previous result),
 * or a finished `RunOutcome` from the adapter layer.
 */

import type { StateCreator } from "zustand";

import type { RunOutcome } from "@/lib/db/types";

export interface RunningState {
  state: "running";
  runId: string;
  startedAt: number;
}

export type TabResult = RunOutcome | RunningState | undefined;

export interface ResultsSlice {
  results: Record<string, TabResult>;
  setRunning(tabId: string, runId: string, startedAt: number): void;
  setResult(tabId: string, outcome: RunOutcome): void;
  clearResult(tabId: string): void;
}

export const createResultsSlice: StateCreator<
  ResultsSlice,
  [["zustand/immer", never]],
  [],
  ResultsSlice
> = (set) => ({
  results: {},

  setRunning(tabId, runId, startedAt) {
    set((state) => {
      state.results[tabId] = { state: "running", runId, startedAt };
    });
  },

  setResult(tabId, outcome) {
    set((state) => {
      state.results[tabId] = outcome;
    });
  },

  clearResult(tabId) {
    set((state) => {
      delete state.results[tabId];
    });
  },
});
