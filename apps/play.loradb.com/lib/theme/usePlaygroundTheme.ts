"use client";

/**
 * `usePlaygroundTheme` — the single hook every playground surface
 * pulls its theme from. It reads the live Mantine theme + computed
 * colour scheme, recovers our token set, and derives the editor /
 * canvas / grid themes on the fly.
 *
 * If the resolved scheme has flipped away from the one the runtime
 * Mantine theme was built with, we fall back to `tokensFor(scheme)`
 * so token-driven colours stay in lock-step with what Mantine is
 * actually painting on the page.
 */

import { useCallback, useMemo } from "react";
import {
  useComputedColorScheme,
  useMantineColorScheme,
  useMantineTheme,
  type MantineTheme,
} from "@mantine/core";

import { deriveCanvasTheme } from "./canvas";
import { deriveEditorTheme } from "./editor";
import { deriveGridTheme } from "./grid";
import { darkTokens, lightTokens, tokensFor, type Tokens } from "./tokens";

type ResolvedScheme = "light" | "dark";

interface PlaygroundTheme {
  tokens: Tokens;
  scheme: ResolvedScheme;
  mantine: MantineTheme;
  canvas: ReturnType<typeof deriveCanvasTheme>;
  editor: ReturnType<typeof deriveEditorTheme>;
  grid: ReturnType<typeof deriveGridTheme>;
}

/** Was this Mantine theme built from the dark or light token set? */
function detectBuiltScheme(theme: MantineTheme): ResolvedScheme | null {
  const other = theme.other as { tokens?: Tokens } | undefined;
  const built = other?.tokens;
  if (!built) return null;
  if (built.bg.editor === darkTokens.bg.editor) return "dark";
  if (built.bg.editor === lightTokens.bg.editor) return "light";
  return null;
}

export function usePlaygroundTheme(): PlaygroundTheme {
  const mantine = useMantineTheme();
  const scheme = useComputedColorScheme("dark", {
    getInitialValueInEffect: false,
  });

  const tokens = useMemo<Tokens>(() => {
    const other = mantine.other as { tokens?: Tokens } | undefined;
    const built = other?.tokens;
    const builtScheme = detectBuiltScheme(mantine);
    // If Mantine flipped scheme but the runtime theme still carries
    // the original token set, swap to the matching tokens so the four
    // surfaces stay aligned with what's actually on screen.
    if (built && builtScheme === scheme) return built;
    return tokensFor(scheme);
  }, [mantine, scheme]);

  return useMemo<PlaygroundTheme>(
    () => ({
      tokens,
      scheme,
      mantine,
      canvas: deriveCanvasTheme(tokens),
      editor: deriveEditorTheme(tokens),
      grid: deriveGridTheme(tokens),
    }),
    [tokens, scheme, mantine],
  );
}

/**
 * Returns a stable callback that flips the Mantine colour scheme
 * between light and dark. `auto` resolves to whatever the computed
 * scheme reports right now before flipping.
 */
export function useColorSchemeToggle(): () => void {
  const { colorScheme, setColorScheme } = useMantineColorScheme();
  const computed = useComputedColorScheme("dark", {
    getInitialValueInEffect: false,
  });

  return useCallback(() => {
    const current = colorScheme === "auto" ? computed : colorScheme;
    setColorScheme(current === "dark" ? "light" : "dark");
  }, [colorScheme, computed, setColorScheme]);
}
