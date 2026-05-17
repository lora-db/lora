/**
 * Mantine theme deriver. Wraps our design tokens into a
 * `MantineThemeOverride` so the rest of the app (buttons, modals,
 * inputs, ...) reads from the same source as the editor, canvas, and
 * grid.
 *
 * The token set is stashed under `theme.other.tokens` so the
 * `usePlaygroundTheme` hook can read it back without re-deriving.
 */

import { createTheme } from "@mantine/core";
import type { MantineThemeOverride } from "@mantine/core";

import { darkTokens, type Tokens } from "./tokens";
import { shadesFrom } from "./util";

/** Build a Mantine theme override from a token set. */
export function buildMantineTheme(tokens: Tokens): MantineThemeOverride {
  return createTheme({
    primaryColor: "brand",
    colors: {
      brand: shadesFrom(tokens.accent.primary),
      success: shadesFrom(tokens.accent.success),
      warning: shadesFrom(tokens.accent.warning),
      danger: shadesFrom(tokens.accent.danger),
    },
    fontFamily: tokens.font.ui,
    fontFamilyMonospace: tokens.font.mono,
    defaultRadius: "md",
    other: { tokens },
  });
}

/**
 * Precomputed default — used for SSR and the initial render before
 * the client-side color-scheme hook has a chance to swap tokens. The
 * live runtime theme comes from `usePlaygroundTheme`.
 */
export const mantineTheme: MantineThemeOverride = buildMantineTheme(darkTokens);
