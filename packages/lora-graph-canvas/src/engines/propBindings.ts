import type { LinkObject, LoraGraphCanvasProps, NodeObject } from "../types";

/** A kapsule prop the engine exposes as a chainable setter. */
export interface PropBinding<P> {
  /** Name of the prop on `LoraGraphCanvasProps` we read from. */
  key: keyof LoraGraphCanvasProps<NodeObject, LinkObject>;
  /** Name of the chainable setter on the engine instance. */
  setter: string;
  /** Whether this binding is supported in the current engine mode. */
  supports: (mode: "2d" | "3d") => boolean;
  /** Optional transformer applied before passing the value to the setter. */
  transform?: (value: unknown, props: P) => unknown;
}

const both = () => true;
const only2d = (m: "2d" | "3d") => m === "2d";
const only3d = (m: "2d" | "3d") => m === "3d";

/** Bindings shared between 2D and 3D engines. Order doesn't matter; the
 *  adapter just walks the list each render. */
export const SHARED_BINDINGS: PropBinding<unknown>[] = [
  // ─ Data field mapping ─
  { key: "nodeId", setter: "nodeId", supports: both },
  { key: "linkSource", setter: "linkSource", supports: both },
  { key: "linkTarget", setter: "linkTarget", supports: both },

  // ─ Container / general ─
  { key: "backgroundColor", setter: "backgroundColor", supports: both },

  // ─ Node styling ─
  { key: "nodeColor", setter: "nodeColor", supports: both },
  { key: "nodeLabel", setter: "nodeLabel", supports: both },
  { key: "nodeVal", setter: "nodeVal", supports: both },
  { key: "nodeAutoColorBy", setter: "nodeAutoColorBy", supports: both },
  { key: "nodeRelSize", setter: "nodeRelSize", supports: both },
  { key: "nodeVisibility", setter: "nodeVisibility", supports: both },

  // ─ Link styling ─
  { key: "linkColor", setter: "linkColor", supports: both },
  { key: "linkLabel", setter: "linkLabel", supports: both },
  { key: "linkWidth", setter: "linkWidth", supports: both },
  { key: "linkCurvature", setter: "linkCurvature", supports: both },
  { key: "linkAutoColorBy", setter: "linkAutoColorBy", supports: both },
  { key: "linkVisibility", setter: "linkVisibility", supports: both },
  {
    key: "linkDirectionalArrowLength",
    setter: "linkDirectionalArrowLength",
    supports: both,
  },
  {
    key: "linkDirectionalArrowColor",
    setter: "linkDirectionalArrowColor",
    supports: both,
  },
  {
    key: "linkDirectionalArrowRelPos",
    setter: "linkDirectionalArrowRelPos",
    supports: both,
  },
  {
    key: "linkDirectionalParticles",
    setter: "linkDirectionalParticles",
    supports: both,
  },
  {
    key: "linkDirectionalParticleSpeed",
    setter: "linkDirectionalParticleSpeed",
    supports: both,
  },
  {
    key: "linkDirectionalParticleWidth",
    setter: "linkDirectionalParticleWidth",
    supports: both,
  },
  {
    key: "linkDirectionalParticleOffset",
    setter: "linkDirectionalParticleOffset",
    supports: both,
  },
  {
    key: "linkDirectionalParticleColor",
    setter: "linkDirectionalParticleColor",
    supports: both,
  },

  // ─ Forces / physics ─
  { key: "cooldownTicks", setter: "cooldownTicks", supports: both },
  { key: "cooldownTime", setter: "cooldownTime", supports: both },
  { key: "warmupTicks", setter: "warmupTicks", supports: both },
  { key: "d3AlphaDecay", setter: "d3AlphaDecay", supports: both },
  { key: "d3VelocityDecay", setter: "d3VelocityDecay", supports: both },
  { key: "d3AlphaMin", setter: "d3AlphaMin", supports: both },
  { key: "d3AlphaTarget", setter: "d3AlphaTarget", supports: both },
  { key: "dagMode", setter: "dagMode", supports: both },
  { key: "dagLevelDistance", setter: "dagLevelDistance", supports: both },
  { key: "dagNodeFilter", setter: "dagNodeFilter", supports: both },
  { key: "onDagError", setter: "onDagError", supports: both },

  // ─ Interaction ─
  { key: "enableNodeDrag", setter: "enableNodeDrag", supports: both },
  {
    key: "enablePointerInteraction",
    setter: "enablePointerInteraction",
    supports: both,
  },
  { key: "linkHoverPrecision", setter: "linkHoverPrecision", supports: both },
  { key: "showPointerCursor", setter: "showPointerCursor", supports: both },

  // ─ 2D-only ─
  { key: "enableZoom", setter: "enableZoomInteraction", supports: only2d },
  { key: "enablePan", setter: "enablePanInteraction", supports: only2d },
  { key: "minZoom", setter: "minZoom", supports: only2d },
  { key: "maxZoom", setter: "maxZoom", supports: only2d },
  { key: "autoPauseRedraw", setter: "autoPauseRedraw", supports: only2d },
  { key: "linkLineDash", setter: "linkLineDash", supports: only2d },
  { key: "nodeCanvasObject", setter: "nodeCanvasObject", supports: only2d },
  {
    key: "nodeCanvasObjectMode",
    setter: "nodeCanvasObjectMode",
    supports: only2d,
  },
  {
    key: "nodePointerAreaPaint",
    setter: "nodePointerAreaPaint",
    supports: only2d,
  },
  { key: "linkCanvasObject", setter: "linkCanvasObject", supports: only2d },
  {
    key: "linkCanvasObjectMode",
    setter: "linkCanvasObjectMode",
    supports: only2d,
  },
  {
    key: "linkPointerAreaPaint",
    setter: "linkPointerAreaPaint",
    supports: only2d,
  },

  // ─ 3D-only ─
  { key: "showNavInfo", setter: "showNavInfo", supports: only3d },
  {
    key: "enableNavigationControls",
    setter: "enableNavigationControls",
    supports: only3d,
  },
  { key: "nodeOpacity", setter: "nodeOpacity", supports: only3d },
  { key: "nodeResolution", setter: "nodeResolution", supports: only3d },
  { key: "linkOpacity", setter: "linkOpacity", supports: only3d },
  { key: "linkResolution", setter: "linkResolution", supports: only3d },
  { key: "linkCurveRotation", setter: "linkCurveRotation", supports: only3d },
  { key: "linkMaterial", setter: "linkMaterial", supports: only3d },
  { key: "nodeThreeObject", setter: "nodeThreeObject", supports: only3d },
  {
    key: "nodeThreeObjectExtend",
    setter: "nodeThreeObjectExtend",
    supports: only3d,
  },
  { key: "linkThreeObject", setter: "linkThreeObject", supports: only3d },
  {
    key: "linkThreeObjectExtend",
    setter: "linkThreeObjectExtend",
    supports: only3d,
  },
  { key: "nodePositionUpdate", setter: "nodePositionUpdate", supports: only3d },
  { key: "linkPositionUpdate", setter: "linkPositionUpdate", supports: only3d },
  {
    key: "linkDirectionalArrowResolution",
    setter: "linkDirectionalArrowResolution",
    supports: only3d,
  },
  {
    key: "linkDirectionalParticleResolution",
    setter: "linkDirectionalParticleResolution",
    supports: only3d,
  },
  { key: "numDimensions", setter: "numDimensions", supports: only3d },
  { key: "forceEngine", setter: "forceEngine", supports: only3d },
  { key: "ngraphPhysics", setter: "ngraphPhysics", supports: only3d },
  {
    key: "pointerRaycasterThrottleMs",
    setter: "pointerRaycasterThrottleMs",
    supports: only3d,
  },
];

