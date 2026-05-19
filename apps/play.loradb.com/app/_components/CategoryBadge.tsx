"use client";

/**
 * Renders a Mantine `Badge` coloured from `tokens.category` so every
 * outline chip, plan-view section badge, inspector pill and schema
 * glyph picks its hue from one place. The `kind` maps directly to a
 * Cypher concept (variable / label / rel-type / parameter / node /
 * relationship) and is the only knob a caller needs.
 */

import { Badge, type BadgeProps } from "@mantine/core";

import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import type { Tokens } from "@/lib/theme/tokens";

export type CategoryKind =
  | "variable"
  | "label"
  | "relType"
  | "parameter"
  | "node"
  | "relationship";

type FgKey =
  | "variable"
  | "label"
  | "relType"
  | "parameter"
  | "node"
  | "relationship";

const FG: Record<CategoryKind, FgKey> = {
  variable: "variable",
  label: "label",
  relType: "relType",
  parameter: "parameter",
  node: "node",
  relationship: "relationship",
};

function colorsFor(tokens: Tokens, kind: CategoryKind) {
  const fg = FG[kind];
  return {
    fg: tokens.category[fg],
    bg: tokens.category[`${fg}Bg` as const],
  };
}

export interface CategoryBadgeProps extends Omit<BadgeProps, "color"> {
  kind: CategoryKind;
}

export function CategoryBadge({
  kind,
  size = "sm",
  variant = "light",
  style,
  ...rest
}: CategoryBadgeProps) {
  const { tokens } = usePlaygroundTheme();
  const { fg, bg } = colorsFor(tokens, kind);
  return (
    <Badge
      size={size}
      variant={variant}
      style={{
        color: fg,
        background: bg,
        borderColor: "transparent",
        ...style,
      }}
      {...rest}
    />
  );
}
