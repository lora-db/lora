/**
 * Design tokens — the single source of truth for the playground's
 * colour and typography decisions. All four surfaces (Mantine, the
 * `LoraQueryEditor`, the `LoraGraphCanvas`, and the Glide Data Grid)
 * derive their themes from this shape via the sibling `*.ts`
 * derivers in `lib/theme/`.
 *
 * Values are CSS-ready strings — hex for solid colours, css strings
 * for font stacks, plain numbers (as px-suffixed strings) for radii.
 *
 * The palettes are VS-Code-inspired: Dark+ for the dark scheme and
 * Light+ for the light scheme. Hues are deliberately conservative so
 * the editor / canvas / grid sit comfortably next to each other.
 */

/** Decision-level tokens that drive every surface in the playground. */
export interface Tokens {
  bg: {
    app: string;
    panel: string;
    editor: string;
    sidebar: string;
    overlay: string;
  };
  fg: {
    primary: string;
    muted: string;
    subtle: string;
    inverse: string;
  };
  border: {
    subtle: string;
    strong: string;
    focus: string;
  };
  accent: {
    primary: string;
    success: string;
    warning: string;
    danger: string;
    info: string;
  };
  syntax: {
    keyword: string;
    string: string;
    number: string;
    comment: string;
    identifier: string;
    type: string;
    operator: string;
    punctuation: string;
  };
  graph: {
    node: string;
    nodeStroke: string;
    nodeHighlight: string;
    link: string;
    linkHighlight: string;
    label: string;
  };
  /**
   * Semantic category palette — the colour used to identify a Cypher
   * concept wherever it surfaces (outline chips, plan badges, schema
   * browser, inspector, table bubbles, graph canvas, summary strips).
   *
   * `*Bg` variants are pre-computed translucent versions of the same
   * hue, suitable for badge / bubble backgrounds.
   *
   * Hues mirror the `lora-query` editor palette so a `:Person` label
   * looks the same in the editor as on a chip below it.
   */
  category: {
    variable: string;
    variableBg: string;
    label: string;
    labelBg: string;
    relType: string;
    relTypeBg: string;
    parameter: string;
    parameterBg: string;
    /** Alias for `graph.node` — kept here so node colours can be looked
     *  up alongside the other categories. */
    node: string;
    nodeBg: string;
    /** Used for the graph-shape "relationship" concept (counts, summary
     *  chips). Equal to `relType` so chips agree with the rel-type colour. */
    relationship: string;
    relationshipBg: string;
  };
  font: {
    ui: string;
    mono: string;
  };
  radius: {
    sm: string;
    md: string;
    lg: string;
  };
}

const FONT_UI =
  'ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", sans-serif';
const FONT_MONO =
  'ui-monospace, SFMono-Regular, "JetBrains Mono", Menlo, Consolas, "Liberation Mono", monospace';

/** VS Code Dark+ inspired palette. */
export const darkTokens: Tokens = {
  bg: {
    app: "#1e1e1e",
    panel: "#252526",
    editor: "#1e1e1e",
    sidebar: "#333333",
    overlay: "#2d2d30",
  },
  fg: {
    primary: "#d4d4d4",
    muted: "#a0a0a0",
    subtle: "#6e7681",
    inverse: "#1e1e1e",
  },
  border: {
    subtle: "#2d2d30",
    strong: "#3c3c3c",
    focus: "#0e639c",
  },
  accent: {
    primary: "#0e639c",
    success: "#4ec9b0",
    warning: "#dcdcaa",
    danger: "#f48771",
    info: "#9cdcfe",
  },
  syntax: {
    keyword: "#569cd6",
    string: "#ce9178",
    number: "#b5cea8",
    comment: "#6a9955",
    identifier: "#9cdcfe",
    type: "#4ec9b0",
    operator: "#d4d4d4",
    punctuation: "#808080",
  },
  graph: {
    node: "#6aa3ff",
    nodeStroke: "#1e1e1e",
    nodeHighlight: "#9cdcfe",
    link: "#666666",
    linkHighlight: "#9cdcfe",
    label: "#d4d4d4",
  },
  category: {
    variable: "#79c0ff",
    variableBg: "rgba(121, 192, 255, 0.16)",
    label: "#7ee787",
    labelBg: "rgba(126, 231, 135, 0.16)",
    relType: "#ffa657",
    relTypeBg: "rgba(255, 166, 87, 0.16)",
    parameter: "#d2a8ff",
    parameterBg: "rgba(210, 168, 255, 0.16)",
    node: "#6aa3ff",
    nodeBg: "rgba(106, 163, 255, 0.16)",
    relationship: "#ffa657",
    relationshipBg: "rgba(255, 166, 87, 0.16)",
  },
  font: {
    ui: FONT_UI,
    mono: FONT_MONO,
  },
  radius: {
    sm: "4px",
    md: "6px",
    lg: "10px",
  },
};

/** VS Code Light+ inspired palette. */
export const lightTokens: Tokens = {
  bg: {
    app: "#ffffff",
    panel: "#f3f3f3",
    editor: "#ffffff",
    sidebar: "#f3f3f3",
    overlay: "#ececec",
  },
  fg: {
    primary: "#1f2328",
    muted: "#57606a",
    subtle: "#8b949e",
    inverse: "#ffffff",
  },
  border: {
    subtle: "#e1e4e8",
    strong: "#c8ccd1",
    focus: "#0969da",
  },
  accent: {
    primary: "#0969da",
    success: "#1f883d",
    warning: "#bf8700",
    danger: "#cf222e",
    info: "#0969da",
  },
  syntax: {
    keyword: "#0000ff",
    string: "#a31515",
    number: "#098658",
    comment: "#008000",
    identifier: "#001080",
    type: "#267f99",
    operator: "#1f2328",
    punctuation: "#6e7781",
  },
  graph: {
    node: "#0969da",
    nodeStroke: "#ffffff",
    nodeHighlight: "#cf222e",
    link: "#afb8c1",
    linkHighlight: "#0969da",
    label: "#1f2328",
  },
  category: {
    variable: "#1e66f5",
    variableBg: "rgba(30, 102, 245, 0.14)",
    label: "#40a02b",
    labelBg: "rgba(64, 160, 43, 0.14)",
    relType: "#df8e1d",
    relTypeBg: "rgba(223, 142, 29, 0.16)",
    parameter: "#8839ef",
    parameterBg: "rgba(136, 57, 239, 0.14)",
    node: "#0969da",
    nodeBg: "rgba(9, 105, 218, 0.14)",
    relationship: "#df8e1d",
    relationshipBg: "rgba(223, 142, 29, 0.16)",
  },
  font: {
    ui: FONT_UI,
    mono: FONT_MONO,
  },
  radius: {
    sm: "4px",
    md: "6px",
    lg: "10px",
  },
};

/** Pick the token set that matches a Mantine colour scheme. */
export function tokensFor(scheme: "light" | "dark"): Tokens {
  return scheme === "dark" ? darkTokens : lightTokens;
}
