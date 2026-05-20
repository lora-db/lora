"use client";

/**
 * Insert Cypher snippets into the editor.
 *
 * Used by the Schema browser (click-to-query), context menus, and the
 * Spotlight command palette. Behavior is driven by `mode` so callers
 * can express intent without having to peek at the active tab:
 *
 *  - `"smart"` (default): replace the active tab body when it's
 *    empty/whitespace, otherwise open a new tab. The path most clicks
 *    take — does the right thing without surprising the user.
 *  - `"replace"`: overwrite the active tab body unconditionally.
 *  - `"append"`: append to the active tab body (separated by a blank
 *    line). Mirrors what shift-click does in most editors.
 *  - `"new-tab"`: always open a new tab.
 *
 * The `name` field, when present, is used as the new-tab title so
 * users can spot it in the strip without re-reading the body.
 */

import { useStore } from "@/lib/state/store";
import { useActiveTabId } from "@/lib/state/selectors";
import { openTabInCell } from "@/lib/actions/tabActions";
import { resolveActiveTabId } from "@/lib/state/workspace/tree";

export type InsertMode = "smart" | "replace" | "append" | "new-tab";

export interface InsertSnippetOptions {
  /** How to place the snippet (default `"smart"`). */
  mode?: InsertMode;
  /** Optional tab title used when a new tab is created. */
  name?: string;
}

function isBlank(text: string): boolean {
  return text.trim().length === 0;
}

function activeTabId(): string | null {
  const s = useStore.getState();
  return resolveActiveTabId(s.workspace, s.activePaneId);
}

/**
 * Place `snippet` into the editor per `opts.mode`. Returns the id of
 * the tab that received the snippet (existing tab for `replace`/
 * `append`/`smart`-into-empty, new tab id otherwise).
 */
export function insertSnippet(
  snippet: string,
  opts: InsertSnippetOptions = {},
): string | null {
  const mode = opts.mode ?? "smart";
  const state = useStore.getState();
  const id = activeTabId();
  const tab = id ? (state.tabs.find((t) => t.id === id) ?? null) : null;

  if (mode === "new-tab" || tab === null) {
    return openTabInCell({
      body: snippet,
      ...(opts.name !== undefined ? { name: opts.name } : {}),
      dedupe: false,
    });
  }

  if (mode === "smart") {
    if (isBlank(tab.body)) {
      state.setBody(tab.id, snippet);
      return tab.id;
    }
    return openTabInCell({
      body: snippet,
      ...(opts.name !== undefined ? { name: opts.name } : {}),
      dedupe: false,
    });
  }

  if (mode === "replace") {
    state.setBody(tab.id, snippet);
    return tab.id;
  }

  // append
  const sep = isBlank(tab.body) ? "" : "\n\n";
  state.setBody(tab.id, `${tab.body}${sep}${snippet}`);
  return tab.id;
}

/**
 * Translate keyboard modifiers from a mouse event into an
 * {@link InsertMode}. Used by schema-row click handlers so users can
 * pick a non-default placement with shift / alt without us having to
 * sprout per-row buttons.
 *
 *  - Alt / Option → always new tab
 *  - Shift → append at end of current tab
 *  - Plain click → smart
 */
export function modeFromEvent(e: {
  altKey: boolean;
  shiftKey: boolean;
}): InsertMode {
  if (e.altKey) return "new-tab";
  if (e.shiftKey) return "append";
  return "smart";
}

// Re-export the selector so non-component callers can pull the active id without
// reaching into the store internals. Cheap convenience; no behavioural change.
export { useActiveTabId };
