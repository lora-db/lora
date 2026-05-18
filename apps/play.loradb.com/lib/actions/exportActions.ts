"use client";

/**
 * Phase 4b export actions.
 *
 * The Graph PNG export uses an event bridge instead of threading a ref
 * across the component tree: `GraphView` instances register a listener
 * on `GRAPH_PNG_EVENT` and call their canvas's imperative `screenshot()`
 * method when triggered. Callers (Spotlight, the result-pane button)
 * dispatch the event — no React refs needed.
 *
 * Multi-pane note: when several GraphView instances are mounted, the
 * event payload carries a `paneId`. Listeners only react when the id
 * matches their containing leaf (or when the id is `null`, in which
 * case the active pane's GraphView wins).
 */

export const GRAPH_PNG_EVENT = "loradb:graph-png";

export interface GraphPngEventDetail {
  /** Leaf id of the pane whose graph should be exported. `null` = active pane. */
  paneId: string | null;
}

export function isGraphPngEvent(event: Event): event is CustomEvent<GraphPngEventDetail> {
  return event instanceof CustomEvent && event.type === GRAPH_PNG_EVENT;
}

/**
 * Dispatches a window event asking GraphView to export a PNG. Defaults
 * to the active pane when no `paneId` is supplied.
 */
export function requestGraphPng(paneId?: string): void {
  if (typeof window === "undefined") return;
  const detail: GraphPngEventDetail = { paneId: paneId ?? null };
  window.dispatchEvent(new CustomEvent<GraphPngEventDetail>(GRAPH_PNG_EVENT, { detail }));
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
