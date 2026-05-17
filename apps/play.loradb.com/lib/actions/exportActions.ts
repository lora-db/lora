"use client";

/**
 * Phase 4b export actions.
 *
 * The Graph PNG export uses an event bridge instead of threading a ref
 * across the component tree: `GraphView` registers a listener on the
 * `GRAPH_PNG_EVENT` and calls the canvas's imperative `screenshot()`
 * method when triggered. Callers (Spotlight, the result-pane button)
 * just dispatch the event — no React refs needed.
 */

export const GRAPH_PNG_EVENT = "loradb:graph-png";

/** Dispatches a window event asking GraphView to export a PNG. */
export function requestGraphPng(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent(GRAPH_PNG_EVENT));
}

function isoDateStamp(timestamp: number): string {
  return new Date(timestamp).toISOString().slice(0, 10);
}

/** Triggers a browser download for the given PNG blob. */
export function downloadGraphPng(blob: Blob): void {
  if (typeof window === "undefined") return;
  const url = URL.createObjectURL(blob);
  try {
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = `loradb-graph-${isoDateStamp(Date.now())}.png`;
    anchor.style.display = "none";
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
  } finally {
    setTimeout(() => {
      URL.revokeObjectURL(url);
    }, 1000);
  }
}
