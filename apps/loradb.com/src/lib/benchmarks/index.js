// Selectors that turn the raw `data.js` arrays into the shapes the
// pages need. Kept small and pure so unit-testing / refresh-only data
// edits don't have to touch every component.

import {
  COMPETITOR_TAGLINE,
  COMPETITORS,
  ENGINES,
  ENGINE_BY_ID,
  GROUPS,
  NOTES,
  TOTAL_ROW,
  WORKLOADS,
} from "./data.js";

export {
  COMPETITOR_TAGLINE,
  COMPETITORS,
  ENGINES,
  ENGINE_BY_ID,
  GROUPS,
  NOTES,
  TOTAL_ROW,
  WORKLOADS,
};

export const SUITE_META = {
  totalWorkloads: TOTAL_ROW.workloadCount,
  totalGroups: GROUPS.length,
  overallWinner: TOTAL_ROW.winner,
  // Where LoraDB is the group winner — used to back the "strongest
  // areas" chip strip on the hero. Order preserved from GROUPS.
  strongestGroups: GROUPS.filter((g) => g.winner === "lora").map((g) => g.id),
};

// `value` here matches data.js — either the literal string "fastest"
// or a numeric slowdown multiplier. Normalises both into a discriminated
// shape so views don't have to keep type-checking.
export function resolveCell(value) {
  if (value === "fastest") return { kind: "winner" };
  if (value == null) return { kind: "omitted" };
  if (typeof value === "number") return { kind: "ratio", value };
  return { kind: "unknown" };
}

export function formatRatio(value) {
  if (value == null) return "—";
  const n = Number(value);
  if (!Number.isFinite(n)) return "—";
  if (n >= 100) return `${n.toFixed(0)}×`;
  if (n >= 10) return `${n.toFixed(1)}×`;
  return `${n.toFixed(2)}×`;
}

// Sort competitors by total-row slowdown so the page ranks fastest →
// slowest. LoraDB sits at the top of the list as the winner.
export function competitorsByTotal() {
  return COMPETITORS.map((c) => ({
    engine: c,
    slowdown: TOTAL_ROW.summary[c.id],
  })).sort((a, b) => (a.slowdown ?? Infinity) - (b.slowdown ?? Infinity));
}

// Per-competitor: how many workloads ran for both engines, how many
// lora won, how many the competitor won, and how many were close (
// within 5%). Driven off the per-workload winner string in WORKLOADS,
// so it stays consistent with the rendered tables.
export function competitorBreakdown(engineId) {
  const groups = [];
  let totalWorkloads = 0;
  let loraWins = 0;
  let competitorWins = 0;
  let other = 0;
  let close = 0;

  for (const g of GROUPS) {
    const rows = WORKLOADS[g.id] ?? [];
    const comparable = rows.filter(
      (w) => w.results.lora?.time && w.results[engineId]?.time,
    );
    if (comparable.length === 0) continue;

    let gLora = 0;
    let gCompetitor = 0;
    let gOther = 0;
    for (const w of comparable) {
      if (w.winner === "lora") gLora++;
      else if (w.winner === engineId) gCompetitor++;
      else gOther++;

      const loraSlow = w.results.lora?.slowdown;
      const compSlow = w.results[engineId]?.slowdown;
      // "Close" = both sides within ~10% of the winner.
      if (
        (loraSlow == null || loraSlow <= 1.1) &&
        (compSlow == null || compSlow <= 1.1)
      ) {
        close++;
      }
    }

    totalWorkloads += comparable.length;
    loraWins += gLora;
    competitorWins += gCompetitor;
    other += gOther;

    groups.push({
      group: g,
      comparable,
      loraWins: gLora,
      competitorWins: gCompetitor,
      otherWins: gOther,
    });
  }

  return {
    totalWorkloads,
    loraWins,
    competitorWins,
    otherWins: other,
    closeCalls: close,
    groups,
  };
}

export function notesForGroup(groupId) {
  return NOTES.filter((n) => n.group === groupId);
}

export function notesForCompetitor(engineId) {
  return NOTES.filter((n) => n.engine === engineId);
}
