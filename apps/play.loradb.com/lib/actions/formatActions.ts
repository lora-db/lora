"use client";

/**
 * Format the active editor tab's body via `@loradb/lora-query`'s
 * `format` helper (WASM-backed pretty-printer). Async — even though
 * the underlying pest call is synchronous, the JS facade wraps it in
 * a Promise so consumers see a uniform contract with the other parser
 * helpers.
 *
 * Errors are tolerated: the underlying `format` returns the original
 * source when the input does not parse, so we treat any thrown error
 * as a notification and leave the buffer untouched.
 */

import { notifications } from "@mantine/notifications";

import { format } from "@loradb/lora-query";

import { useStore } from "@/lib/state/store";

export async function formatActiveTab(): Promise<void> {
  const state = useStore.getState();
  const tabId = state.activeTabId;
  if (tabId === null) return;
  const tab = state.tabs.find((t) => t.id === tabId);
  if (!tab) return;
  const body = tab.body;
  if (body.length === 0) return;

  let output: string;
  try {
    // `format` returns the formatted source string. The WASM layer
    // falls back to the original on parse error, so the only way
    // we end up here is a true runtime fault.
    output = await format(body);
  } catch (err) {
    notifications.show({
      color: "red",
      title: "Format failed",
      message: err instanceof Error ? err.message : String(err),
    });
    return;
  }

  if (output === body) {
    // Nothing changed — skip the dirty flag flip and the toast.
    return;
  }

  useStore.getState().setBody(tabId, output);
  notifications.show({
    color: "green",
    title: "Formatted",
    message: "Active tab body reformatted.",
    autoClose: 1500,
  });
}
