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

/** Where a delete originated. Hosts often want to gate UI-driven deletes
 *  (show a confirm modal) but trust `imperative` calls they made
 *  themselves, so the source string lets the guard short-circuit. */
export type DeletionSource =
  | "keyboard"
  | "toolbar"
  | "selectionPanel"
  | "contextMenu"
  | "cut"
  | "imperative";

/** Returns `true` to allow the deletion, `false` to cancel. Can be
 *  async — the caller awaits it before mutating data. A thrown error
 *  is treated as a cancel. Called once per batch (not per item). */
export type DeletionGuard<T> = (
  items: T[],
  ctx: { source: DeletionSource },
) => boolean | Promise<boolean>;

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

  // Data field mapping (when host data uses non-standard property names)
  nodeId?: string;
  linkSource?: string;
  linkTarget?: string;

  // Node styling
  nodeColor?: Accessor<string, N>;
  nodeLabel?: Accessor<string | HTMLElement, N>;
  nodeVal?: Accessor<number, N>;
  nodeAutoColorBy?: Accessor<string | null, N>;
  nodeRelSize?: number;
  nodeVisibility?: Accessor<boolean, N>;
  nodeOpacity?: number;
  nodeResolution?: number;

  // Link styling
  linkColor?: Accessor<string, L>;
  linkLabel?: Accessor<string | HTMLElement, L>;
  linkWidth?: Accessor<number, L>;
  linkCurvature?: Accessor<number, L>;
  linkLineDash?: Accessor<number[] | null, L>;
  linkAutoColorBy?: Accessor<string | null, L>;
  linkVisibility?: Accessor<boolean, L>;
  linkOpacity?: number;
  linkResolution?: number;
  linkCurveRotation?: Accessor<number, L>;
  linkDirectionalArrowLength?: Accessor<number, L>;
  linkDirectionalArrowColor?: Accessor<string, L>;
  linkDirectionalArrowRelPos?: Accessor<number, L>;
  linkDirectionalArrowResolution?: number;
  linkDirectionalParticles?: Accessor<number, L>;
  linkDirectionalParticleSpeed?: Accessor<number, L>;
  linkDirectionalParticleWidth?: Accessor<number, L>;
  linkDirectionalParticleOffset?: Accessor<number, L>;
  linkDirectionalParticleColor?: Accessor<string, L>;
  linkDirectionalParticleResolution?: number;

  // Forces / physics
  cooldownTicks?: number;
  cooldownTime?: number;
  warmupTicks?: number;
  d3AlphaDecay?: number;
  d3VelocityDecay?: number;
  d3AlphaMin?: number;
  d3AlphaTarget?: number;
  dagMode?: DagMode;
  dagLevelDistance?: number;
  dagNodeFilter?: (node: N) => boolean;
  onDagError?: (loopNodeIds: Array<string | number>) => void;
  /** 3D-only: drop layout to 1, 2, or 3 dimensions (flattens z when 2). */
  numDimensions?: 1 | 2 | 3;
  /** 3D-only: switch the layout engine. ngraph is ~3× faster for >10k nodes. */
  forceEngine?: "d3" | "ngraph";
  /** 3D-only: ngraph physics tuning, passed straight through. */
  ngraphPhysics?: object;
  /** 3D-only: throttle the raycaster used for hover / click hit
   *  testing. Defaults to 0 (every frame). Bump to 50-200ms on
   *  huge graphs to skip raycasts on most frames — the perf tier
   *  picks a sensible value automatically. */
  pointerRaycasterThrottleMs?: number;
  /** Add a node-collision force so circles don't overlap. `true` uses
   *  `nodeRelSize` as the radius; pass a number to override. */
  collideNodes?: boolean | number;

  // UI chrome
  tools?: boolean | ToolId[] | ToolbarConfig;
  showContextMenu?: boolean;
  showLegend?: boolean;
  selection?: "none" | "single" | "multi";

  // Feature toggles — let the host disable any of the built-in
  // editing flows. All default to `true`.
  /** Allow ⌘C / ⌘X / ⌘V (and the matching ref methods). */
  enableClipboard?: boolean;
  /** Render the floating tooltip (the cursor-anchored label resolved
   *  from `nodeLabel` / `linkLabel`) on hover. Defaults to `false` —
   *  the hover state still drives neighbour highlighting and on-canvas
   *  labels, just without the mouse-attached pill. */
  enableTooltip?: boolean;
  /** Play an animated "zoom in to the bounds" intro on first mount.
   *  The camera starts pulled back proportional to node count (so a
   *  sparse 10-node graph reveals from ~×3 out and a 10k stress graph
   *  from ~×6 out) and tweens into the fitted view. Defaults to
   *  `true`. Pass `false` to suppress the intro and snap to the
   *  kapsule's own initial fit instead. */
  introZoom?: boolean;

  // Interactions
  enableNodeDrag?: boolean;
  enableZoom?: boolean;
  enablePan?: boolean;
  enablePointerInteraction?: boolean;
  enableNavigationControls?: boolean;
  linkHoverPrecision?: number;
  minZoom?: number;
  maxZoom?: number;
  showPointerCursor?: Accessor<boolean, N | L>;
  showNavInfo?: boolean;
  autoPauseRedraw?: boolean;
  /** When true (default false), clicking a node animates the camera
   *  toward it; clicking the same node again restores the prior view. */
  focusOnClick?: boolean;
  /** When true (default false), hovering a node highlights it and its
   *  neighbours via the accent color. Requires cross-linked neighbour
   *  refs in the node data, or set `autoIndexNeighbors`. */
  highlightNeighborsOnHover?: boolean;
  /** Auto-build `node._neighbors` and `node._links` arrays after every
   *  data change so `highlightNeighborsOnHover` has something to walk. */
  autoIndexNeighbors?: boolean;
  /** Background helper: draw a faint grid behind the canvas. Useful in
   *  a playground. 2D only. */
  showGrid?: boolean | { spacing?: number; color?: string };
  /** Render each node's label directly on the 2D canvas (under the
   *  node). When this is on, the default node colour is automatically
   *  faded so the label text reads clearly on top of overlapping
   *  nodes. 2D only — in 3D, use `nodeLabel` for the hover tooltip. */
  showLabels?: boolean;
  /** When `true` (default), releasing a dragged node pins it at its
   *  new position by writing `fx`/`fy`/`fz`. Dragging a node that's
   *  part of the current selection moves all selected nodes together
   *  and pins them as a group on release. Set to `false` to keep the
   *  default simulation-pull behaviour. */
  fixOnDrop?: boolean;

  /** When `true` (default `false`), the camera animates to fit the
   *  current selection any time it changes — like the toolbar "fit"
   *  button, but scoped to the selected nodes (and the endpoints of
   *  any selected links). No-ops when the selection becomes empty;
   *  the user keeps their view instead of being snapped back. */
  fitOnSelect?: boolean;

  /** Auto-tune renderer / simulation settings based on graph size so
   *  the canvas stays responsive into the 50k-100k-node range.
   *
   *  - `"auto"` (default): pick a tier from the live node + link count.
   *  - `"off"`: never inject perf defaults — only the host's props
   *    drive the kapsule.
   *  - `"default" | "large" | "xlarge" | "huge"`: force a specific
   *    tier regardless of size (useful for benchmarking).
   *
   *  Each tier overrides things like `cooldownTicks`, `d3AlphaDecay`,
   *  3D `nodeResolution`/`linkResolution`, and the 3D layout engine.
   *  Any prop the host sets explicitly always wins. */
  performanceProfile?:
    | "auto"
    | "off"
    | "default"
    | "large"
    | "xlarge"
    | "huge";

  /** Switch the simulation into a beeswarm layout: nodes spread along
   *  one axis driven by a value accessor, with a weak orthogonal pull
   *  and collision so they don't overlap. Pass `true` for an
   *  id-hashed spread, or an object to control the value source and
   *  axis. Disables the default `center` + `charge` forces while
   *  active. */
  beeswarm?:
    | boolean
    | {
        /** Target axis (default `"x"`). */
        axis?: "x" | "y";
        /** Position along the chosen axis — `(node) => number` or a
         *  property key whose value is numeric. Defaults to a stable
         *  hash of the node id. */
        value?: string | ((n: N) => number);
        /** Strength of the orthogonal pull-to-zero force. Default 0.2. */
        strength?: number;
      };

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
  onZoom?: (transform: { k: number; x: number; y: number }) => void;
  onZoomEnd?: (transform: { k: number; x: number; y: number }) => void;
  onRenderFramePre?: (
    ctx: CanvasRenderingContext2D,
    globalScale: number,
  ) => void;
  onRenderFramePost?: (
    ctx: CanvasRenderingContext2D,
    globalScale: number,
  ) => void;

  /** Optional async gate fired before nodes are removed. Receives every
   *  node in the batch (single-node deletes get a 1-length array). Resolve
   *  `true` to proceed, `false` to cancel. Throws are treated as cancel.
   *  Fires for: keyboard, toolbar, selection-panel, context-menu, cut,
   *  and `handle.removeNode(s)` calls — discriminate via `ctx.source`. */
  onBeforeNodeDelete?: DeletionGuard<N>;
  /** Same shape as `onBeforeNodeDelete` for links. */
  onBeforeLinkDelete?: DeletionGuard<L>;
  /** Fires after a node delete has been applied to the data. */
  onNodeDeleted?: (nodes: N[], ctx: { source: DeletionSource }) => void;
  /** Fires after a link delete has been applied to the data. */
  onLinkDeleted?: (links: L[], ctx: { source: DeletionSource }) => void;

  // Editing lifecycle hooks. Each fires *after* the underlying data
  // mutation, so `getData()` already reflects the new state.
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
  nodeCanvasObjectMode?:
    | "replace"
    | "before"
    | "after"
    | ((n: N) => "replace" | "before" | "after" | undefined);
  nodePointerAreaPaint?: (
    n: N,
    color: string,
    ctx: CanvasRenderingContext2D,
    globalScale: number,
  ) => void;
  linkCanvasObject?: (
    l: L,
    ctx: CanvasRenderingContext2D,
    globalScale: number,
  ) => void;
  linkCanvasObjectMode?:
    | "replace"
    | "before"
    | "after"
    | ((l: L) => "replace" | "before" | "after" | undefined);
  linkPointerAreaPaint?: (
    l: L,
    color: string,
    ctx: CanvasRenderingContext2D,
    globalScale: number,
  ) => void;
  nodeThreeObject?: (n: N) => unknown;
  nodeThreeObjectExtend?: Accessor<boolean, N>;
  linkThreeObject?: (l: L) => unknown;
  linkThreeObjectExtend?: Accessor<boolean, L>;
  linkMaterial?: Accessor<unknown, L>;
  nodePositionUpdate?:
    | ((
        obj: unknown,
        coords: { x: number; y: number; z: number },
        node: N,
      ) => void | boolean | null)
    | null;
  linkPositionUpdate?:
    | ((
        obj: unknown,
        coords: {
          start: { x: number; y: number; z: number };
          end: { x: number; y: number; z: number };
        },
        link: L,
      ) => void | boolean | null)
    | null;
  /** 3D init-only — passed once on engine construction. */
  controlType?: "trackball" | "orbit" | "fly";
  rendererConfig?: Record<string, unknown>;
  extraRenderers?: unknown[];
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
  /** Returns a promise that resolves to `true` when the node was removed.
   *  When the host passes `onBeforeNodeDelete`, the guard runs with
   *  `source: "imperative"` and may cancel — the promise resolves
   *  `false` and the graph is unchanged. Hosts that don't pass a guard
   *  can ignore the promise; the data mutation is observable on the
   *  same tick (only the resolved promise itself is async). */
  removeNode(id: string | number): Promise<boolean>;
  removeNodes(ids: Array<string | number>): Promise<boolean>;
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
  /** Same gate semantics as `removeNode` — `onBeforeLinkDelete` runs with
   *  `source: "imperative"` and may cancel. */
  removeLink(predicate: (l: L) => boolean): Promise<boolean>;
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
  /** Translate the view by a world-space delta. In 2D the z component
   *  is ignored (the camera height is locked). */
  panBy(
    delta: { x?: number; y?: number; z?: number },
    durationMs?: number,
  ): void;
  /** Animate to a world coordinate, preserving the current viewing
   *  direction. Use for "jump to coordinates" UI. */
  goTo(
    target: { x: number; y: number; z?: number },
    opts?: { durationMs?: number },
  ): void;
  /** Fit the view to a subset of nodes. Empty array → full fit. */
  fitToNodes(
    nodeIds: ReadonlyArray<string | number>,
    durationMs?: number,
    padding?: number,
  ): void;
  /** Fit to the current selection (nodes + endpoints of selected links).
   *  No-op if nothing is selected. */
  fitToSelection(durationMs?: number, padding?: number): void;

  // Clipboard / duplication
  copy(): N[];
  /** Cut funnels through the same delete-gate as keyboard / toolbar
   *  cuts. A rejected guard resolves to an empty array and leaves the
   *  graph + clipboard untouched. */
  cut(): Promise<N[]>;
  paste(opts?: { at?: { x: number; y: number; z?: number } }): N[];
  duplicate(): N[];
  /** Create a fresh node and link each currently selected node to it.
   *  No-op when the selection is empty. Returns the new node, or null
   *  if nothing was created. */
  addConnectedNode(opts?: {
    at?: { x: number; y: number; z?: number };
    label?: string;
  }): N | null;

  // Editing
  togglePin(id: string | number): void;

  // Import / export
  exportJSON(): string;
  importJSON(json: string): void;
  downloadJSON(filename?: string): void;

  // Engine
  pause(): void;
  resume(): void;
  reheat(): void;
  /** Get / set / clear a d3-force by name. Pass `null` to remove. */
  d3Force(name: string): unknown;
  d3Force(name: string, fn: unknown | null): void;
  /** Emit a one-off particle along a link (visual ping for events). */
  emitParticle(link: L): void;
  /** Halt any in-flight camera animation (e.g. a focus tween) and
   *  freeze the camera at its current state. */
  stopAnimation(): void;
  screenshot(): Promise<Blob | null>;

  // Raw engine handle (escape hatch). Returns null when the active mode
  // is not the one requested.
  engine2D(): unknown | null;
  engine3D(): unknown | null;
}
