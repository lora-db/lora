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
