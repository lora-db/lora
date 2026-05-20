"use client";

/**
 * Imperative "run the active tab" action — wired to the `Run` button and
 * the `⌘↵` keybinding. Reads the store synchronously, flips the active
 * tab's result to a `running` marker, awaits the database, and stores
 * the outcome.
 *
 * Returns `null` if there's nothing to run (no active tab or empty body)
 * so callers can decide whether to play a toast.
 *
 * Parameter binding: at run time we parse `tab.params` (raw JSON
 * source) and compare against the analyser's detected `$param` list
 * (mirrored into the `paramsByTab` slice by `EditorPane`). On parse
 * failure or missing-required, surface a confirm-style toast so the
 * user can choose to run anyway or cancel. Power users keep their
 * fast path; cautious users get a guard.
 */

import { notifications } from "@mantine/notifications";

import type { LoraParams } from "@loradb/lora-wasm";
import { format as formatQuery, validate as validateQuery } from "@loradb/lora-query";

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

interface RunOpts {
  /**
   * When true, bypass the params-validation toast — the user has
   * already acknowledged the issue via "Run anyway". The action
   * itself still passes whatever parsed cleanly to the driver.
   */
  force?: boolean;
}

/**
 * Parse `tab.params` and validate it against the analyser-detected
 * `$param` list. Returns:
 *   - `{ ok: true, params }` when everything checks out
 *   - `{ ok: false, kind: "invalid", message }` when JSON.parse threw
 *   - `{ ok: false, kind: "non-object" }` when params is not an object
 *   - `{ ok: false, kind: "missing", missing }` when required keys absent
 */
type ParamsCheck =
  | { ok: true; params: LoraParams | undefined }
  | { ok: false; kind: "invalid"; message: string }
  | { ok: false; kind: "non-object" }
  | { ok: false; kind: "missing"; missing: string[]; params: LoraParams };

function inspectParams(
  source: string,
  detected: readonly string[],
): ParamsCheck {
  const trimmed = source.trim();
  if (trimmed.length === 0 || trimmed === "{}") {
    if (detected.length === 0) return { ok: true, params: undefined };
    return { ok: false, kind: "missing", missing: [...detected], params: {} };
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch (err) {
    return {
      ok: false,
      kind: "invalid",
      message: err instanceof Error ? err.message : String(err),
    };
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    return { ok: false, kind: "non-object" };
  }
  const params = parsed as LoraParams;
  const missing = detected.filter((name) => !(name in params));
  if (missing.length > 0) {
    return { ok: false, kind: "missing", missing, params };
  }
  return { ok: true, params };
}

/**
 * Show a Mantine notification that includes a "Run anyway" action.
 * Returns a cleanup so the caller can clear the notification when
 * the next run starts.
 */
function showParamsGate(opts: {
  title: string;
  message: string;
  color?: string;
  onConfirm: () => void;
}) {
  const id = `params-gate-${ulid()}`;
  notifications.show({
    id,
    color: opts.color ?? "yellow",
    title: opts.title,
    message: opts.message,
    autoClose: 6000,
    withCloseButton: true,
    onClick: () => {
      notifications.hide(id);
      opts.onConfirm();
    },
  });
}

export async function runActiveTab(
  opts: RunOpts = {},
): Promise<string | null> {
  const state = useStore.getState();
  const tabId = getActiveTabId();
  if (tabId === null) return null;
  const tab = state.tabs.find((t) => t.id === tabId);
  if (!tab) return null;
  let body = tab.body;
  if (body.trim().length === 0) return null;

  // Auto-format on run — only when the source parses cleanly so we
  // never silently rewrite a buffer the user is mid-edit on. Errors
  // from the formatter itself are swallowed: a failed prettify must
  // never block a run.
  if (state.autoFormatOnRun) {
    try {
      const diagnostics = await validateQuery(body);
      if (diagnostics.length === 0) {
        const formatted = await formatQuery(body);
        if (formatted !== body) {
          state.setBody(tabId, formatted);
          body = formatted;
        }
      }
    } catch {
      // ignore — fall through with the original body
    }
  }

  // Validate the params payload unless the caller has explicitly
  // opted in to bypassing the gate.
  const detected = state.paramsByTab[tabId] ?? [];
  let params: LoraParams | undefined;
  if (!opts.force) {
    const check = inspectParams(tab.params ?? "{}", detected);
    if (check.ok) {
      params = check.params;
    } else if (check.kind === "invalid") {
      showParamsGate({
        title: "Params payload isn't valid JSON",
        message: `${check.message}. Click to run with no params.`,
        color: "red",
        onConfirm: () => {
          void runActiveTab({ force: true });
        },
      });
      return null;
    } else if (check.kind === "non-object") {
      showParamsGate({
        title: "Params payload must be a JSON object",
        message: "Top-level value should be a `{}`. Click to run with no params.",
        color: "red",
        onConfirm: () => {
          void runActiveTab({ force: true });
        },
      });
      return null;
    } else {
      // missing keys
      const list = check.missing.map((n) => `$${n}`).join(", ");
      showParamsGate({
        title:
          check.missing.length === 1
            ? "1 required parameter missing"
            : `${check.missing.length} required parameters missing`,
        message: `Click to run anyway. Missing: ${list}.`,
        onConfirm: () => {
          void runActiveTab({ force: true });
        },
      });
      return null;
    }
  } else {
    // Force path — best-effort parse, ignore failures.
    const check = inspectParams(tab.params ?? "{}", detected);
    if (check.ok) {
      params = check.params;
    } else if (check.kind === "missing") {
      params = check.params;
    }
    // For "invalid" / "non-object" → params stays undefined.
  }

  const runId = ulid();
  const startedAt = Date.now();
  state.setRunning(tabId, runId, startedAt);

  const outcome = await run(body, params);
  const after = useStore.getState();
  if (!after.tabs.some((t) => t.id === tabId)) return outcome.runId;
  const current = after.results[tabId];
  const stillLatest =
    current !== undefined &&
    current.state === "running" &&
    current.runId === runId;
  if (!stillLatest) return outcome.runId;
  after.setResult(tabId, outcome);

  if (outcome.state === "ok" && MUTATION_RE.test(body) && typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent(LORADB_MUTATION_EVENT));
  }

  // Persist the params source verbatim so the user can replay this
  // exact binding set later.
  void appendHistoryEntry({
    tabId,
    body,
    params: tab.params,
    startedAt: outcome.startedAt,
    ms: outcome.ms,
    rowCount: outcome.state === "ok" ? outcome.result.stats.rowCount : 0,
    ok: outcome.state === "ok",
    ...(outcome.state === "error" ? { errorMessage: outcome.message } : {}),
  }).catch(() => {});

  return outcome.runId;
}
