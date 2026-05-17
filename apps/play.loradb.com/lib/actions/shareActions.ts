"use client";

/**
 * Share-link helpers. The Share dialog and saved-query "Copy link"
 * menu item delegate here so the URL shape and copy-failure UX are
 * defined in exactly one place.
 */

import { notifications } from "@mantine/notifications";

import { makeShareHash } from "@/lib/share/encode";

/**
 * Build a full share URL for the given query body. Returns an empty
 * string on the server — the playground only ever needs share links
 * client-side, but TS-strict callers still expect a string back.
 */
export function buildShareLink(body: string): string {
  if (typeof window === "undefined") return "";
  return `${window.location.origin}${window.location.pathname}${makeShareHash(body)}`;
}

/**
 * Copy a share link for `body` to the clipboard and surface a Mantine
 * notification with the result. Throws on transport failures so the
 * Share dialog can keep its button in a sensible state.
 */
export async function copyShareLink(body: string): Promise<void> {
  if (typeof window === "undefined") {
    throw new Error("copyShareLink is only available in the browser");
  }
  if (!navigator.clipboard) {
    notifications.show({
      color: "red",
      title: "Clipboard unavailable",
      message: "Your browser does not expose a clipboard API.",
    });
    throw new Error("Clipboard API unavailable");
  }
  const url = buildShareLink(body);
  try {
    await navigator.clipboard.writeText(url);
    notifications.show({
      color: "green",
      title: "Link copied",
      message: "Share link copied to clipboard.",
    });
  } catch (err) {
    notifications.show({
      color: "red",
      title: "Copy failed",
      message: err instanceof Error ? err.message : String(err),
    });
    throw err;
  }
}
