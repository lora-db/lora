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
  { key: "backgroundColor", setter: "backgroundColor", supports: both },
  { key: "nodeColor", setter: "nodeColor", supports: both },
  { key: "nodeLabel", setter: "nodeLabel", supports: both },
  { key: "nodeVal", setter: "nodeVal", supports: both },
  { key: "nodeAutoColorBy", setter: "nodeAutoColorBy", supports: both },
  { key: "nodeRelSize", setter: "nodeRelSize", supports: both },
  { key: "linkColor", setter: "linkColor", supports: both },
  { key: "linkLabel", setter: "linkLabel", supports: both },
  { key: "linkWidth", setter: "linkWidth", supports: both },
  { key: "linkCurvature", setter: "linkCurvature", supports: both },
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
    key: "linkDirectionalParticleColor",
    setter: "linkDirectionalParticleColor",
    supports: both,
  },
  { key: "cooldownTicks", setter: "cooldownTicks", supports: both },
  { key: "cooldownTime", setter: "cooldownTime", supports: both },
  { key: "warmupTicks", setter: "warmupTicks", supports: both },
  { key: "d3AlphaDecay", setter: "d3AlphaDecay", supports: both },
  { key: "d3VelocityDecay", setter: "d3VelocityDecay", supports: both },
  { key: "d3AlphaMin", setter: "d3AlphaMin", supports: both },
  { key: "dagMode", setter: "dagMode", supports: both },
  { key: "dagLevelDistance", setter: "dagLevelDistance", supports: both },
  { key: "enableNodeDrag", setter: "enableNodeDrag", supports: both },
  {
    key: "enablePointerInteraction",
    setter: "enablePointerInteraction",
    supports: both,
  },
  { key: "linkHoverPrecision", setter: "linkHoverPrecision", supports: both },
  // 2D-only — pan/zoom are wired to dedicated kapsule keys; 3D uses
  // navigation controls instead.
  { key: "enableZoom", setter: "enableZoomInteraction", supports: only2d },
  { key: "enablePan", setter: "enablePanInteraction", supports: only2d },
  { key: "nodeCanvasObject", setter: "nodeCanvasObject", supports: only2d },
  { key: "linkCanvasObject", setter: "linkCanvasObject", supports: only2d },
  // 3D-only
  { key: "nodeThreeObject", setter: "nodeThreeObject", supports: only3d },
];

/** Walk the bindings, calling each engine setter for props whose value
 *  changed (using Object.is). The engine instance is treated as a loose
 *  record of chainable setters so this works for both kapsule types. */
export function applyDiffedProps<
  N extends NodeObject,
  L extends LinkObject,
>(
  engine: Record<string, (value: unknown) => unknown>,
  props: LoraGraphCanvasProps<N, L>,
  prev: LoraGraphCanvasProps<N, L>,
  mode: "2d" | "3d",
): void {
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

export type EventName = (typeof EVENT_BINDINGS)[number];
