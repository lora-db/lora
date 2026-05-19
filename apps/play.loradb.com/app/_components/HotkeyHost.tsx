"use client";

/**
 * Mounts the playground's hotkey map. The host renders nothing; it
 * exists purely so `useHotkeys` runs alongside the workbench tree.
 *
 * A subset of chords (Spotlight, Save, Reopen-closed-tab, Duplicate)
 * are registered with hand-rolled, document-level `keydown` listeners
 * in the **capture** phase rather than through `useHotkeys`. Reasons:
 *
 *  - Mantine's `useHotkeys` deliberately bails when focus sits inside a
 *    `contentEditable` host — which CodeMirror is. Bare `useHotkeys`
 *    chords would shadow whenever the editor has focus (≈ always).
 *  - The browser claims `mod+S` for "Save Page As"; we need to
 *    `preventDefault` ahead of every other listener.
 *  - CodeMirror's own keymap can claim chords like `mod+D` for editor
 *    commands; firing in capture phase ensures the workbench handler
 *    wins.
 */

import { useEffect, useMemo } from "react";
import {
  useComputedColorScheme,
  useMantineColorScheme,
} from "@mantine/core";
import { useHotkeys } from "@mantine/hooks";
import { spotlight } from "@mantine/spotlight";

import { buildHotkeys } from "@/lib/hotkeys/bindings";
import { saveOrPromptActiveTab } from "@/lib/actions/savedQueryActions";
import { reopenLastClosedTab } from "@/lib/actions/tabActions";

/**
 * Cross-platform matcher for `mod+<key>` chords with optional modifiers.
 * `event.metaKey` covers macOS, `event.ctrlKey` covers Windows/Linux —
 * "mod" in Mantine parlance.
 *
 * `key` is matched case-insensitively against `event.key`. For chords
 * involving `shift` we expect the caller to pass the unshifted base
 * (`"t"` for `mod+shift+t`) — browsers report `event.key` as the
 * shifted form (`"T"`) which we normalise here.
 */
function matchModChord(
  event: KeyboardEvent,
  key: string,
  opts: { shift?: boolean; alt?: boolean } = {},
): boolean {
  const wantShift = opts.shift ?? false;
  const wantAlt = opts.alt ?? false;
  if (event.altKey !== wantAlt) return false;
  if (event.shiftKey !== wantShift) return false;
  if (!(event.metaKey || event.ctrlKey)) return false;
  return event.key.toLowerCase() === key.toLowerCase();
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

  // Capture-phase chords. Each entry runs ahead of CodeMirror and
  // browser-default handlers; we `preventDefault` + `stopPropagation`
  // unconditionally on a match so neither layer fights the workbench.
  //
  // Note: we deliberately *don't* register `mod+T` or `mod+shift+T`
  // here. Chromium-family and Safari browsers reserve those for tab
  // management and refuse `preventDefault` from page JS, so they would
  // open/reopen a browser tab instead of running our handler.
  useEffect(() => {
    if (typeof window === "undefined") return;
    const handler = (event: KeyboardEvent): void => {
      // mod+K / mod+P → Spotlight (mod+P preventDefault overrides
      // the browser's Print shortcut on the way through).
      if (
        matchModChord(event, "k") ||
        matchModChord(event, "p")
      ) {
        event.preventDefault();
        event.stopPropagation();
        spotlight.open();
        return;
      }
      // mod+shift+S → Save As… (check shift variant first; the bare
      // mod+S check below would otherwise short-circuit on it).
      if (matchModChord(event, "s", { shift: true })) {
        event.preventDefault();
        event.stopPropagation();
        void saveOrPromptActiveTab({ forceAs: true });
        return;
      }
      // mod+S → Save (in place or via dialog)
      if (matchModChord(event, "s")) {
        event.preventDefault();
        event.stopPropagation();
        void saveOrPromptActiveTab();
        return;
      }
      // mod+alt+T → Reopen last closed tab. We pick alt over shift
      // because mod+shift+T is reserved by browsers for "reopen
      // browser tab" and can't be preventDefault-ed reliably.
      if (matchModChord(event, "t", { alt: true })) {
        event.preventDefault();
        event.stopPropagation();
        reopenLastClosedTab();
        return;
      }
    };
    document.addEventListener("keydown", handler, { capture: true });
    return () => {
      document.removeEventListener("keydown", handler, { capture: true });
    };
  }, []);

  return null;
}
