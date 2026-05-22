/**
 * Rolling-window throughput tracker for long-running streamed flows.
 *
 * Records `(timestampMs, bytesProcessed, rowsProcessed)` samples and
 * computes recent rates over the last `windowMs` of observations.
 * Cheap and synchronous — drop a sample after every progress event,
 * read the rates whenever the UI repaints.
 *
 * Pure utility — no React, no LoraDB. Used by the import wizard's
 * Running step today; export progress and any other "ship N bytes
 * over the worker boundary" flow can re-use it.
 */

export interface ThroughputSample {
  /** `Date.now()` at the moment the sample was recorded. */
  timestampMs: number;
  /** Cumulative bytes processed by the producer at that moment. */
  bytes: number;
  /** Cumulative rows committed by the producer at that moment. */
  rows: number;
}

export interface ThroughputReading {
  /** Bytes per second over the most recent `windowMs`. */
  bytesPerSecond: number;
  /** Rows per second over the most recent `windowMs`. */
  rowsPerSecond: number;
  /**
   * Estimated seconds remaining to finish, given the most recent
   * bytes-rate and the supplied `totalBytes`. `null` when we can't
   * estimate (zero rate, unknown total, finished).
   */
  etaSeconds: number | null;
  /** Wall-clock duration since the first sample, in seconds. */
  elapsedSeconds: number;
}

export class ThroughputTracker {
  private samples: ThroughputSample[] = [];
  private readonly windowMs: number;

  constructor(windowMs: number = 2_000) {
    this.windowMs = windowMs;
  }

  /** Drop the oldest samples outside the rolling window. Internal. */
  private prune(now: number): void {
    // Keep at least two samples so we can always compute a rate.
    const cutoff = now - this.windowMs;
    let firstKeep = 0;
    while (
      firstKeep < this.samples.length - 1 &&
      this.samples[firstKeep + 1]!.timestampMs < cutoff
    ) {
      firstKeep += 1;
    }
    if (firstKeep > 0) this.samples.splice(0, firstKeep);
  }

  /** Record a new cumulative-counts observation. */
  record(bytes: number, rows: number, now: number = Date.now()): void {
    this.samples.push({ timestampMs: now, bytes, rows });
    this.prune(now);
  }

  /** Reset the window — call when starting a new run. */
  reset(): void {
    this.samples = [];
  }

  /**
   * Compute rates + ETA. Returns `null` if fewer than two samples
   * have been recorded — the caller should treat that as "warming
   * up; show no numbers yet."
   */
  read(
    totalBytes?: number,
    now: number = Date.now(),
  ): ThroughputReading | null {
    if (this.samples.length < 2) return null;
    const first = this.samples[0]!;
    const last = this.samples[this.samples.length - 1]!;
    const dtMs = last.timestampMs - first.timestampMs;
    if (dtMs <= 0) return null;
    const dtSec = dtMs / 1000;
    const bytesPerSecond = (last.bytes - first.bytes) / dtSec;
    const rowsPerSecond = (last.rows - first.rows) / dtSec;
    const elapsedSeconds = (now - first.timestampMs) / 1000;
    let etaSeconds: number | null = null;
    if (
      typeof totalBytes === "number" &&
      totalBytes > 0 &&
      bytesPerSecond > 0 &&
      last.bytes < totalBytes
    ) {
      etaSeconds = (totalBytes - last.bytes) / bytesPerSecond;
    }
    return { bytesPerSecond, rowsPerSecond, etaSeconds, elapsedSeconds };
  }
}

/** Format a bytes-per-second reading. */
export function formatRate(bytesPerSecond: number): string {
  if (bytesPerSecond < 1024) return `${bytesPerSecond.toFixed(0)} B/s`;
  if (bytesPerSecond < 1024 * 1024) {
    return `${(bytesPerSecond / 1024).toFixed(1)} KB/s`;
  }
  return `${(bytesPerSecond / 1024 / 1024).toFixed(2)} MB/s`;
}

/** Format a rows-per-second reading with thousands separators. */
export function formatRowsPerSecond(rps: number): string {
  if (rps < 100) return `${rps.toFixed(1)} rows/s`;
  return `${Math.round(rps).toLocaleString()} rows/s`;
}

/** Format a duration in seconds as a compact human string. */
export function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "—";
  if (seconds < 1) return "<1s";
  if (seconds < 60) return `${Math.round(seconds)}s`;
  const m = Math.floor(seconds / 60);
  const s = Math.round(seconds % 60);
  if (m < 60) return `${m}m ${s}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}
