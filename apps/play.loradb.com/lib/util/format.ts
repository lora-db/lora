/**
 * Small display formatters for durations, byte sizes and integer counts.
 *
 * These intentionally avoid `Intl.NumberFormat` locale dependence except
 * for thousands separators — playground UI surfaces should look identical
 * across environments and locales.
 */

/** Formats a duration in milliseconds as `"123ms"`, `"1.23s"` or `"1m 23s"`. */
export function formatMs(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "0ms";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) {
    const seconds = ms / 1000;
    // Two decimal places, but trim trailing zeros for cleaner output.
    return `${seconds.toFixed(2).replace(/\.?0+$/, "")}s`;
  }
  const totalSeconds = Math.floor(ms / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes}m ${seconds}s`;
}

/** Formats a non-negative byte count as `"512 B"`, `"4.2 KB"`, `"1.3 MB"` etc. */
export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "0 B";
  if (n < 1024) return `${Math.round(n)} B`;
  const units = ["KB", "MB", "GB", "TB", "PB"] as const;
  let value = n / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  const unit = units[unitIndex] ?? "KB";
  // 1 decimal place; drop trailing ".0" for tidy display.
  const formatted = value.toFixed(1).replace(/\.0$/, "");
  return `${formatted} ${unit}`;
}

/** Formats an integer with thousands separators (e.g. `"1,234"`). */
export function formatCount(n: number): string {
  if (!Number.isFinite(n)) return "0";
  const rounded = Math.trunc(n);
  // `toLocaleString("en-US")` keeps the comma separator stable across locales.
  return rounded.toLocaleString("en-US");
}