/** Walk the bindings, calling each engine setter for props whose value
 *  changed (using Object.is). The engine instance is treated as a loose
 *  record of chainable setters so this works for both kapsule types. */
export function applyDiffedProps<N extends NodeObject, L extends LinkObject>(
  engine: Record<string, (value: unknown) => unknown>,
  props: LoraGraphCanvasProps<N, L>,
  prev: LoraGraphCanvasProps<N, L>,
  mode: "2d" | "3d",
): void {
  // useGraphEngine forwards on every render to avoid stale-closure
  // ordering issues; short-circuit when the prop bag identity is the
  // same so we don't walk 30+ bindings doing Object.is checks per render.
  if (props === prev) return;
  for (const binding of SHARED_BINDINGS) {
    if (!binding.supports(mode)) continue;
    const next = props[binding.key as keyof typeof props];
    const old = prev[binding.key as keyof typeof prev];
    if (Object.is(next, old)) continue;
    const setter = engine[binding.setter];
    if (typeof setter !== "function") continue;
    setter.call(engine, next);
  }
}

/** Event-handler bindings. These are wired once at engine construction;
 *  the React layer forwards through a stable trampoline so latest props
 *  always win without re-binding. */
export const EVENT_BINDINGS = [
  "onNodeClick",
  "onNodeRightClick",
  "onNodeHover",
  "onNodeDrag",
  "onNodeDragEnd",
  "onLinkClick",
  "onLinkRightClick",
  "onLinkHover",
  "onBackgroundClick",
  "onBackgroundRightClick",
  "onEngineTick",
  "onEngineStop",
] as const;

/** 2D-only event bindings (zoom + render-frame callbacks). The 3D
 *  kapsule doesn't expose these — they're driven by Three.js controls
 *  there. We wire them in the same trampoline way for consistency. */
export const EVENT_BINDINGS_2D = [
  "onZoom",
  "onZoomEnd",
  "onRenderFramePre",
  "onRenderFramePost",
] as const;

export type EventName = (typeof EVENT_BINDINGS)[number];
export type EventName2D = (typeof EVENT_BINDINGS_2D)[number];
