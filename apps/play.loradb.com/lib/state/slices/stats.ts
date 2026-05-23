/**
 * Stats slice — cached `GraphStats` + `MemoryReport` snapshots backing
 * the Stats side panel.
 *
 * The cache is rebuilt by `refreshStats()` after every mutation that
 * matters (snapshot load, large import, manual refresh button, or a
 * `loradb:mutation` window event debounced to ~600ms). Neither
 * `graphStats` nor `memoryReport` is on a hot path on small/medium
 * graphs but both walk every owned structure, so we don't auto-poll.
 *
 * `history` is an in-memory ring buffer of the last
 * {@link STATS_HISTORY_LIMIT} samples — used by the panel to draw a
 * mini growth sparkline. Not persisted: it resets on reload because
 * the database does too (everything is in-memory).
 */

import type { StateCreator } from "zustand";
import type {
  GraphStatsSnapshot,
  MemoryReportSnapshot,
} from "@loradb/lora-wasm";

export const STATS_HISTORY_LIMIT = 60;

export interface StatsSample {
  /** `Date.now()` at the moment the sample landed. */
  takenAt: number;
  totalBytes: number;
  nodeCount: number;
  relationshipCount: number;
}

export interface StatsSlice {
  /** Last `GraphStats` snapshot, or `null` before first fetch. */
  graphStats: GraphStatsSnapshot | null;
  /** Last `MemoryReport` snapshot, or `null` before first fetch. */
  memoryReport: MemoryReportSnapshot | null;
  /** True while a fetch is in flight. */
  statsRefreshing: boolean;
  /** `Date.now()` of the last successful fetch, or `null`. */
  statsFetchedAt: number | null;
  /** Ring buffer of the last few samples for the sparkline. */
  statsHistory: StatsSample[];

  setStatsSnapshot(snap: {
    graphStats: GraphStatsSnapshot;
    memoryReport: MemoryReportSnapshot;
  }): void;
  setStatsRefreshing(v: boolean): void;
  clearStatsHistory(): void;
}

/**
 * Compute the total retained bytes of a `MemoryReport`. Mirrors the
 * `MemoryReport::total_bytes()` helper on the Rust side so we don't
 * have to ship that calculation through the wire — every field is a
 * scalar we already have.
 */
export function memoryReportTotalBytes(r: MemoryReportSnapshot): number {
  return (
    r.nodesBytes +
    r.relationshipsBytes +
    r.outgoingBytes +
    r.incomingBytes +
    r.labelIndexBytes +
    r.typeIndexBytes +
    r.propertyIndexBytes +
    r.sortedIndexBytes +
    r.textIndexBytes +
    r.pointIndexBytes +
    r.fulltextIndexBytes +
    r.vectorIndexBytes +
    r.indexCatalogBytes +
    r.constraintCatalogBytes
  );
}

export const createStatsSlice: StateCreator<
  StatsSlice,
  [["zustand/immer", never]],
  [],
  StatsSlice
> = (set) => ({
  graphStats: null,
  memoryReport: null,
  statsRefreshing: false,
  statsFetchedAt: null,
  statsHistory: [],

  setStatsSnapshot(snap) {
    set((state) => {
      state.graphStats = snap.graphStats;
      state.memoryReport = snap.memoryReport;
      state.statsFetchedAt = Date.now();
      const sample: StatsSample = {
        takenAt: state.statsFetchedAt,
        totalBytes: memoryReportTotalBytes(snap.memoryReport),
        nodeCount: snap.graphStats.nodeCount,
        relationshipCount: snap.graphStats.relationshipCount,
      };
      state.statsHistory.push(sample);
      if (state.statsHistory.length > STATS_HISTORY_LIMIT) {
        state.statsHistory.splice(
          0,
          state.statsHistory.length - STATS_HISTORY_LIMIT,
        );
      }
    });
  },

  setStatsRefreshing(v) {
    set((state) => {
      state.statsRefreshing = v;
    });
  },

  clearStatsHistory() {
    set((state) => {
      state.statsHistory = [];
    });
  },
});
