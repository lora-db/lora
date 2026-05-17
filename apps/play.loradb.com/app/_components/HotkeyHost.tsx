"use client";

/**
 * Mounts the playground's hotkey map. The host renders nothing; it
 * exists purely so `useHotkeys` runs alongside the workbench tree.
 *
 * The `mod+K` Spotlight opener is registered with a hand-rolled
 * document-level `keydown` listener instead of `useHotkeys` because
 * Mantine's `useHotkeys` deliberately bails out when focus sits inside
 * a `contentEditable` host — which CodeMirror is. That would shadow
 * Spotlight whenever the user is in the editor (i.e. ~always).
 */

import { useEffect, useMemo } from "react";
import {
  useComputedColorScheme,
  useMantineColorScheme,
} from "@mantine/core";
import { useHotkeys } from "@mantine/hooks";
import { spotlight } from "@mantine/spotlight";

import { buildHotkeys } from "@/lib/hotkeys/bindings";

/**
 * `keydown` matcher for `mod+K` that works regardless of `event.target`.
 * `event.metaKey` covers macOS, `event.ctrlKey` covers Windows/Linux —
 * "mod" in Mantine parlance.
 */
function isModK(event: KeyboardEvent): boolean {
  if (event.altKey || event.shiftKey) return false;
  if (!(event.metaKey || event.ctrlKey)) return false;
  // `event.key` is "k"/"K" depending on shift state — but we already
  // bailed on shift above. Normalise just in case a future browser
  // returns the uppercase form.
  return event.key === "k" || event.key === "K";
}

export function HotkeyHost() {
  const { colorScheme, setColorScheme } = useMantineColorScheme();
  const computed = useComputedColorScheme("dark", {
    getInitialValueInEffect: false,
  });

  const entries = useMemo(
    () =>
      buildHotkeys({
        currentColorScheme: colorScheme,
        setColorScheme,
        computedColorScheme: computed,
      }),
    [colorScheme, setColorScheme, computed],
  );

  useHotkeys(entries.map(([k, fn]) => [k, fn]));

  // mod+K → Spotlight. Bypasses `useHotkeys`' contentEditable guard so
  // ⌘K still works when the CodeMirror editor has focus. Uses
  // `capture: true` to fire ahead of any CodeMirror keymap that might
  // claim the event for editor commands.
  useEffect(() => {
    if (typeof window === "undefined") return;
    const handler = (event: KeyboardEvent): void => {
      if (!isModK(event)) return;
      event.preventDefault();
      event.stopPropagation();
      spotlight.open();
    };
    document.addEventListener("keydown", handler, { capture: true });
    return () => {
      document.removeEventListener("keydown", handler, { capture: true });
    };
  }, []);

  return null;
}
