import type { CSSProperties } from "react";

/** Graph node. `id` is required; the rest are optional. The engine writes
 *  back `x/y/z` (and `vx/vy/vz`) every tick, and the host can pin a node
 *  by setting `fx/fy/fz`. */
export interface NodeObject {
  id: string | number;
  x?: number;
  y?: number;
  z?: number;
  vx?: number;
  vy?: number;
  vz?: number;
  fx?: number;
  fy?: number;
  fz?: number;
  label?: string;
  color?: string;
  group?: string | number;
  val?: number;
  [key: string]: unknown;
}

/** Graph link. `source` and `target` are required. */
export interface LinkObject {
  id?: string | number;
  source: string | number | NodeObject;
  target: string | number | NodeObject;
  label?: string;
  color?: string;
  width?: number;
  curvature?: number;
  [key: string]: unknown;
}

export interface GraphData<
  N extends NodeObject = NodeObject,
  L extends LinkObject = LinkObject,
> {
  nodes: N[];
  links: L[];
}

export type GraphMode = "2d" | "3d";

/** Built-in tool identifiers. */
export type ToolId =
  | "select"
  | "pan"
  | "add-node"
  | "add-link"
  | "delete"
  | "duplicate"
  | "select-all"
  | "fit"
  | "zoom-in"
  | "zoom-out"
  | "pause"
  | "resume"
  | "screenshot"
  | "export-json"
  | "import-json"
  | "toggle-mode";

/** Accepts a literal value, a property accessor string, or a function. */
export type Accessor<T, In> = T | string | ((obj: In) => T);

export type DagMode =
  | "td"
  | "bu"
  | "lr"
  | "rl"
  | "radialout"
  | "radialin"
  | null;

/** CSS-variable theme. Each key maps to one of the `--lgc-*` variables.
 *  Only set what you want to override. */
export interface LoraGraphTheme {
  background?: string;
  foreground?: string;
  border?: string;
  accent?: string;
  toolbarBackground?: string;
  toolbarForeground?: string;
  toolbarBorder?: string;
  toolActiveBackground?: string;
  toolHoverBackground?: string;
  tooltipBackground?: string;
  tooltipForeground?: string;
  menuBackground?: string;
  menuForeground?: string;
  menuHoverBackground?: string;
  fontFamily?: string;
  fontSize?: string;
}

export interface ToolbarConfig {
  include?: ToolId[];
  exclude?: ToolId[];
  position?: "top" | "top-right" | "top-left" | "bottom";
}

export interface LoraGraphCanvasProps<
  N extends NodeObject = NodeObject,
  L extends LinkObject = LinkObject,
