"use client";

/**
 * Auto-restore actions.
 *
 * Glues the WASM database snapshot APIs to an IndexedDB-backed
 * single-slot store so the workbench can rehydrate after a page reload.
 *
 * - `bootAutoRestore` runs once on mount; if a saved blob exists and
 *   the user hasn't opted out, the DB is rehydrated and the
 *   schema/counts pipeline is nudged via the existing mutation event.
 * - `startAutoSaveLoop` listens for `loradb:mutation` and persists the
 *   serialized DB after a 2s idle. It also flushes eagerly on
 *   `visibilitychange=hidden` (and best-effort on `beforeunload`). Both
 *   `saveSnapshot` and the IDB write are async, so the unload flush is
 *   genuinely fire-and-forget — the recovery guarantee really comes
 *   from the visibility flush, which fires while the tab is still
 *   alive enough to await the write.
 */

import {
  loadSnapshot,
  nodeCount,
  relationshipCount,
  saveSnapshot,
} from "@/lib/db/client";
import { LORADB_MUTATION_EVENT } from "@/lib/actions/runActiveTab";
import {
  AUTO_SNAPSHOT_CAP_BYTES,
  readAuto,
  writeAuto,
} from "@/lib/persistence/autoSnapshot";
import { useStore } from "@/lib/state/store";
import { debounce } from "@/lib/util/async";
import { notifications } from "@mantine/notifications";

const SAVE_DEBOUNCE_MS = 2000;

let booted = false;
// Latched once the auto-save backing store rejects a write (e.g. the blob
// exceeds our soft cap, or the browser is out of storage quota). We
// surface a single notification and stop scheduling further saves until
// the page reloads — repeatedly toasting on every mutation would be a
// worse UX than failing loudly once.
let autoSavePaused = false;
// Mirror guard for the exception path (saveSnapshot itself throws, vs.
// writeAuto returning a non-ok result). One-shot to avoid toast spam on a
// flaky WASM backend.
let autoSaveErrorToasted = false;

function formatBytes(n: number): string {
  if (n >= 1024 * 1024 * 1024)
    return `${(n / 1024 / 1024 / 1024).toFixed(1)} GB`;
  if (n >= 1024 * 1024) return `${Math.round(n / 1024 / 1024)} MB`;
  if (n >= 1024) return `${Math.round(n / 1024)} KB`;
  return `${n} B`;
}

export async function bootAutoRestore(): Promise<void> {
  // React StrictMode double-fires effects in dev; guard so we only
  // touch the DB once per page load.
  if (booted) return;
  booted = true;

  if (typeof window === "undefined") return;
  const { autoRestore } = useStore.getState();
  if (!autoRestore) return;

  let blob: Uint8Array | null;
  try {
    blob = await readAuto();
  } catch (err) {
    // The auto-snapshot exists but couldn't be read — most likely a
    // partial write from a previous tab crash, or IDB access blocked.
    // Toast so the user knows their previous session won't restore.
    console.warn("bootAutoRestore: readAuto failed", err);
    notifications.show({
      color: "yellow",
      title: "Auto-save couldn't be read",
      message:
        "The stored snapshot was corrupted and will be overwritten on next save. Starting with an empty database.",
      autoClose: 8000,
    });
    return;
  }
  if (!blob) return;

  try {
    await loadSnapshot(blob);
    const [n, r] = await Promise.all([nodeCount(), relationshipCount()]);
    window.dispatchEvent(new CustomEvent(LORADB_MUTATION_EVENT));
    notifications.show({
      color: "blue",
      title: "Session restored",
      message: `Restored ${n} node${n === 1 ? "" : "s"}, ${r} rel${r === 1 ? "" : "s"} from auto-save`,
    });
  } catch (err) {
    console.warn("bootAutoRestore: failed to load snapshot", err);
    notifications.show({
      color: "yellow",
      title: "Couldn't restore previous session",
      message:
        "The auto-saved snapshot couldn't be loaded. Starting with an empty database.",
      autoClose: 8000,
    });
  }
}

export async function saveAutoSnapshotNow(): Promise<void> {
  if (typeof window === "undefined") return;
  if (autoSavePaused) return;
  try {
    const blob = await saveSnapshot();
    const result = await writeAuto(blob);
    if (!result.ok && !autoSavePaused) {
      autoSavePaused = true;
      const message =
        result.reason === "too-large"
          ? `This graph (${formatBytes(result.size)}) exceeds the ${formatBytes(result.cap)} auto-save cap. Use Snapshots → New to save manually.`
          : result.reason === "quota-exceeded"
            ? "Browser storage is full. Free up space (or remove unused snapshots) and reload to resume auto-save."
            : `Auto-save is unavailable (cap ${formatBytes(AUTO_SNAPSHOT_CAP_BYTES)}). Use Snapshots → New to save manually.`;
      notifications.show({
        color: "yellow",
        title: "Auto-save paused",
        message,
        autoClose: 8000,
      });
    }
  } catch (err) {
    console.warn("saveAutoSnapshotNow failed", err);
    if (!autoSaveErrorToasted) {
      autoSaveErrorToasted = true;
      notifications.show({
        color: "yellow",
        title: "Auto-save failed",
        message:
          err instanceof Error
            ? `${err.message}. Subsequent failures will be silent — check the console.`
            : "Auto-save couldn't serialize the database. Use Snapshots → New to save manually.",
        autoClose: 8000,
      });
    }
  }
}

export function startAutoSaveLoop(): () => void {
  if (typeof window === "undefined") return () => {};

  const debounced = debounce(() => {
    void saveAutoSnapshotNow();
  }, SAVE_DEBOUNCE_MS);

  const onMutation = () => {
    if (!useStore.getState().autoRestore) return;
    debounced();
  };

  const flush = () => {
    if (!useStore.getState().autoRestore) return;
    // Fire-and-forget: the localStorage write inside is synchronous,
    // so the most recent in-memory bytes will reach disk before the
    // unload completes — provided saveSnapshot resolves quickly,
    // which it does for typical playground sizes.
    debounced.cancel();
    void saveAutoSnapshotNow();
  };

  const onVisibility = () => {
    if (document.visibilityState === "hidden") {
      flush();
    }
  };

  window.addEventListener(LORADB_MUTATION_EVENT, onMutation);
  window.addEventListener("beforeunload", flush);
  document.addEventListener("visibilitychange", onVisibility);

  return () => {
    debounced.cancel();
    window.removeEventListener(LORADB_MUTATION_EVENT, onMutation);
    window.removeEventListener("beforeunload", flush);
    document.removeEventListener("visibilitychange", onVisibility);
  };
}
