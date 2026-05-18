"use client";

/**
 * Imperative "run the active tab" action — wired to the `Run` button and
 * the `⌘↵` keybinding. Reads the store synchronously, flips the active
 * tab's result to a `running` marker, awaits the database, and stores
 * the outcome.
 *
 * Returns `null` if there's nothing to run (no active tab or empty body)
 * so callers can decide whether to play a toast.
 */

import { useStore } from "@/lib/state/store";
import { run } from "@/lib/db/client";
import { ulid } from "@/lib/util/id";
import { appendHistoryEntry } from "@/lib/actions/historyActions";
import { getActiveTabId } from "@/lib/actions/workspaceActions";

/**
 * Heuristic — any query that touches one of these keywords is assumed
 * to mutate the graph. Used to decide whether the post-run hook should
 * trigger a DB-count refresh. False positives (e.g. a `CREATE` token
 * inside a string literal) are harmless: we just recount.
 */
const MUTATION_RE = /\b(CREATE|MERGE|DELETE|SET|REMOVE)\b/i;

/** DOM event name listened to by `useDbStatus` for count refreshes. */
export const LORADB_MUTATION_EVENT = "loradb:mutation";

export async function runActiveTab(): Promise<string | null> {
  const state = useStore.getState();
  // Derived from the workspace tree — see resolveActiveTabId in tree.ts.
  const tabId = getActiveTabId();
  if (tabId === null) return null;
  const tab = state.tabs.find((t) => t.id === tabId);
  if (!tab) return null;
  const body = tab.body;
  if (body.trim().length === 0) return null;

  const runId = ulid();
  const startedAt = Date.now();
  // Set running BEFORE awaiting so the UI flips state immediately.
  state.setRunning(tabId, runId, startedAt);

  const outcome = await run(body);
  // Re-read so we can decide whether this outcome is still relevant.
  // We drop the result when:
  //   - the tab no longer exists (closed mid-flight)
  //   - the user cancelled (results[tabId] cleared by ResultPane)
  //   - a newer run took over (different runId in the running marker, or
  //     a fully resolved outcome is already in place)
  // Dropping a stale result avoids the WASM call silently clobbering a
  // newer query the user has already kicked off.
  const after = useStore.getState();
  if (!after.tabs.some((t) => t.id === tabId)) return outcome.runId;
  const current = after.results[tabId];
  const stillLatest =
    current !== undefined &&
    current.state === "running" &&
    current.runId === runId;
  if (!stillLatest) return outcome.runId;
  after.setResult(tabId, outcome);

  // After a successful mutation, nudge `useDbStatus` to recount nodes
  // and relationships. We post a DOM event instead of importing the
  // hook so this action stays callable from non-React contexts.
  if (outcome.state === "ok" && MUTATION_RE.test(body) && typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent(LORADB_MUTATION_EVENT));
  }

  // Fire-and-forget history append; IDB failures are non-fatal for
  // the workbench UX.
  void appendHistoryEntry({
    tabId,
    body,
    startedAt: outcome.startedAt,
    ms: outcome.ms,
    rowCount: outcome.state === "ok" ? outcome.result.stats.rowCount : 0,
    ok: outcome.state === "ok",
    ...(outcome.state === "error" ? { errorMessage: outcome.message } : {}),
  }).catch(() => {});

  return outcome.runId;
}