> {
  // Data
  data?: GraphData<N, L>;
  defaultData?: GraphData<N, L>;
  onDataChange?: (next: GraphData<N, L>) => void;

  // Mode
  mode?: GraphMode;
  defaultMode?: GraphMode;
  onModeChange?: (mode: GraphMode) => void;

  // Layout
  width?: number;
  height?: number;
  className?: string;
  style?: CSSProperties;
  backgroundColor?: string;
  theme?: Partial<LoraGraphTheme>;

  // Node styling
  nodeColor?: Accessor<string, N>;
  nodeLabel?: Accessor<string | HTMLElement, N>;
  nodeVal?: Accessor<number, N>;
  nodeAutoColorBy?: Accessor<string | null, N>;
  nodeRelSize?: number;

  // Link styling
  linkColor?: Accessor<string, L>;
  linkLabel?: Accessor<string | HTMLElement, L>;
  linkWidth?: Accessor<number, L>;
  linkCurvature?: Accessor<number, L>;
  linkDirectionalArrowLength?: Accessor<number, L>;
  linkDirectionalArrowColor?: Accessor<string, L>;
  linkDirectionalArrowRelPos?: Accessor<number, L>;
  linkDirectionalParticles?: Accessor<number, L>;
  linkDirectionalParticleSpeed?: Accessor<number, L>;
  linkDirectionalParticleWidth?: Accessor<number, L>;
  linkDirectionalParticleColor?: Accessor<string, L>;

  // Forces / physics
  cooldownTicks?: number;
  cooldownTime?: number;
  warmupTicks?: number;
  d3AlphaDecay?: number;
  d3VelocityDecay?: number;
  d3AlphaMin?: number;
  dagMode?: DagMode;
  dagLevelDistance?: number;

  // UI chrome
  tools?: boolean | ToolId[] | ToolbarConfig;
  showContextMenu?: boolean;
  showLegend?: boolean;
  selection?: "none" | "single" | "multi";

  // Feature toggles — let the host disable any of the built-in
  // editing flows. All default to `true`.
  /** Allow double-clicking a node to rename it inline. */
  enableRename?: boolean;
  /** Allow ⌘C / ⌘X / ⌘V (and the matching ref methods). */
  enableClipboard?: boolean;

  // Interactions
  enableNodeDrag?: boolean;
  enableZoom?: boolean;
  enablePan?: boolean;
  enablePointerInteraction?: boolean;
  linkHoverPrecision?: number;

  onNodeClick?: (node: N, event: MouseEvent) => void;
  onNodeRightClick?: (node: N, event: MouseEvent) => void;
  onNodeHover?: (node: N | null, previousNode: N | null) => void;
  onNodeDoubleClick?: (node: N, event: MouseEvent) => void;
  onNodeDrag?: (node: N, translate: { x: number; y: number; z?: number }) => void;
  onNodeDragEnd?: (node: N, translate: { x: number; y: number; z?: number }) => void;
  onLinkClick?: (link: L, event: MouseEvent) => void;
  onLinkRightClick?: (link: L, event: MouseEvent) => void;
  onLinkHover?: (link: L | null, previousLink: L | null) => void;
  onBackgroundClick?: (event: MouseEvent) => void;
  onBackgroundRightClick?: (event: MouseEvent) => void;
  onSelectionChange?: (selectedIds: Array<string | number>) => void;
  onEngineTick?: () => void;
  onEngineStop?: () => void;

  // Editing lifecycle hooks. Each fires *after* the underlying data
  // mutation, so `getData()` already reflects the new state.
  /** Fires after a node's label has been changed via rename. */
  onNodeRename?: (
    node: N,
    nextLabel: string,
    previousLabel: string | undefined,
  ) => void;
  /** Fires when the user copies nodes (⌘C) — receives the snapshot. */
  onCopy?: (nodes: N[]) => void;
  /** Fires when the user cuts nodes (⌘X) — receives the snapshot
   *  *before* the originals are removed from the graph. */
  onCut?: (nodes: N[]) => void;
  /** Fires after a paste places new nodes in the graph. */
  onPaste?: (nodes: N[]) => void;

  // Escape hatches (renderer-specific; ignored in the other mode)
  nodeCanvasObject?: (
    n: N,
    ctx: CanvasRenderingContext2D,
    globalScale: number,
  ) => void;
  linkCanvasObject?: (
    l: L,
    ctx: CanvasRenderingContext2D,
    globalScale: number,
  ) => void;
  nodeThreeObject?: (n: N) => unknown;
}

export interface LoraGraphCanvasHandle<
  N extends NodeObject = NodeObject,
  L extends LinkObject = LinkObject,
> {
  // Data
  getData(): GraphData<N, L>;
  setData(next: GraphData<N, L>): void;
  addNode(
    node?: Partial<N> & { id?: string | number },
    opts?: { at?: { x: number; y: number; z?: number } },
  ): N;
  addNodes(nodes: Array<Partial<N> & { id?: string | number }>): N[];
  updateNode(id: string | number, patch: Partial<N>): void;
  removeNode(id: string | number): void;
  removeNodes(ids: Array<string | number>): void;
  addLink(link: {
    source: string | number;
    target: string | number;
    id?: string | number;
  } & Partial<L>): L;
  addLinks(
    links: Array<
      { source: string | number; target: string | number } & Partial<L>
    >,
  ): L[];
  removeLink(predicate: (l: L) => boolean): void;
  clear(): void;

  // Selection
  getSelection(): Array<string | number>;
  setSelection(ids: Array<string | number>): void;
  selectAll(): void;
  clearSelection(): void;

  // View / camera
  getMode(): GraphMode;
  setMode(mode: GraphMode): void;
  fit(durationMs?: number, padding?: number): void;
  centerAt(x: number, y: number, z?: number, durationMs?: number): void;
  zoom(scale: number, durationMs?: number): void;
  zoomIn(step?: number): void;
  zoomOut(step?: number): void;

  // Clipboard / duplication
  copy(): N[];
  cut(): N[];
  paste(opts?: { at?: { x: number; y: number; z?: number } }): N[];
  duplicate(): N[];

  // Editing
  renameNode(id: string | number, label: string): void;
  togglePin(id: string | number): void;

  // Import / export
  exportJSON(): string;
  importJSON(json: string): void;
  downloadJSON(filename?: string): void;

  // Engine
  pause(): void;
  resume(): void;
  reheat(): void;
  screenshot(): Promise<Blob | null>;

  // Raw engine handle (escape hatch). Returns null when the active mode
  // is not the one requested.
  engine2D(): unknown | null;
  engine3D(): unknown | null;
}
