"use client";

/**
 * Auto-restore snapshot storage.
 *
 * Phase 4b stashes a single serialized DB blob in `localStorage` so the
 * workbench can rehydrate after a page reload. We deliberately bypass the
 * `snapshots` IDB store here so the auto-snapshot stays invisible to the
 * Snapshots panel (which lists user-named snapshots only). The trade-off
 * is the ~5MB localStorage cap; we cap writes at 4MB and silently skip
 * larger DBs with a console warning.
 */

const KEY = "loradb-play.autosnap.v1";
const MAX_BYTES = 4 * 1024 * 1024;
// Chunk size for base64 encoding the byte array. String.fromCharCode with
// a single huge array can blow the call stack; this keeps the per-call
// argument count well below typical engine limits.
const CHUNK = 0x8000;

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += CHUNK) {
    const slice = bytes.subarray(i, i + CHUNK);
    binary += String.fromCharCode(...slice);
  }
  return btoa(binary);
}

function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    out[i] = binary.charCodeAt(i);
  }
  return out;
}

/**
 * Read the auto-snapshot from localStorage.
 *
 * Returns `null` when no entry exists. Throws when an entry exists but
 * can't be decoded (corrupted base64, partial write, etc.) so callers
 * can surface that to the user — silently returning null in the
 * corruption case would lead to confusing "session not restored"
 * behaviour with no explanation.
 */
export function readAuto(): Uint8Array | null {
  if (typeof window === "undefined") return null;
  let raw: string | null;
  try {
    raw = window.localStorage.getItem(KEY);
  } catch (err) {
    // localStorage may throw if access is blocked (Safari private mode,
    // disabled by enterprise policy). Treat as "nothing to restore" so
    // the workbench still boots; the empty-DB toast in bootAutoRestore
    // is sufficient signal.
    console.warn("readAuto: localStorage access failed", err);
    return null;
  }
  if (raw === null) return null;
  return base64ToBytes(raw);
}

/** Returns false if the blob is too large to store. */
export function writeAuto(blob: Uint8Array): boolean {
  if (typeof window === "undefined") return false;
  if (blob.byteLength > MAX_BYTES) {
    console.warn(
      `autoSnapshot: blob is ${blob.byteLength}B, exceeds the ${MAX_BYTES}B cap — skipping`,
    );
    return false;
  }
  try {
    const encoded = bytesToBase64(blob);
    window.localStorage.setItem(KEY, encoded);
    return true;
  } catch (err) {
    console.warn("writeAuto failed", err);
    return false;
  }
}

export function clearAuto(): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.removeItem(KEY);
  } catch {
    /* ignore */
  }
}
