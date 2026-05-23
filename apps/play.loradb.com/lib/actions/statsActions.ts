"use client";

/**
 * Imperative actions for the Stats side panel.
 *
 * - `refreshStats()` pulls a fresh `GraphStats` + `MemoryReport` pair
 *   in parallel and pushes both into the store. Safe to call
 *   concurrently — the last resolution wins (older responses are
 *   dropped).
 * - `attachStatsMutationListener()` mirrors the schema-design one:
 *   re-fetches the snapshots (debounced) on `loradb:mutation` so the
 *   panel stays roughly current during a long-running import.
 *
 * Neither hook auto-polls — both queries walk every owned graph
 * structure once. Calling them on every keystroke would amplify
 * import overhead noticeably on million-node graphs.
 */

import { notifications } from "@mantine/notifications";

import { useStore } from "@/lib/state/store";
import { debounce } from "@/lib/util/async";
import { LORADB_MUTATION_EVENT } from "@/lib/actions/runActiveTab";
import { graphStats, memoryReport } from "@/lib/db/client";

let inFlight = 0;

export async function refreshStats(): Promise<void> {
  const state = useStore.getState();
  state.setStatsRefreshing(true);
  const ticket = ++inFlight;
  try {
    const [stats, report] = await Promise.all([graphStats(), memoryReport()]);
    if (ticket !== inFlight) return;
    useStore.getState().setStatsSnapshot({
      graphStats: stats,
      memoryReport: report,
    });
  } catch (err) {
    if (ticket !== inFlight) return;
    const message = err instanceof Error ? err.message : String(err);
    notifications.show({
      color: "red",
      title: "Couldn't refresh database stats",
      message,
    });
  } finally {
    if (ticket === inFlight) {
      useStore.getState().setStatsRefreshing(false);
    }
  }
}

/**
 * Re-fetch on the WASM mutation event, debounced. Returns the
 * teardown function so layout effects can unsubscribe on unmount.
 */
export function attachStatsMutationListener(): () => void {
  if (typeof window === "undefined") return () => {};
  const debounced = debounce(() => {
    void refreshStats();
  }, 600);
  const handler = (): void => {
    debounced();
  };
  window.addEventListener(LORADB_MUTATION_EVENT, handler);
  return () => {
    window.removeEventListener(LORADB_MUTATION_EVENT, handler);
    debounced.cancel();
  };
}
